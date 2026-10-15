//! Rasterized glyphs, and where each one landed in an atlas texture.
//!
//! The scene stores glyph *references* — a [`GlyphKey`] and a position — never
//! pixels. This is where a reference becomes a rectangle of an atlas: on a
//! miss the glyph is rasterized through [`FontDb`], allocated a region, and
//! uploaded; on a hit nothing touches the font backend or the GPU at all.
//!
//! Pictures go through the same motions into atlases of their own. A glyph
//! atlas is filled by a working set that is bounded — one entry per glyph per
//! size — and is never emptied; a picture is a megabyte at a time and the
//! next card shows six more, so the image atlases are dropped as a set once
//! there are too many of them, and the glyphs never feel it.
//!
//! A picture drawn wider or taller than an atlas row takes none: it gets a
//! texture of its own, at the pixels it was handed — the size it is drawn at,
//! for a picture decoded to it — so a screenshot across a Retina pane is as
//! sharp as its pixels rather than stretched up from an atlas's 1022. Pixels
//! of another size are stretched to the quad by the sampler rather than
//! resampled here: at that size they are a screenful, and here is the thread
//! that draws. So are pixels bigger than an atlas drawn within a row, since a
//! resample costs what its source weighs. Such a texture is a whole texture
//! to give back, which a region of an atlas never is, so it is kept exactly
//! while it is drawn.
//!
//! Ported from Warp's `crates/warpui/src/rendering/glyph_cache.rs` (MIT). Warp
//! parameterizes the cache over four `dyn Fn` callbacks so one implementation
//! can serve both its Metal and its wgpu renderer. Crook has one renderer, so
//! the callbacks collapse into a `&dyn FontDb` and an [`AtlasContext`].

use anyhow::Result;
use crookui_core::fonts::{Canvas, GlyphKey, RasterBounds, RasterFormat, SubpixelAlignment};
use crookui_core::geometry::{RectF, Vector2F};
use crookui_core::icons::{self, IconKey, Mark};
use crookui_core::image::{Bitmap, ImageId, resample};
use crookui_core::platform::FontDb;
use rustc_hash::FxHashMap;

use super::atlas::{self, TextureId, TextureOffset};
use super::glyph::{AtlasContext, AtlasTexture};

/// The edge length of one atlas texture, in pixels.
///
/// 1024² of RGBA8 is 4 MB. Masks are stored as RGBA too — the shader reads
/// coverage out of the red channel — so this fills faster than it looks: one
/// entry per glyph, per size, per scale factor, per subpixel bucket.
const ATLAS_SIZE: u32 = 1024;

/// How many image atlases may exist before they are all dropped.
///
/// Eight is 32 MB, and one card of six previews — each at most 1022 device
/// pixels wide and 960 tall, one to an atlas — never reaches it, so the
/// pictures on screen are never the ones being thrown away.
const IMAGE_ATLASES: usize = 8;

/// The longest side an image atlas region may have, in device pixels.
///
/// Two short of the atlas edge rather than one: the shelf allocator places a
/// row only while its height is strictly less than what is left below the
/// row's top, and a region that is the whole atlas tall would be refused twice
/// and burn a texture id on the way. A picture drawn longer than this gets a
/// texture of its own instead — see [`image_region`].
pub const MAX_IMAGE_SIDE: u32 = ATLAS_SIZE - 2;

/// Where a picture is uploaded, and the size it is resampled to there.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ImageRegion {
    /// A region of a shared image atlas, `(width, height)`.
    Atlas(u32, u32),
    /// A texture of the picture's own, `(width, height)`.
    Own(u32, u32),
}

impl ImageRegion {
    /// The width and height the pixels are resampled to.
    pub fn size(self) -> (u32, u32) {
        match self {
            Self::Atlas(width, height) | Self::Own(width, height) => (width, height),
        }
    }
}

/// Where a picture of `pixels` drawn at `drawn` device pixels goes, and at
/// what size.
///
/// An atlas region of `drawn` itself while both sides fit an atlas row and
/// the pixels fit an atlas: they are resampled to it, a few megabytes made
/// out of a few at most, and the sampler reads them one to one. Otherwise a
/// texture of the picture's own at `pixels` — which is `drawn` for a picture
/// decoded at the size it is drawn — and when the two differ the sampler
/// stretches the texture to the quad, or shrinks it. Resampled here instead,
/// a picture handed over at another size than it is drawn — its old pixels,
/// while it is decoded again at a new one, or pixels kept short of a size
/// that would be made up — would be a screenful resampled on the thread that
/// draws at every new size, with a font step a new size for every picture in
/// view. And a resample costs what its source weighs, not what it makes, so
/// pixels bigger than an atlas are not resampled into one either: a
/// 4000-pixel square zoomed back out to 1000 was sixteen million pixels
/// averaged on the thread that draws, for the frame or two before its decode
/// at the new size lands. Drawn from a texture of its own — the one the frame
/// before gave it, when that frame drew it past a row — it costs no resample
/// and no upload; shrunk by the sampler, which has no mipmaps, it aliases
/// until then.
///
/// Only pixels longer on a side than `max_texture_side`, the most a texture
/// may be on this device, are resampled: both sides shrink by the same
/// factor. A side of `drawn` that is nothing stays nothing, so the caller
/// can see the picture is not drawn at all.
pub fn image_region(drawn: (u32, u32), pixels: (u32, u32), max_texture_side: u32) -> ImageRegion {
    let (width, height) = drawn;
    if width.max(height) <= MAX_IMAGE_SIDE && pixels.0.max(pixels.1) <= ATLAS_SIZE {
        return ImageRegion::Atlas(width, height);
    }
    let (width, height) = if width == 0 || height == 0 {
        drawn
    } else {
        pixels
    };
    let longest = width.max(height);
    if longest <= max_texture_side {
        return ImageRegion::Own(width, height);
    }
    let factor = max_texture_side as f32 / longest as f32;
    let side = |pixels| match pixels {
        0 => 0,
        _ => ((pixels as f32 * factor).round() as u32).clamp(1, max_texture_side),
    };
    ImageRegion::Own(side(width), side(height))
}

