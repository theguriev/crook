//! Crook's font backend: font discovery, face selection and glyph rasterization.
//!
//! This module implements [`crookui_core::platform::FontDb`], and its sibling
//! `text_layout` implements [`crookui_core::platform::TextLayoutSystem`].
//! Together they are the only code in Crook that knows a font file exists.
//!
//! # One backend, not three
//!
//! Warp declares `FontDB` as a trait object because it has three
//! implementations behind it: CoreText on macOS, `font-kit` over `fontconfig`
//! on Linux, and DirectWrite on Windows — about 2,500 lines, a C library to
//! link, and a `[patch]`ed fork of `font-kit` to keep them all agreeing.
//!
//! Crook has one implementation, so this is a concrete struct and there is no
//! vtable. The reason it can be one implementation is a single call:
//! [`cosmic_text::FontSystem::new`] runs `fontdb`'s pure-Rust system font scan,
//! which reads `/System/Library/Fonts` and friends on macOS, parses
//! fontconfig's *configuration files* and scans the directories they name on
//! Linux, and reads `%WINDIR%\Fonts` on Windows. No system library is linked on
//! any of the three, which is why Crook builds with no `pkg-config` and no
//! build script. The traits stay traits so a test double or a headless shaper
//! can be dropped in; the vtable Warp needs for platforms is what is gone.
//!
//! # Two objects over one store
//!
//! [`CosmicFontDb`] rasterizes and is deliberately not `Send`/`Sync`;
//! [`CosmicTextLayout`] shapes and must be both, because shaping is the
//! expensive half of laying out text and belongs off the render thread. They
//! are separate objects sharing one `FontStore` behind an [`Arc`], so the
//! font database is discovered, parsed and memoized exactly once.
//! [`CosmicFontDb::text_layout`] hands out the shaper.
//!
//! # What is cached here and what is not
//!
//! Face selection, font metrics, glyph advances and glyph raster bounds are
//! memoized in the store. The first three because they are pure functions of a
//! face that get asked once per run and once per grid cell; the fourth because
//! answering it costs three rasterizations, for reasons `swash_rasterizer`
//! explains.
//!
//! Glyph *pixels* are not memoized. The renderer's atlas is already the cache
//! for those: every rasterized glyph it asks for is one it is about to upload
//! and never ask for again, and a second copy of every bitmap in memory buys
//! nothing.

mod str_index_map;
mod swash_rasterizer;
mod text_layout;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use cosmic_text::{
    Align, Attrs, AttrsList, FontSystem, Hinting, ShapeLine, Shaping, SwashCache, Wrap, fontdb,
};
use crookui_core::fonts::{
    FamilyId, FontId, GlyphId, GlyphKey, Metrics, Properties, RasterBounds, RasterFormat,
    RasterizedGlyph, Style, SubpixelAlignment, Weight,
};
use crookui_core::geometry::{Vector2F, vec2f};
use crookui_core::platform;
use parking_lot::{Mutex, RwLock};
use rustc_hash::FxHashMap;

pub use text_layout::CosmicTextLayout;

/// Families to try for UI text, most preferred first.
///
/// Every entry is a family that ships with a default install of the platform,
/// so the first is almost always the one taken. When none of them is present —
/// a minimal container image, a stripped embedded system — resolution falls
/// through to the database's generic sans-serif family and then to any face
/// that can draw Latin text, so a missing preferred font degrades and only a
/// genuinely font-less machine errors.
#[cfg(target_os = "macos")]
pub const DEFAULT_UI_FAMILIES: &[&str] = &[
    "SF Pro Text",
    "SF Pro",
    ".AppleSystemUIFont",
    "Helvetica Neue",
    "Helvetica",
    "Arial",
];

/// Families to try for UI text, most preferred first. See the macOS list.
#[cfg(target_os = "windows")]
pub const DEFAULT_UI_FAMILIES: &[&str] = &[
    "Segoe UI Variable Text",
    "Segoe UI",
    "Tahoma",
    "Verdana",
    "Arial",
];

