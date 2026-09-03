//! Rasterized glyphs, and where each one landed in an atlas texture.
//!
//! The scene stores glyph *references* — a [`GlyphKey`] and a position — never
//! pixels. This is where a reference becomes a rectangle of an atlas: on a
//! miss the glyph is rasterized through [`FontDb`], allocated a region, and
//! uploaded; on a hit nothing touches the font backend or the GPU at all.
//!
//! Ported from Warp's `crates/warpui/src/rendering/glyph_cache.rs` (MIT). Warp
//! parameterizes the cache over four `dyn Fn` callbacks so one implementation
//! can serve both its Metal and its wgpu renderer. Crook has one renderer, so
//! the callbacks collapse into a `&dyn FontDb` and an [`AtlasContext`].

use anyhow::Result;
use crookui_core::fonts::{Canvas, GlyphKey, RasterBounds, RasterFormat, SubpixelAlignment};
use crookui_core::geometry::Vector2F;
use crookui_core::icons::{self, IconKey, Lucide};
use crookui_core::platform::FontDb;
use rustc_hash::FxHashMap;

use super::atlas::{self, TextureId};
use super::glyph::{AtlasContext, AtlasTexture};

/// The edge length of one atlas texture, in pixels.
///
/// 1024² of RGBA8 is 4 MB. Masks are stored as RGBA too — the shader reads
/// coverage out of the red channel — so this fills faster than it looks: one
/// entry per glyph, per size, per scale factor, per subpixel bucket.
const ATLAS_SIZE: u32 = 1024;

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

/// A permanent map from glyph reference to atlas region.
///
/// Nothing is ever evicted. Crook has no glyph-rendering configuration to
/// invalidate on, so the only thing that grows the cache is a genuinely new
/// glyph at a genuinely new size.
pub struct GlyphCache {
    textures: Vec<AtlasTexture>,
    cache: FxHashMap<GlyphCacheKey, Option<GlyphTextureOffset>>,
    icons: FxHashMap<IconCacheKey, Option<IconTextureOffset>>,
    atlas_manager: atlas::Manager,
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

/// What one icon mask is cached under: an icon, a device size and a stroke.
///
/// Whole device pixels rather than a logical size and a scale factor, because
/// two windows on two monitors that arrive at the same physical size want the
/// same mask. There is no subpixel bucket: an icon is snapped to the pixel
/// grid on both axes, so there is only ever one alignment to rasterize.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
struct IconCacheKey {
    icon: Lucide,
    pixels: u32,

    /// The stroke width's bit pattern, for the reason `scale_factor` above is
    /// one: an `f32` is not `Eq`.
    stroke_width: u32,
}

impl GlyphCache {
    /// An empty cache with no atlas textures allocated yet.
    pub fn new() -> Self {
        Self {
            textures: Vec::new(),
            cache: FxHashMap::default(),
            icons: FxHashMap::default(),
            atlas_manager: atlas::Manager::new(ATLAS_SIZE),
        }
    }

    /// The atlas texture `texture_id` refers to.
    pub fn texture(&self, texture_id: TextureId) -> Option<&AtlasTexture> {
        self.textures.get(texture_id.as_index())
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

        let offset = self.atlas_manager.insert(glyph.canvas.size)?;
        let index = offset.texture_id.as_index();
        if index >= self.textures.len() {
            self.textures
                .resize_with(index + 1, || AtlasTexture::new(ATLAS_SIZE, atlas));
        }
        self.textures[index].insert(offset.allocated_region, &glyph.canvas, atlas.queue);

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
            icon: icon_key.icon,
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

        let mask = icons::rasterize(icon_key.icon, pixels, icon_key.stroke_width);
        let canvas = widen(&mask);

        let offset = self.atlas_manager.insert(canvas.size)?;
        let index = offset.texture_id.as_index();
        if index >= self.textures.len() {
            self.textures
                .resize_with(index + 1, || AtlasTexture::new(ATLAS_SIZE, atlas));
        }
        self.textures[index].insert(offset.allocated_region, &canvas, atlas.queue);

        let placed = IconTextureOffset {
            texture_id: offset.texture_id,
            allocated_region: offset.allocated_region,
        };
        self.icons.insert(cache_key, Some(placed));

        Ok(Some(placed))
    }
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