/// Everything the renderer needs to draw one cached glyph.
#[derive(Copy, Clone, Debug)]
pub struct GlyphTextureOffset {
    /// Which atlas holds it.
    pub texture_id: TextureId,

    /// Where in that atlas, in both UV and pixel space.
    ///
    /// The pixel size here — not the raster bounds — is what the glyph quad
    /// must be sized by. The two can differ, and a quad sized to the smaller
    /// one samples a sub-region of what was uploaded, fringing every glyph.
    pub allocated_region: atlas::AllocatedRegion,

    /// Where the bitmap sits relative to the point where the glyph's left edge
    /// meets the baseline.
    pub raster_bounds: RasterBounds,

    /// Whether the glyph carries its own color, in which case the text color is
    /// ignored.
    pub is_emoji: bool,
}

/// Where one cached icon mask landed.
///
/// Smaller than [`GlyphTextureOffset`] by exactly the fields a glyph needs and
/// an icon does not: an icon is positioned by the square it was asked to fill,
/// not by a baseline, and it is never a colour bitmap.
#[derive(Copy, Clone, Debug)]
pub struct IconTextureOffset {
    /// Which atlas holds it.
    pub texture_id: TextureId,

    /// Where in that atlas, in both UV and pixel space.
    pub allocated_region: atlas::AllocatedRegion,
}

/// Where one cached picture landed.
///
/// The region's pixel size is the size the picture was uploaded at: the size
/// it is drawn at in an atlas, and in a texture of its own the pixels it was
/// handed (see [`image_region`]). The renderer sizes the quad by what it
/// asked for, not by this, and stretches the region when the two differ.
#[derive(Copy, Clone, Debug)]
pub struct ImageTextureOffset {
    /// Which texture holds it.
    pub texture: ImageTexture,

    /// Where in that texture, in both UV and pixel space.
    pub allocated_region: atlas::AllocatedRegion,
}

/// The texture a picture was placed in.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ImageTexture {
    /// An image atlas, shared with other pictures: see
    /// [`GlyphCache::image_texture`].
    Atlas(TextureId),
    /// A texture of the picture's own: see [`GlyphCache::own_texture`].
    Own(ImageCacheKey),
}

/// A permanent map from glyph reference to atlas region, and a bounded one
/// from picture to atlas region.
///
/// No glyph or icon is ever evicted. Crook has no glyph-rendering
/// configuration to invalidate on, so the only thing that grows that part of
/// the cache is a genuinely new glyph at a genuinely new size. Pictures keep
/// a set of atlases of their own — never the glyphs' — that
/// [`Self::sweep_images`] empties whole once it has grown past its budget,
/// and the textures of the pictures too big for an atlas, which it lets go
/// one by one as they stop being drawn.
pub struct GlyphCache {
    textures: Vec<AtlasTexture>,
    cache: FxHashMap<GlyphCacheKey, Option<GlyphTextureOffset>>,
    icons: FxHashMap<IconCacheKey, Option<IconTextureOffset>>,
    atlas_manager: atlas::Manager,

    images: FxHashMap<ImageCacheKey, Option<ImageTextureOffset>>,
    image_textures: Vec<AtlasTexture>,
    image_atlas: atlas::Manager,
    own_textures: FxHashMap<ImageCacheKey, OwnTexture>,
}

/// A picture's own texture, and whether a frame has drawn it since the last
/// sweep.
struct OwnTexture {
    texture: AtlasTexture,
    region: atlas::AllocatedRegion,
    drawn: bool,
}

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
struct GlyphCacheKey {
    glyph_key: GlyphKey,

    /// The scale factor's bit pattern. Two scale factors that are not bitwise
    /// identical rasterize differently, which is exactly what a cache key wants
    /// and exactly what `f32` cannot express as `Eq`.
    scale_factor: u32,

    subpixel_alignment: SubpixelAlignment,
}

/// What one icon mask is cached under: a mark, a device size and a stroke.
///
/// Whole device pixels rather than a logical size and a scale factor, because
/// two windows on two monitors that arrive at the same physical size want the
/// same mask. There is no subpixel bucket: an icon is snapped to the pixel
/// grid on both axes, so there is only ever one alignment to rasterize.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
struct IconCacheKey {
    mark: Mark,
    pixels: u32,

    /// The stroke width's bit pattern, for the reason `scale_factor` above is
    /// one: an `f32` is not `Eq`.
    stroke_width: u32,
}