/// Families to try for UI text, most preferred first. See the macOS list.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const DEFAULT_UI_FAMILIES: &[&str] = &[
    "Inter",
    "Cantarell",
    "Ubuntu",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
    "FreeSans",
];

/// Families to try for terminal and code text, most preferred first.
#[cfg(target_os = "macos")]
pub const DEFAULT_MONOSPACE_FAMILIES: &[&str] =
    &["SF Mono", "Menlo", "Monaco", "Andale Mono", "Courier New"];

/// Families to try for terminal and code text, most preferred first.
#[cfg(target_os = "windows")]
pub const DEFAULT_MONOSPACE_FAMILIES: &[&str] =
    &["Cascadia Mono", "Consolas", "Lucida Console", "Courier New"];

/// Families to try for terminal and code text, most preferred first.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const DEFAULT_MONOSPACE_FAMILIES: &[&str] = &[
    "JetBrains Mono",
    "Fira Mono",
    "Ubuntu Mono",
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "FreeMono",
];

/// The character every family is validated against.
///
/// Warp checks the same one at four separate load sites, and its `em_width`
/// then `.expect()`s that the check happened. Crook keeps the check — a family
/// with no `m` cannot size a UI — and returns errors instead of expecting.
const VALIDATION_CHAR: char = 'm';

/// Tab stops to assume when a [`LineStyle`] does not fix one, matching Warp.
///
/// [`LineStyle`]: crookui_core::fonts::LineStyle
const DEFAULT_TAB_WIDTH: u16 = 4;

/// Metrics reported for a face that cannot be read at all.
///
/// [`platform::FontDb::font_metrics`] cannot fail, and every caller divides by
/// `units_per_em`, so a broken face has to produce arithmetic that terminates.
/// These are the proportions of a typical text face, which makes the result
/// wrong but laid out rather than wrong and infinite.
const FALLBACK_METRICS: Metrics = Metrics {
    units_per_em: 1000,
    ascent: 800,
    descent: -200,
    line_gap: 0,
};

/// One registered face: what `fontdb` calls it, and the weight to render it at.
///
/// The weight is carried rather than looked up because it has to match the
/// weight shaping used. cosmic-text keys its swash scaler on
/// `(face, weight, size, subpixel bin)` and applies the weight as a `wght`
/// variation axis, so rasterizing a variable face at a weight the shaper did
/// not use produces glyphs of a different thickness than were measured.
#[derive(Copy, Clone, Debug)]
struct Face {
    id: fontdb::ID,
    weight: fontdb::Weight,
}

/// A registered family: the name `fontdb` knows it by, and its faces.
struct Family {
    /// The family name as the database spells it, which is what face selection
    /// and shaping must be asked for. Not necessarily what the caller asked to
    /// register it as.
    name: String,

    /// Non-empty by construction; the first entry is the fallback face when
    /// selection matches nothing.
    font_ids: Vec<FontId>,
}

/// Everything both halves of the backend share, and the identity maps over it.
///
/// Held behind an [`Arc`] by [`CosmicFontDb`] and [`CosmicTextLayout`].
///
/// Locking rule: **never hold two of these locks at once.** Every method takes
/// one, does its work, and drops it before taking another. The cost is that a
/// check-then-insert can race and do its work twice; since every value here is
/// a pure function of a face, the loser of that race writes the same bytes the
/// winner did.
struct FontStore {
    font_system: RwLock<FontSystem>,
    registry: RwLock<Registry>,
    metrics: RwLock<FxHashMap<FontId, Metrics>>,
    advances: RwLock<FxHashMap<(FontId, GlyphId), Vector2F>>,

    /// Keyed by glyph and by the bit patterns of both scale components, which
    /// is what makes a float a usable map key.
    raster_bounds: RwLock<FxHashMap<(GlyphKey, u32, u32), RasterBounds>>,
}

/// The identity maps: ours to `fontdb`'s and back, plus resolved selections.
#[derive(Default)]
struct Registry {
    face_by_font: FxHashMap<FontId, Face>,
    font_by_face: FxHashMap<fontdb::ID, FontId>,
    families: FxHashMap<FamilyId, Family>,
    family_by_name: FxHashMap<String, FamilyId>,
    selections: FxHashMap<(FamilyId, Properties), FontId>,
}