/// What one picture is cached under: which bitmap, at which device size.
///
/// Device pixels, as an icon's key is, and for the same reason. The bitmap's
/// identity rather than its bytes, because two pictures of the same size are
/// two pictures, and an id is minted once per decode and never reused. It is
/// also the name of a picture's own texture, which holds that one picture at
/// that one size.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct ImageCacheKey {
    id: ImageId,
    width: u32,
    height: u32,
}

impl GlyphCache {
    /// An empty cache with no atlas textures allocated yet.
    pub fn new() -> Self {
        Self {
            textures: Vec::new(),
            cache: FxHashMap::default(),
            icons: FxHashMap::default(),
            atlas_manager: atlas::Manager::new(ATLAS_SIZE),
            images: FxHashMap::default(),
            image_textures: Vec::new(),
            image_atlas: atlas::Manager::new(ATLAS_SIZE),
            own_textures: FxHashMap::default(),
        }
    }

    /// The glyph atlas texture `texture_id` refers to.
    pub fn texture(&self, texture_id: TextureId) -> Option<&AtlasTexture> {
        self.textures.get(texture_id.as_index())
    }

    /// The image atlas texture `texture_id` refers to.
    ///
    /// Image atlases are numbered from zero like glyph atlases, so an id means
    /// nothing without knowing which set it came from; the renderer keeps
    /// them apart by construction.
    pub fn image_texture(&self, texture_id: TextureId) -> Option<&AtlasTexture> {
        self.image_textures.get(texture_id.as_index())
    }

    /// The texture of a picture's own that `key` names, while it is kept.
    pub fn own_texture(&self, key: ImageCacheKey) -> Option<&AtlasTexture> {
        self.own_textures.get(&key).map(|own| &own.texture)
    }

    /// Where to find `glyph_key`, rasterizing and uploading it on a miss.
    ///
    /// `Ok(None)` means the glyph has no pixels — a space, a zero-width
    /// joiner — and should be skipped rather than drawn. That answer is cached
    /// too, so a line full of spaces asks the rasterizer once.
    pub fn get(
        &mut self,
        glyph_key: GlyphKey,
        scale_factor: f32,
        subpixel_alignment: SubpixelAlignment,
        font_db: &dyn FontDb,
        atlas: &AtlasContext<'_>,
    ) -> Result<Option<GlyphTextureOffset>> {
        let cache_key = GlyphCacheKey {
            glyph_key,
            scale_factor: scale_factor.to_bits(),
            subpixel_alignment,
        };

        if let Some(cached) = self.cache.get(&cache_key) {
            return Ok(*cached);
        }

        let scale = Vector2F::splat(scale_factor);
        let raster_bounds = font_db.glyph_raster_bounds(glyph_key, scale)?;
        if raster_bounds.is_empty() {
            self.cache.insert(cache_key, None);
            return Ok(None);
        }

        // Always RGBA, because one format means one atlas and one pipeline for
        // both coverage masks and color emoji. A mask's coverage byte must
        // reach all four channels; the shader reads it from red.
        let glyph =
            font_db.rasterize_glyph(glyph_key, scale, subpixel_alignment, RasterFormat::Rgba32)?;

        let offset = place(
            &mut self.textures,
            &mut self.atlas_manager,
            &glyph.canvas,
            atlas,
        )?;

        let placed = GlyphTextureOffset {
            texture_id: offset.texture_id,
            allocated_region: offset.allocated_region,
            raster_bounds,
            is_emoji: glyph.is_emoji,
        };
        self.cache.insert(cache_key, Some(placed));

        Ok(Some(placed))
    }

    /// Where to find an icon, rasterizing and uploading it on a miss.
    ///
    /// `Ok(None)` means the icon rounded to nothing — a zero-sized square —
    /// and should be skipped. Everything else about this mirrors
    /// [`Self::get`]: same atlas, same textures, same never-evicted map. The
    /// only thing an icon does differently is where its pixels come from, and
    /// that is one call.
    pub fn get_icon(
        &mut self,
        icon_key: IconKey,
        scale_factor: f32,
        atlas: &AtlasContext<'_>,
    ) -> Result<Option<IconTextureOffset>> {
        let pixels = (icon_key.size * scale_factor).round().max(0.) as u32;
        let cache_key = IconCacheKey {
            mark: icon_key.mark,
            pixels,
            stroke_width: icon_key.stroke_width.to_bits(),
        };

        if let Some(cached) = self.icons.get(&cache_key) {
            return Ok(*cached);
        }

        if pixels == 0 {
            self.icons.insert(cache_key, None);
            return Ok(None);
        }

        let mask = icons::rasterize(icon_key.mark, pixels, icon_key.stroke_width);
        let canvas = widen(&mask);

        let offset = place(&mut self.textures, &mut self.atlas_manager, &canvas, atlas)?;

        let placed = IconTextureOffset {
            texture_id: offset.texture_id,
            allocated_region: offset.allocated_region,
        };
        self.icons.insert(cache_key, Some(placed));

        Ok(Some(placed))
    }

    /// Where to find `bitmap` at `drawn` device pixels, resampling and
    /// uploading it on a miss.
    ///
    /// `Ok(None)` means the picture is drawn at no size at all and should be
    /// skipped, the way an icon squeezed to nothing is. The region is
    /// [`image_region`] of `drawn` and the bitmap's own size — the drawn size
    /// in an atlas, the bitmap's in a texture of its own — and the pixels are
    /// resampled on the CPU only when they are not that size, which in a
    /// texture of its own takes a bitmap longer than the GPU's largest
    /// texture. A picture already the drawn size is uploaded as it is, which
    /// is what a caller that decoded it at the size it draws it at is spared.
    pub fn get_image(
        &mut self,
        bitmap: &Bitmap,
        drawn: (u32, u32),
        atlas: &AtlasContext<'_>,
    ) -> Result<Option<ImageTextureOffset>> {
        let region = image_region(
            drawn,
            bitmap.size(),
            atlas.device.limits().max_texture_dimension_2d,
        );
        let (width, height) = region.size();
        let cache_key = ImageCacheKey {
            id: bitmap.id(),
            width,
            height,
        };

        if let Some(own) = self.own_textures.get_mut(&cache_key) {
            own.drawn = true;
            return Ok(Some(ImageTextureOffset {
                texture: ImageTexture::Own(cache_key),
                allocated_region: own.region,
            }));
        }
        if let Some(cached) = self.images.get(&cache_key) {
            return Ok(*cached);
        }

        if width == 0 || height == 0 {
            self.images.insert(cache_key, None);
            return Ok(None);
        }

        let resampled;
        let canvas = if bitmap.size() == (width, height) {
            bitmap.canvas()
        } else {
            resampled = resample(bitmap, width, height);
            resampled.canvas()
        };

        if let ImageRegion::Own(..) = region {
            let mut texture = AtlasTexture::sized(width, height, atlas);
            let allocated_region = atlas::AllocatedRegion {
                uv_region: RectF::new(Vector2F::zero(), Vector2F::splat(1.)),
                pixel_region: atlas::PixelRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
            };
            texture.insert(allocated_region, canvas, atlas.queue);
            self.own_textures.insert(
                cache_key,
                OwnTexture {
                    texture,
                    region: allocated_region,
                    drawn: true,
                },
            );
            return Ok(Some(ImageTextureOffset {
                texture: ImageTexture::Own(cache_key),
                allocated_region,
            }));
        }

        let offset = place(
            &mut self.image_textures,
            &mut self.image_atlas,
            canvas,
            atlas,
        )?;

        let placed = ImageTextureOffset {
            texture: ImageTexture::Atlas(offset.texture_id),
            allocated_region: offset.allocated_region,
        };
        self.images.insert(cache_key, Some(placed));

        Ok(Some(placed))
    }

    /// Lets go of every picture's own texture that the frame before did not
    /// draw, and drops every image atlas, and every entry into one, once there
    /// are more than `IMAGE_ATLASES` of them. Glyphs and icons are untouched.
    ///
    /// An own texture one at a time, because it is a whole texture and can be:
    /// it is a screenful of pixels, and one scrolled out of view is worth less
    /// than the one upload it takes to bring it back. The atlases whole,
    /// because the allocator never frees: a region cannot be given back, so
    /// the only way to reclaim an atlas is to start it over. Run at the top of
    /// a frame, before any layer is walked, so a picture placed for this frame
    /// is never the one dropped in it.
    pub fn sweep_images(&mut self) {
        self.own_textures
            .retain(|_, own| std::mem::take(&mut own.drawn));
        if self.image_textures.len() <= IMAGE_ATLASES {
            return;
        }
        self.image_textures.clear();
        self.images.clear();
        self.image_atlas = atlas::Manager::new(ATLAS_SIZE);
    }
}

/// Finds `canvas` a region in `manager`'s atlases and uploads it there,
/// minting the texture if the region landed in one that does not exist yet.
///
/// The one block every kind of pixels shares: a glyph, an icon and a picture
/// each arrive here with a canvas and leave with a region, and which set of
/// atlases they went into is the caller's choice of `textures` and `manager`.
fn place(
    textures: &mut Vec<AtlasTexture>,
    manager: &mut atlas::Manager,
    canvas: &Canvas,
    atlas: &AtlasContext<'_>,
) -> Result<TextureOffset> {
    let offset = manager.insert(canvas.size)?;
    let index = offset.texture_id.as_index();
    if index >= textures.len() {
        textures.resize_with(index + 1, || AtlasTexture::new(ATLAS_SIZE, atlas));
    }
    textures[index].insert(offset.allocated_region, canvas, atlas.queue);
    Ok(offset)
}

/// An A8 coverage mask as the RGBA the atlas is made of.
///
/// The coverage byte is replicated into all four channels rather than put in
/// alpha alone, because the glyph shader reads a mask's coverage out of *red*
/// — one texture and one pipeline serve masks and colour emoji alike, and the
/// shader tells them apart with a flag rather than a second sampler. The same
/// widening happens to a glyph inside the font backend; it happens here for an
/// icon because here is where an icon becomes pixels.
fn widen(mask: &Canvas) -> Canvas {
    let (width, height) = mask.size;
    let row_stride = width as usize * RasterFormat::Rgba32.bytes_per_pixel() as usize;
    let mut pixels = vec![0u8; row_stride * height as usize];

    for row in 0..height as usize {
        let source = &mask.pixels[row * mask.row_stride..][..width as usize];
        let target = &mut pixels[row * row_stride..][..row_stride];
        for (pixel, coverage) in target.chunks_exact_mut(4).zip(source) {
            pixel.fill(*coverage);
        }
    }

    Canvas {
        pixels,
        size: mask.size,
        row_stride,
        format: RasterFormat::Rgba32,
    }
}