impl FontStore {
    fn new(font_system: FontSystem) -> Self {
        Self {
            font_system: RwLock::new(font_system),
            registry: RwLock::new(Registry::default()),
            metrics: RwLock::new(FxHashMap::default()),
            advances: RwLock::new(FxHashMap::default()),
            raster_bounds: RwLock::new(FxHashMap::default()),
        }
    }

    /// The [`FontId`] standing for a `fontdb` face, minting one if this is the
    /// first time the face has been seen.
    ///
    /// Minting on demand is what lets shaping-time font fallback work. Warp
    /// enumerates every installed face up front and `.expect()`s that any face
    /// cosmic-text picked is already in its map; Crook registers only the
    /// families it was asked for, so a face cosmic-text reaches for to cover a
    /// character the requested family lacks arrives here unknown, and gets a
    /// name rather than a panic.
    fn font_id_for_face(&self, face_id: fontdb::ID) -> FontId {
        if let Some(font_id) = self.registry.read().font_by_face.get(&face_id) {
            return *font_id;
        }

        let weight = self
            .font_system
            .read()
            .db()
            .face(face_id)
            .map(|face| face.weight)
            .unwrap_or_default();

        let mut registry = self.registry.write();
        if let Some(font_id) = registry.font_by_face.get(&face_id) {
            return *font_id;
        }

        let font_id = FontId::new();
        registry.font_by_face.insert(face_id, font_id);
        registry.face_by_font.insert(
            font_id,
            Face {
                id: face_id,
                weight,
            },
        );
        font_id
    }

    fn face(&self, font_id: FontId) -> Result<Face> {
        self.registry
            .read()
            .face_by_font
            .get(&font_id)
            .copied()
            .with_context(|| format!("{font_id:?} was never registered with this font backend"))
    }

    /// Runs `read` against a parsed view of a face.
    ///
    /// The parsed face is cached inside [`FontSystem`], so only the first call
    /// per face memory-maps and parses; the rest are a hash lookup. That is
    /// also why this needs the write lock for a read-only operation.
    fn with_font_ref<T>(
        &self,
        font_id: FontId,
        read: impl FnOnce(swash::FontRef<'_>) -> T,
    ) -> Result<T> {
        let face = self.face(font_id)?;
        let font = self
            .font_system
            .write()
            .get_font(face.id, face.weight)
            .with_context(|| format!("{font_id:?} could not be parsed as a font"))?;

        Ok(read(font.as_swash()))
    }

    /// Resolves a family and a weight/style to the concrete face that best
    /// matches, or `None` when the family was never registered.
    ///
    /// The match is a best effort, not an equality: ask a family that ships
    /// only Regular and Bold for Medium and you get Regular, recorded under the
    /// Medium key. That is what CSS font matching does and what every caller
    /// wants; it is worth knowing before debugging why a weight looks wrong.
    fn select_font(&self, family_id: FamilyId, properties: Properties) -> Option<FontId> {
        if let Some(font_id) = self
            .registry
            .read()
            .selections
            .get(&(family_id, properties))
        {
            return Some(*font_id);
        }

        let (name, first_font_id) = {
            let registry = self.registry.read();
            let family = registry.families.get(&family_id)?;
            (family.name.clone(), *family.font_ids.first()?)
        };

        let query = fontdb::Query {
            families: &[fontdb::Family::Name(&name)],
            weight: to_fontdb_weight(properties.weight),
            stretch: fontdb::Stretch::Normal,
            style: to_fontdb_style(properties.style),
        };
        let matched = self.font_system.read().db().query(&query);

        let font_id = matched.map_or(first_font_id, |face_id| self.font_id_for_face(face_id));
        self.registry
            .write()
            .selections
            .insert((family_id, properties), font_id);
        Some(font_id)
    }

    /// Registers `face_ids` as one family under `requested_name`.
    ///
    /// Idempotent: registering a name that already resolves returns the
    /// existing id rather than a second one, so a view that loads its family
    /// lazily on every render does not leak ids.
    fn register_family(&self, requested_name: &str, face_ids: Vec<fontdb::ID>) -> Result<FamilyId> {
        if let Some(family_id) = self.registry.read().family_by_name.get(requested_name) {
            return Ok(*family_id);
        }
        if face_ids.is_empty() {
            bail!("font family {requested_name:?} has no faces");
        }

        let canonical_name = {
            let font_system = self.font_system.read();
            face_ids
                .iter()
                .filter_map(|id| font_system.db().face(*id))
                .find_map(|face| face.families.first().map(|(name, _)| name.clone()))
                .unwrap_or_else(|| requested_name.to_owned())
        };

        // Family names are matched case-insensitively but cached by the exact
        // string, so a caller that asks for "segoe ui" after "Segoe UI" would
        // otherwise get a second id for the same faces.
        let existing = self
            .registry
            .read()
            .family_by_name
            .get(&canonical_name)
            .copied();
        if let Some(family_id) = existing {
            self.registry
                .write()
                .family_by_name
                .insert(requested_name.to_owned(), family_id);
            return Ok(family_id);
        }

        let font_ids: Vec<_> = face_ids
            .into_iter()
            .map(|face_id| self.font_id_for_face(face_id))
            .collect();

        // Warp validates every family it loads the same way, and for the same
        // reason: a face that cannot draw `m` cannot size a UI built out of
        // ems, and it is almost always a symbol or icon font that reached a
        // text code path by mistake.
        let has_latin = font_ids
            .iter()
            .any(|font_id| self.glyph_for_char(*font_id, VALIDATION_CHAR).is_some());
        if !has_latin {
            bail!(
                "font family {requested_name:?} (as {canonical_name:?}) has no glyph for \
                 {VALIDATION_CHAR:?}, so it cannot be used for text"
            );
        }

        let family_id = FamilyId::new();
        let mut registry = self.registry.write();
        registry.families.insert(
            family_id,
            Family {
                name: canonical_name.clone(),
                font_ids,
            },
        );
        registry
            .family_by_name
            .insert(requested_name.to_owned(), family_id);
        registry.family_by_name.insert(canonical_name, family_id);
        Ok(family_id)
    }

    fn glyph_for_char(&self, font_id: FontId, character: char) -> Option<GlyphId> {
        let glyph_id = self
            .with_font_ref(font_id, |font| font.charmap().map(character))
            .ok()?;

        // swash reports the notdef glyph for a character the face cannot cover.
        (glyph_id != 0).then_some(glyph_id.into())
    }

    /// The face and glyph a character falls back to when `font_id` has none.
    ///
    /// This is the shaper's own fallback, asked one character at a time.
    /// `ShapeLine` walks cosmic-text's per-OS fallback lists — script-specific
    /// families first, then the platform's common ones — and finishes by trying
    /// every face in the database, so the answer is "the best installed font
    /// that can draw this", not "one of a hardcoded few". It comes back with
    /// the face it chose, which is exactly what a grid needs to push a glyph
    /// record.
    ///
    /// The base face is named in the request so shaping starts where the grid
    /// is: a face that turns out to cover the character after all keeps it.
    fn fallback_glyph(&self, font_id: FontId, character: char) -> Option<(FontId, GlyphId)> {
        let mut buffer = [0; 4];
        let text: &str = character.encode_utf8(&mut buffer);

        let attrs_list = {
            let font_system = self.font_system.read();
            let base = self
                .face(font_id)
                .ok()
                .and_then(|face| font_system.db().face(face.id).cloned());
            let mut attrs = Attrs::new();
            if let Some(face) = &base
                && let Some((family, _)) = face.families.first()
            {
                attrs = attrs
                    .family(fontdb::Family::Name(family.as_str()))
                    .style(face.style)
                    .weight(face.weight);
            }
            AttrsList::new(&attrs)
        };

        let shape_line = {
            let mut font_system = self.font_system.write();
            ShapeLine::new(
                &mut font_system,
                text,
                &attrs_list,
                Shaping::Advanced,
                DEFAULT_TAB_WIDTH,
            )
        };
        // The size is irrelevant — only the face and the glyph id are read back
        // — so it is the one every font is defined at.
        let layout = shape_line.layout(
            1.,
            None,
            Wrap::None,
            Some(Align::Left),
            None,
            Hinting::Disabled,
        );

        let glyph = layout.first()?.glyphs.first()?;
        // Every fallback face was tried and none covered it: there is no font
        // on this machine that can draw this character.
        (glyph.glyph_id != 0).then(|| {
            (
                self.font_id_for_face(glyph.font_id),
                GlyphId::from(glyph.glyph_id),
            )
        })
    }

    fn font_metrics(&self, font_id: FontId) -> Metrics {
        if let Some(metrics) = self.metrics.read().get(&font_id) {
            return *metrics;
        }

        let read = self.with_font_ref(font_id, |font| {
            let metrics = font.metrics(&[]);
            Metrics {
                units_per_em: metrics.units_per_em.into(),
                ascent: metrics.ascent.round() as i16,
                // swash reports a positive drop below the baseline; the
                // `Metrics` contract is the OpenType `sTypoDescender` sign.
                descent: -(metrics.descent.round() as i16),
                line_gap: metrics.leading.round() as i16,
            }
        });

        let metrics = match read {
            Ok(metrics) if metrics.units_per_em > 0 => metrics,
            Ok(_) => {
                log::warn!("{font_id:?} reports zero units per em; using fallback metrics");
                FALLBACK_METRICS
            }
            Err(error) => {
                log::warn!("{font_id:?} has no readable metrics ({error:#}); using fallbacks");
                FALLBACK_METRICS
            }
        };

        self.metrics.write().insert(font_id, metrics);
        metrics
    }

    fn glyph_advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Vector2F> {
        if let Some(advance) = self.advances.read().get(&(font_id, glyph_id)) {
            return Ok(*advance);
        }

        let glyph = to_swash_glyph_id(glyph_id)?;
        let advance = self.with_font_ref(font_id, |font| {
            let metrics = font.glyph_metrics(&[]);
            if glyph >= metrics.glyph_count() {
                return None;
            }

            // Vertical advance is reported only when the face actually carries
            // vertical metrics. swash otherwise synthesizes one from ascent
            // plus descent, which would silently turn "this font has no
            // vertical layout" into a plausible-looking number.
            let vertical = if metrics.has_vertical_metrics() {
                metrics.advance_height(glyph)
            } else {
                0.
            };
            Some(vec2f(metrics.advance_width(glyph), vertical))
        })?;
        let advance =
            advance.with_context(|| format!("{font_id:?} has no glyph with id {glyph_id}"))?;

        self.advances.write().insert((font_id, glyph_id), advance);
        Ok(advance)
    }
}

/// Crook's font database: system font discovery, face selection, rasterization.
///
/// Construct one per process — [`CosmicFontDb::new`] scans every font installed
/// on the machine and takes tens to hundreds of milliseconds — and share the
/// shaper it hands out through [`CosmicFontDb::text_layout`].
pub struct CosmicFontDb {
    store: Arc<FontStore>,