#[cfg(test)]
mod tests {
    use crookui_core::icons::Lucide;
    use std::cell::Cell;

    use anyhow::bail;
    use crookui_core::fonts::{Canvas, FamilyId, FontId, GlyphId, Metrics, RasterizedGlyph};

    use super::super::resources::Resources;
    use super::*;

    /// A rasterizer that hands back a solid square of the size it was built
    /// with, and counts how many times it was asked.
    ///
    /// The count is the point: a cache that answers from its map never reaches
    /// this, so an unchanged counter is what proves the second lookup was a
    /// hit rather than a second rasterization that happened to land in the same
    /// place.
    struct CountingGlyphs {
        size: u32,
        rasterizations: Cell<usize>,
    }

    impl CountingGlyphs {
        fn new(size: u32) -> Self {
            Self {
                size,
                rasterizations: Cell::new(0),
            }
        }
    }

    impl FontDb for CountingGlyphs {
        fn load_family_from_bytes(&mut self, name: &str, _: Vec<Vec<u8>>) -> Result<FamilyId> {
            bail!("CountingGlyphs has no families to load, but {name} was requested")
        }

        fn font_metrics(&self, _: FontId) -> Metrics {
            Metrics {
                units_per_em: 1000,
                ascent: 800,
                descent: -200,
                line_gap: 0,
            }
        }

        fn glyph_advance(&self, _: FontId, _: GlyphId) -> Result<Vector2F> {
            Ok(Vector2F::new(self.size as f32, 0.))
        }

        fn glyph_raster_bounds(&self, glyph_key: GlyphKey, _: Vector2F) -> Result<RasterBounds> {
            // Glyph 0 stands in for a space: no pixels, so no atlas region.
            let size = if glyph_key.glyph_id == 0 {
                0
            } else {
                self.size
            };

            Ok(RasterBounds {
                left: 0,
                top: size as i32,
                width: size,
                height: size,
            })
        }

        fn rasterize_glyph(
            &self,
            _: GlyphKey,
            _: Vector2F,
            _: SubpixelAlignment,
            _: RasterFormat,
        ) -> Result<RasterizedGlyph> {
            self.rasterizations.set(self.rasterizations.get() + 1);

            Ok(RasterizedGlyph {
                canvas: Canvas {
                    pixels: vec![255; (self.size * self.size * 4) as usize],
                    size: (self.size, self.size),
                    row_stride: (self.size * 4) as usize,
                    format: RasterFormat::Rgba32,
                },
                is_emoji: false,
            })
        }
    }

    /// A device plus the bind-group layout and sampler an [`AtlasContext`]
    /// needs, or `None` on a machine with no usable adapter.
    ///
    /// The layout is built here rather than borrowed from the glyph pipeline
    /// because nothing in these tests draws: the bind groups only have to be
    /// creatable, not renderable.
    fn device() -> Option<(Resources, wgpu::BindGroupLayout, wgpu::Sampler)> {
        let resources = Resources::new(None).ok()?;

        let layout = resources
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Test atlas bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let sampler = resources
            .device
            .create_sampler(&wgpu::SamplerDescriptor::default());

        Some((resources, layout, sampler))
    }

    fn key(glyph_id: GlyphId) -> GlyphKey {
        GlyphKey {
            glyph_id,
            font_id: FontId(0),
            font_size: 14.,
        }
    }

    /// A `width` × `height` picture of one opaque colour.
    fn picture(width: u32, height: u32) -> Bitmap {
        let pixels = [40, 80, 120, 255].repeat((width * height) as usize);
        Bitmap::rgba8(width, height, pixels).expect("a solid is a bitmap")
    }

    #[test]
    fn a_picture_is_placed_once_in_an_atlas_of_its_own_at_the_size_it_is_drawn() {
        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the image cache test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let mut cache = GlyphCache::new();
        let bitmap = picture(4, 4);

        let first = cache
            .get_image(&bitmap, (12, 12), &atlas)
            .unwrap()
            .expect("a picture drawn at a size has pixels");
        let second = cache
            .get_image(&bitmap, (12, 12), &atlas)
            .unwrap()
            .expect("a picture drawn at a size has pixels");

        assert_eq!(
            first.allocated_region.pixel_region, second.allocated_region.pixel_region,
            "the same picture at the same size is one region"
        );
        assert_eq!(
            (
                first.allocated_region.pixel_region.width,
                first.allocated_region.pixel_region.height
            ),
            (12, 12),
            "resampled to the drawn size, not uploaded at its own"
        );
        let ImageTexture::Atlas(texture_id) = first.texture else {
            panic!("a picture this small shares an atlas: {first:?}");
        };
        assert!(
            cache.image_texture(texture_id).is_some(),
            "the region is in an image atlas"
        );
        assert!(
            cache.texture(texture_id).is_none(),
            "and no glyph atlas was opened for it"
        );

        let other = cache
            .get_image(&bitmap, (24, 24), &atlas)
            .unwrap()
            .expect("a picture drawn at a size has pixels");
        assert_ne!(
            other.allocated_region.pixel_region, first.allocated_region.pixel_region,
            "another device size is another region"
        );
    }

    #[test]
    fn a_picture_drawn_at_no_size_is_cached_as_nothing() {
        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the image cache test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let mut cache = GlyphCache::new();
        let bitmap = picture(4, 4);

        assert!(cache.get_image(&bitmap, (0, 4), &atlas).unwrap().is_none());
        assert!(cache.get_image(&bitmap, (0, 4), &atlas).unwrap().is_none());
        assert!(
            cache.image_texture(TextureId::initial()).is_none(),
            "nothing was uploaded for a picture with no size"
        );
    }

    #[test]
    fn a_picture_past_an_atlas_row_is_given_a_texture_of_its_own_at_its_drawn_size() {
        // A 1120-pixel picture drawn 560 logical pixels wide on a 2x
        // display, and a 2000-pixel screenshot drawn 1000 wide on one: both
        // used to be squeezed to the 1022 an atlas row takes and stretched
        // back up, soft.
        let own = |drawn| image_region(drawn, drawn, 8192);
        assert_eq!(own((1120, 700)), ImageRegion::Own(1120, 700));
        assert_eq!(own((2000, 1000)), ImageRegion::Own(2000, 1000));
        assert_eq!(
            own((1022, 1022)),
            ImageRegion::Atlas(1022, 1022),
            "one that fits a row still shares an atlas"
        );
        assert_eq!(own((40, 20)), ImageRegion::Atlas(40, 20));

        // Only a window wider than the GPU's largest texture is stretched.
        assert_eq!(own((16384, 4096)), ImageRegion::Own(8192, 2048));
        assert_eq!(
            own((30000, 1)),
            ImageRegion::Own(8192, 1),
            "a side never rounds away"
        );
        assert_eq!(
            image_region((30000, 0), (30000, 4), 8192),
            ImageRegion::Own(8192, 0),
            "unless it was drawn as nothing"
        );

        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the own texture test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let mut cache = GlyphCache::new();
        let screenshot = picture(2000, 1000);
        let placed = cache
            .get_image(&screenshot, (2000, 1000), &atlas)
            .unwrap()
            .expect("a picture drawn at a size has pixels");

        assert_eq!(
            (
                placed.allocated_region.pixel_region.width,
                placed.allocated_region.pixel_region.height
            ),
            (2000, 1000),
            "uploaded at the size it is drawn, not at the 1022 of an atlas row"
        );
        let ImageTexture::Own(key) = placed.texture else {
            panic!("placed in an atlas: {placed:?}");
        };
        assert!(cache.own_texture(key).is_some());
        assert!(
            cache.image_texture(TextureId::initial()).is_none(),
            "and no atlas was opened for it"
        );
        assert_eq!(
            placed.allocated_region.uv_region,
            RectF::new(Vector2F::zero(), Vector2F::splat(1.)),
            "the whole of its texture, which is the whole of the picture"
        );

        let again = cache
            .get_image(&screenshot, (2000, 1000), &atlas)
            .unwrap()
            .expect("a picture drawn at a size has pixels");
        assert_eq!(again.texture, placed.texture, "and it is uploaded once");
    }

    #[test]
    fn pixels_handed_over_at_another_size_past_an_atlas_row_are_stretched_and_not_resampled() {
        // A picture is decoded again when a font step changes the size it is
        // drawn at, and drawn from its old pixels until then. Resampled to
        // the new size here, a screenshot across a Retina pane was hundreds
        // of milliseconds on the thread that draws, at every step, for every
        // picture in view. Past an atlas row the pixels go up as they are
        // and the sampler stretches them, either way.
        assert_eq!(
            image_region((3024, 1890), (2800, 1750), 8192),
            ImageRegion::Own(2800, 1750),
            "grown a step"
        );
        assert_eq!(
            image_region((2600, 1625), (2800, 1750), 8192),
            ImageRegion::Own(2800, 1750),
            "shrunk a step"
        );
        assert_eq!(
            image_region((1120, 700), (1024, 640), 8192),
            ImageRegion::Own(1024, 640),
            "a 1024-pixel preview on a card 560 logical pixels wide at 2x"
        );
        assert_eq!(
            image_region((9000, 4500), (12000, 6000), 8192),
            ImageRegion::Own(8192, 4096),
            "and past the GPU's largest texture they are shrunk to it"
        );

        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the stretched texture test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let mut cache = GlyphCache::new();
        let pixels = picture(1200, 600);
        let mut textures = Vec::new();
        for drawn in [(1200, 600), (1296, 648), (1104, 552)] {
            let placed = cache
                .get_image(&pixels, drawn, &atlas)
                .unwrap()
                .expect("a picture drawn at a size has pixels");
            let region = placed.allocated_region.pixel_region;
            assert_eq!(
                (region.width, region.height),
                (1200, 600),
                "drawn at {drawn:?}, uploaded at its own pixels"
            );
            textures.push(placed.texture);
        }
        assert!(
            textures.iter().all(|texture| *texture == textures[0]),
            "one upload stretched to every size, not one resample a size: {textures:?}"
        );
    }