    /// swash's scaler state. Rasterization needs `&mut` on it, and
    /// [`platform::FontDb`] rasterizes through `&self`, so it lives behind its
    /// own lock rather than in the store — nothing else ever wants it.
    swash: Mutex<SwashCache>,
}

impl CosmicFontDb {
    /// Discovers the machine's fonts and builds the backend.
    ///
    /// Fails only when the machine has no fonts at all, which a container image
    /// built from `scratch` genuinely can be. Every other degradation — a
    /// missing preferred family, an unparseable face — is handled further in.
    pub fn new() -> Result<Self> {
        let font_system = FontSystem::new();
        if font_system.db().is_empty() {
            bail!(
                "no fonts are installed on this machine: the system font scan found zero faces, \
                 so nothing can be drawn. Install a font package, or register one from bytes \
                 with FontDb::load_family_from_bytes before laying out any text."
            );
        }

        log::debug!("font backend ready: {} faces", font_system.db().len());
        Ok(Self::from_font_system(font_system))
    }

    /// Builds the backend over a font system the caller assembled.
    ///
    /// The way to get a deterministic font set: hand it a [`FontSystem`] built
    /// with [`FontSystem::new_with_locale_and_db`] over an empty database and
    /// register bundled faces from bytes. Tests want this; so does a build that
    /// ships its own fonts and refuses to look like the machine it runs on.
    pub fn from_font_system(font_system: FontSystem) -> Self {
        Self {
            store: Arc::new(FontStore::new(font_system)),
            swash: Mutex::new(SwashCache::new()),
        }
    }