    #[test]
    fn pixels_bigger_than_an_atlas_drawn_within_a_row_are_shrunk_by_the_sampler_and_not_resampled()
    {
        // A picture zoomed in until it was decoded at 4000 pixels square, and
        // the font put back: drawn at 1000 until the decode at that size
        // lands, from the 4000-pixel square. Resampled into an atlas row, that
        // was sixteen million pixels averaged on the thread that draws, since
        // a resample costs what its source weighs; and three screenshots
        // dragged from a 2x display to a 1x one were three of them at once.
        assert_eq!(
            image_region((1000, 1000), (4000, 4000), 8192),
            ImageRegion::Own(4000, 4000)
        );
        assert_eq!(
            image_region((800, 500), (2800, 1750), 8192),
            ImageRegion::Own(2800, 1750)
        );
        assert_eq!(
            image_region((560, 350), (1024, 640), 8192),
            ImageRegion::Atlas(560, 350),
            "a preview no bigger than an atlas, drawn small, is still resampled into one"
        );

        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the shrunk texture test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let mut cache = GlyphCache::new();
        let pixels = picture(1200, 600);
        cache.sweep_images();
        let zoomed = cache
            .get_image(&pixels, (1200, 600), &atlas)
            .unwrap()
            .expect("a picture drawn at a size has pixels");

        // The next frame draws it within an atlas row.
        cache.sweep_images();
        let put_back = cache
            .get_image(&pixels, (600, 300), &atlas)
            .unwrap()
            .expect("a picture drawn at a size has pixels");
        assert_eq!(
            put_back.texture, zoomed.texture,
            "drawn from the texture the frame before gave it, uploaded once"
        );
        let region = put_back.allocated_region.pixel_region;
        assert_eq!((region.width, region.height), (1200, 600));
        assert!(
            cache.image_texture(TextureId::initial()).is_none(),
            "resampled into an atlas row"
        );
    }

    #[test]
    fn a_picture_with_a_texture_of_its_own_keeps_it_exactly_while_it_is_drawn() {
        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the own texture sweep test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let mut cache = GlyphCache::new();
        let shown = picture(2, 2);
        let scrolled_away = picture(2, 2);
        let place = |cache: &mut GlyphCache, bitmap: &Bitmap| {
            let placed = cache
                .get_image(bitmap, (1500, 900), &atlas)
                .unwrap()
                .expect("a picture drawn at a size has pixels");
            let ImageTexture::Own(key) = placed.texture else {
                panic!("placed in an atlas: {placed:?}");
            };
            key
        };

        // Frame one draws both.
        cache.sweep_images();
        let kept = place(&mut cache, &shown);
        let let_go = place(&mut cache, &scrolled_away);

        // Frame two draws one of them.
        cache.sweep_images();
        assert!(cache.own_texture(kept).is_some());
        assert!(
            cache.own_texture(let_go).is_some(),
            "both were drawn in the frame before"
        );
        place(&mut cache, &shown);

        // Frame three lets the other go.
        cache.sweep_images();
        assert!(
            cache.own_texture(kept).is_some(),
            "a picture still drawn keeps its texture"
        );
        assert!(
            cache.own_texture(let_go).is_none(),
            "a screenful of pixels nobody drew in the last frame is given back"
        );
    }

    #[test]
    fn a_ninth_image_atlas_is_swept_away_and_the_glyphs_never_notice() {
        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the image sweep test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let font_db = CountingGlyphs::new(24);
        let mut cache = GlyphCache::new();
        let alignment = SubpixelAlignment::new(Vector2F::zero());
        let glyph = cache
            .get(key(1), 1., alignment, &font_db, &atlas)
            .unwrap()
            .expect("glyph 1 has pixels");
        let icon = cache
            .get_icon(IconKey::new(Lucide::X, 16.), 1., &atlas)
            .unwrap()
            .expect("an icon with a size has pixels");

        // 600 square: two do not share a row, and a second row has no room
        // under the first, so every one opens an atlas of its own.
        let pictures: Vec<_> = (0..IMAGE_ATLASES + 1).map(|_| picture(2, 2)).collect();
        let mut expected = TextureId::initial();
        for bitmap in &pictures {
            let placed = cache
                .get_image(bitmap, (600, 600), &atlas)
                .unwrap()
                .expect("a picture drawn at a size has pixels");
            assert_eq!(placed.texture, ImageTexture::Atlas(expected));
            expected = expected.next();
        }

        cache.sweep_images();

        assert!(
            cache.image_texture(TextureId::initial()).is_none(),
            "past the budget, every image atlas goes"
        );
        let again = cache
            .get_image(&pictures[0], (600, 600), &atlas)
            .unwrap()
            .expect("a picture drawn at a size has pixels");
        assert_eq!(
            again.texture,
            ImageTexture::Atlas(TextureId::initial()),
            "and the next picture starts the set over"
        );
        cache.sweep_images();
        assert!(
            cache.image_texture(TextureId::initial()).is_some(),
            "one atlas is within the budget and stays"
        );

        let glyph_again = cache
            .get(key(1), 1., alignment, &font_db, &atlas)
            .unwrap()
            .expect("glyph 1 has pixels");
        let icon_again = cache
            .get_icon(IconKey::new(Lucide::X, 16.), 1., &atlas)
            .unwrap()
            .expect("an icon with a size has pixels");
        assert_eq!(
            glyph_again.allocated_region.pixel_region,
            glyph.allocated_region.pixel_region
        );
        assert_eq!(
            icon_again.allocated_region.pixel_region,
            icon.allocated_region.pixel_region
        );
        assert_eq!(
            font_db.rasterizations.get(),
            1,
            "the sweep never reached the glyph atlas"
        );
    }