    /// A shaper sharing this backend's fonts.
    ///
    /// Cheap: it clones an [`Arc`]. Hand it to the presenter, or to a worker —
    /// unlike the database, the shaper is `Send + Sync`.
    pub fn text_layout(&self) -> CosmicTextLayout {
        CosmicTextLayout::new(Arc::clone(&self.store))
    }

    /// Registers an installed family by name.
    ///
    /// Matching is case-insensitive against every name the face declares.
    /// Calling this twice with the same name returns the same [`FamilyId`].
    pub fn load_family_from_system(&self, name: &str) -> Result<FamilyId> {
        if let Some(family_id) = self.family_id_for_name(name) {
            return Ok(family_id);
        }

        let face_ids: Vec<_> = {
            let font_system = self.store.font_system.read();
            font_system
                .db()
                .faces()
                .filter(|face| {
                    face.families
                        .iter()
                        .any(|(family, _)| family.eq_ignore_ascii_case(name))
                })
                .map(|face| face.id)
                .collect()
        };

        if face_ids.is_empty() {
            bail!("no font family named {name:?} is installed");
        }
        self.store.register_family(name, face_ids)
    }

    /// Registers the first of `preferences` that is installed and usable.
    ///
    /// Falls back, in order, to the database's generic sans-serif family and
    /// then to any installed face that can draw Latin text, so this fails only
    /// on a machine whose fonts are all unusable.
    pub fn load_first_available_family(&self, preferences: &[&str]) -> Result<FamilyId> {
        for name in preferences {
            match self.load_family_from_system(name) {
                Ok(family_id) => return Ok(family_id),
                Err(error) => log::debug!("font family {name:?} unavailable: {error:#}"),
            }
        }

        let generic = {
            let font_system = self.store.font_system.read();
            font_system
                .db()
                .family_name(&fontdb::Family::SansSerif)
                .to_owned()
        };
        if let Ok(family_id) = self.load_family_from_system(&generic) {
            log::info!("none of {preferences:?} is installed; falling back to {generic:?}");
            return Ok(family_id);
        }

        let names: Vec<_> = {
            let font_system = self.store.font_system.read();
            font_system
                .db()
                .faces()
                .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
                .collect()
        };
        for name in &names {
            if let Ok(family_id) = self.load_family_from_system(name) {
                log::warn!("no preferred font family is installed; falling back to {name:?}");
                return Ok(family_id);
            }
        }

        bail!(
            "no installed font family can draw text: tried {preferences:?}, the generic \
             sans-serif family {generic:?}, and all {} families the font database knows",
            names.len()
        )
    }

    /// The platform's default UI family. See [`DEFAULT_UI_FAMILIES`].
    pub fn default_ui_family(&self) -> Result<FamilyId> {
        self.load_first_available_family(DEFAULT_UI_FAMILIES)
    }

    /// The platform's default monospace family.
    /// See [`DEFAULT_MONOSPACE_FAMILIES`].
    pub fn default_monospace_family(&self) -> Result<FamilyId> {
        self.load_first_available_family(DEFAULT_MONOSPACE_FAMILIES)
    }

    /// The id a family was registered under, if it was.
    pub fn family_id_for_name(&self, name: &str) -> Option<FamilyId> {
        self.store.registry.read().family_by_name.get(name).copied()
    }