    #[test]
    fn an_icon_shares_the_atlas_with_the_glyphs_and_is_cached_beside_them() {
        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the icon cache test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let font_db = CountingGlyphs::new(24);
        let mut cache = GlyphCache::new();
        let glyph = cache
            .get(
                key(1),
                2.,
                SubpixelAlignment::new(Vector2F::zero()),
                &font_db,
                &atlas,
            )
            .unwrap()
            .expect("glyph 1 has pixels");

        let icon_key = IconKey::new(Lucide::X, 16.);
        let first = cache
            .get_icon(icon_key, 2., &atlas)
            .unwrap()
            .expect("an icon with a size has pixels");
        let second = cache
            .get_icon(icon_key, 2., &atlas)
            .unwrap()
            .expect("an icon with a size has pixels");

        assert_eq!(
            first.texture_id, glyph.texture_id,
            "an icon is a mask like any other and belongs in the same atlas"
        );
        assert_ne!(
            first.allocated_region.pixel_region.x, glyph.allocated_region.pixel_region.x,
            "and in a region of its own"
        );
        assert_eq!(
            first.allocated_region.pixel_region, second.allocated_region.pixel_region,
            "the second lookup rasterized the icon again instead of reading the cache"
        );
        assert_eq!(
            first.allocated_region.pixel_region.width, 32,
            "a 16px icon on a 2x scale is a 32px mask"
        );
    }

    #[test]
    fn an_icon_with_no_pixels_in_it_is_cached_as_nothing() {
        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the icon cache test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let mut cache = GlyphCache::new();
        let squeezed = IconKey::new(Lucide::Check, 0.2);

        // A quarter of a pixel rounds to none, and an atlas region of zero
        // width is an error rather than an empty draw — so the answer has to
        // be "skip this one", the way a space is skipped.
        assert!(cache.get_icon(squeezed, 1., &atlas).unwrap().is_none());
        assert!(cache.get_icon(squeezed, 1., &atlas).unwrap().is_none());
    }

    #[test]
    fn a_repeated_key_comes_back_from_the_cache_unchanged() {
        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the glyph cache test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let font_db = CountingGlyphs::new(24);
        let mut cache = GlyphCache::new();
        let alignment = SubpixelAlignment::new(Vector2F::zero());

        let first = cache
            .get(key(1), 2., alignment, &font_db, &atlas)
            .unwrap()
            .expect("glyph 1 has pixels");
        let second = cache
            .get(key(1), 2., alignment, &font_db, &atlas)
            .unwrap()
            .expect("glyph 1 has pixels");

        assert_eq!(first.texture_id, second.texture_id);
        assert_eq!(
            first.allocated_region.pixel_region,
            second.allocated_region.pixel_region
        );
        assert_eq!(
            font_db.rasterizations.get(),
            1,
            "the second lookup should never have reached the rasterizer"
        );

        // A different glyph is a different key, so it gets its own region in
        // the same atlas rather than the first one's.
        let other = cache
            .get(key(2), 2., alignment, &font_db, &atlas)
            .unwrap()
            .expect("glyph 2 has pixels");
        assert_eq!(other.texture_id, first.texture_id);
        assert_ne!(
            other.allocated_region.pixel_region,
            first.allocated_region.pixel_region
        );
    }

    #[test]
    fn the_scale_factor_and_the_subpixel_bucket_are_both_part_of_the_key() {
        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the glyph cache key test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let font_db = CountingGlyphs::new(24);
        let mut cache = GlyphCache::new();

        for (scale, x) in [(1., 0.), (2., 0.), (2., 0.5)] {
            cache
                .get(
                    key(1),
                    scale,
                    SubpixelAlignment::new(Vector2F::new(x, 0.)),
                    &font_db,
                    &atlas,
                )
                .unwrap();
        }

        assert_eq!(font_db.rasterizations.get(), 3);
    }

    #[test]
    fn filling_an_atlas_rolls_the_next_glyph_into_a_new_texture() {
        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the atlas rollover test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        // Two of these fit in neither one row nor two of a 1024px atlas, so the
        // second one has to open a texture of its own.
        let font_db = CountingGlyphs::new(600);
        let mut cache = GlyphCache::new();
        let alignment = SubpixelAlignment::new(Vector2F::zero());

        let first = cache
            .get(key(1), 1., alignment, &font_db, &atlas)
            .unwrap()
            .expect("glyph 1 has pixels");
        let second = cache
            .get(key(2), 1., alignment, &font_db, &atlas)
            .unwrap()
            .expect("glyph 2 has pixels");

        assert_eq!(first.texture_id, atlas::TextureId::initial());
        assert_eq!(second.texture_id, atlas::TextureId::initial().next());
        assert!(cache.texture(second.texture_id).is_some());
    }

    #[test]
    fn a_glyph_with_no_pixels_answers_none_once_and_from_then_on_for_free() {
        let Some((resources, layout, sampler)) = device() else {
            log::warn!("skipping the empty glyph test: no usable GPU adapter");
            return;
        };
        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &layout,
            sampler: &sampler,
        };

        let font_db = CountingGlyphs::new(24);
        let mut cache = GlyphCache::new();
        let alignment = SubpixelAlignment::new(Vector2F::zero());

        assert!(
            cache
                .get(key(0), 1., alignment, &font_db, &atlas)
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .get(key(0), 1., alignment, &font_db, &atlas)
                .unwrap()
                .is_none()
        );
        assert_eq!(font_db.rasterizations.get(), 0);
    }
}