    /// The name the font database knows a registered family by.
    pub fn family_name(&self, family_id: FamilyId) -> Option<String> {
        self.store
            .registry
            .read()
            .families
            .get(&family_id)
            .map(|family| family.name.clone())
    }

    /// A handle to the measurement path, sharing this backend's fonts.
    ///
    /// Cheap: it clones an [`Arc`]. See [`CosmicGlyphs`] for why a grid needs
    /// one rather than a table computed at startup.
    pub fn glyphs(&self) -> CosmicGlyphs {
        CosmicGlyphs {
            store: Arc::clone(&self.store),
        }
    }

    /// The face in `family_id` that best matches `properties`.
    ///
    /// `None` only for a [`FamilyId`] this backend never issued.
    pub fn select_font(&self, family_id: FamilyId, properties: Properties) -> Option<FontId> {
        self.store.select_font(family_id, properties)
    }

    /// The glyph `character` maps to in `font_id`, if the face covers it.
    ///
    /// This is the measurement path — a terminal grid asks it once per distinct
    /// character and then pushes glyph records straight into the scene, never
    /// shaping at all. It does *not* fall back to another face; see
    /// [`CosmicFontDb::fallback_fonts`].
    pub fn glyph_for_char(&self, font_id: FontId, character: char) -> Option<GlyphId> {
        self.store.glyph_for_char(font_id, character)
    }

    /// The face and glyph `character` falls back to when `font_id` has none.
    ///
    /// Font fallback happens twice in this stack. Shaping does its own, from
    /// cosmic-text's per-OS lists, so a tab title containing CJK or an emoji
    /// has always rendered correctly and the [`FontId`]s of the faces it chose
    /// come back in the [`Run`]s. This is the same answer for the other path —
    /// the single-glyph measurement one above — which is what a terminal grid
    /// paints from. A grid is *not* Latin by construction: a spinner is
    /// braille, a prompt is powerline, `ls` prints whatever the filenames are.
    ///
    /// Warp's version costs a `fontconfig` link on Linux and a DirectWrite
    /// `IDWriteFontFallback` call on Windows. This one costs a shaping call per
    /// distinct character its caller has never seen before, and callers memoize
    /// it.
    ///
    /// [`Run`]: crookui_core::text_layout::Run
    pub fn fallback_glyph(&self, font_id: FontId, character: char) -> Option<(FontId, GlyphId)> {
        self.store.fallback_glyph(font_id, character)
    }

    /// The advance width of `m` in `family_id` at `font_size`, in logical
    /// pixels — the unit UI sizing is expressed in.
    pub fn em_width(&self, family_id: FamilyId, font_size: f32) -> Result<f32> {
        self.glyphs().em_width(family_id, font_size)
    }
}

/// The measurement path on its own: which glyph a character is, how wide it is,
/// and what a face's vertical metrics are.
///
/// A cheap-clone handle over the same `FontStore` [`CosmicFontDb`] owns, which
/// is the trick [`CosmicFontDb::text_layout`] already plays for the shaper. It
/// exists because a monospace grid is the one thing in Crook that resolves
/// glyphs *while painting* rather than while shaping: it asks
/// [`Self::glyph_for_char`] per distinct character and then pushes glyph records
/// straight into the scene at a fixed advance. The database itself is moved into
/// the event loop when the window opens, so nothing above the platform line can
/// hold it — and a table precomputed at startup cannot answer for the characters
/// a shell has not printed yet.
///
/// Rasterization is deliberately not here. It needs `&mut` scaler state and its
/// only caller is the renderer, which has the database itself.
#[derive(Clone)]
pub struct CosmicGlyphs {
    store: Arc<FontStore>,
}

impl CosmicGlyphs {
    /// The face in `family_id` that best matches `properties`.
    ///
    /// `None` only for a [`FamilyId`] the backend never issued.
    pub fn select_font(&self, family_id: FamilyId, properties: Properties) -> Option<FontId> {
        self.store.select_font(family_id, properties)
    }

    /// The glyph `character` maps to in `font_id`, or `None` when the face does
    /// not cover it. For the answer that also looks at every other installed
    /// face, see [`Self::fallback_glyph`].
    pub fn glyph_for_char(&self, font_id: FontId, character: char) -> Option<GlyphId> {
        self.store.glyph_for_char(font_id, character)
    }

    /// The face and glyph `character` falls back to when `font_id` has none.
    /// See [`CosmicFontDb::fallback_glyph`].
    pub fn fallback_glyph(&self, font_id: FontId, character: char) -> Option<(FontId, GlyphId)> {
        self.store.fallback_glyph(font_id, character)
    }

    /// A glyph's advance in font units — scale by `font_size / units_per_em`.
    pub fn glyph_advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Vector2F> {
        self.store.glyph_advance(font_id, glyph_id)
    }

    /// The font-wide metrics of a face, in font units.
    pub fn font_metrics(&self, font_id: FontId) -> Metrics {
        self.store.font_metrics(font_id)
    }

    /// The advance width of `m` in `family_id` at `font_size`, in logical
    /// pixels — the unit UI sizing, and a terminal cell, is expressed in.
    pub fn em_width(&self, family_id: FamilyId, font_size: f32) -> Result<f32> {
        let font_id = self
            .select_font(family_id, Properties::default())
            .with_context(|| format!("{family_id:?} was never registered"))?;
        let glyph_id = self
            .glyph_for_char(font_id, VALIDATION_CHAR)
            .with_context(|| format!("{font_id:?} has no {VALIDATION_CHAR:?} glyph"))?;

        let advance = self.glyph_advance(font_id, glyph_id)?;
        let units_per_em = self.font_metrics(font_id).units_per_em;
        Ok(advance.x() * font_size / units_per_em as f32)
    }
}

impl platform::FontDb for CosmicFontDb {
    fn load_family_from_bytes(&mut self, name: &str, bytes: Vec<Vec<u8>>) -> Result<FamilyId> {
        let slice_count = bytes.len();
        let face_ids: Vec<_> = {
            let mut font_system = self.store.font_system.write();
            let db = font_system.db_mut();
            bytes
                .into_iter()
                .flat_map(|data| db.load_font_source(fontdb::Source::Binary(Arc::new(data))))
                .collect()
        };

        if face_ids.is_empty() {
            bail!("none of the {slice_count} byte slices given for {name:?} parsed as a font");
        }
        self.store.register_family(name, face_ids)
    }

    fn font_metrics(&self, font_id: FontId) -> Metrics {
        self.store.font_metrics(font_id)
    }

    fn glyph_advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Vector2F> {
        self.store.glyph_advance(font_id, glyph_id)
    }

    fn glyph_raster_bounds(&self, glyph_key: GlyphKey, scale: Vector2F) -> Result<RasterBounds> {
        self.raster_bounds(glyph_key, scale)
    }

    fn rasterize_glyph(
        &self,
        glyph_key: GlyphKey,
        scale: Vector2F,
        subpixel_alignment: SubpixelAlignment,
        format: RasterFormat,
    ) -> Result<RasterizedGlyph> {
        self.rasterize(glyph_key, scale, subpixel_alignment, format)
    }
}

/// Narrows a [`GlyphId`] to the `u16` every font format actually stores.
///
/// `GlyphId` is `u32` across the API and `u16` in every backend. Warp casts,
/// which silently wraps on a face with more than 65,535 glyphs — large CJK and
/// some emoji faces do exist. This refuses instead.
fn to_swash_glyph_id(glyph_id: GlyphId) -> Result<u16> {
    u16::try_from(glyph_id)
        .with_context(|| format!("glyph id {glyph_id} does not fit the 16 bits a font face uses"))
}

fn to_fontdb_weight(weight: Weight) -> fontdb::Weight {
    fontdb::Weight(weight.to_number())
}

fn to_fontdb_style(style: Style) -> fontdb::Style {
    match style {
        Style::Normal => fontdb::Style::Normal,
        Style::Italic => fontdb::Style::Italic,
    }
}
