//! Parley owns bidi, shaping and breaking; the display-list boundary uses font handles and glyph IDs.
#![warn(missing_docs)]
pub use editing::{Clipboard, EditEvent, TextInput};
mod editing;
/// Text provider errors.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
/// Measured paragraph dimensions.
#[derive(Clone, Copy, Debug, Default)]
pub struct Measure {
    /// Measured paragraph width in logical pixels.
    pub width: f32,
    /// Measured paragraph height in logical pixels.
    pub height: f32,
}
use parley::{
    FontContext, FontFamily, Layout, LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty,
};
use serde::Serialize;
/// Registered font bytes behind a renderer-neutral font handle.
pub struct FontAsset {
    /// Stable font-resource cache identity.
    pub source_id: u64,
    /// Face index within a font collection.
    pub index: u32,
    /// Immutable font data owned independently of Parley layouts.
    pub bytes: Vec<u8>,
}
/// Reusable Parley contexts and explicit fallback resources; no implicit OS font discovery.
pub struct TextSystem {
    /// Explicit Parley font collection and source cache.
    pub fonts: FontContext,
    /// Reusable paragraph-layout allocation context.
    pub context: LayoutContext<u8>,
    /// Registered renderer resources used by shaped runs.
    pub assets: Vec<FontAsset>,
    /// Cumulative paragraph shaping counter (cache misses only).
    pub shapes: usize,
    families: String,
    /// Recently shaped paragraphs, unbroken, by text, family and size: a recreated
    /// node (a remounted virtual row, a re-shown panel) re-breaks instead of reshaping.
    shaped: std::collections::HashMap<(String, Option<String>, u32), Layout<u8>>,
}
/// Shaped paragraphs kept for reuse before the cache starts over.
const SHAPE_CACHE: usize = 4096;
/// Cached shaped paragraph; width changes rebreak lines without reshaping runs.
pub struct Text {
    /// Current UTF-8 display string.
    pub value: String,
    /// Needs text measurement or geometry recomputation.
    pub layout: Layout<u8>,
    /// Last line-breaking width constraint in logical pixels.
    pub width: Option<f32>,
    /// Logical viewport width and height.
    pub size: f32,
    /// Optional explicit family list for numeric or branded UI text.
    pub family: Option<String>,
    /// Separate incremental work flags.
    pub dirty: bool,
}
impl Text {
    /// Create initial owned state with its cache marked dirty.
    pub fn new(value: String) -> Self {
        Self {
            value,
            layout: Layout::new(),
            width: None,
            size: 0.0,
            family: None,
            dirty: true,
        }
    }
    /// Replace changed text while keeping the paragraph allocation for reuse.
    pub fn set(&mut self, value: &str) -> bool {
        if self.value == value {
            return false;
        }
        self.value.clear();
        self.value.push_str(value);
        self.dirty = true;
        true
    }
}
impl TextSystem {
    /// Licensed, pinned Latin/Greek/Cyrillic and Arabic fallback shipped with Ara.
    /// Hosts may add CJK/emoji/font packs through `new` for their localized content.
    pub fn bundled() -> Result<Self> {
        Self::new(
            [
                include_bytes!("../fonts/NotoSans.ttf").to_vec(),
                include_bytes!("../fonts/NotoNaskhArabic.ttf").to_vec(),
            ],
            "'Noto Sans', 'Noto Naskh Arabic'",
        )
    }
    /// Register explicit, caller-owned licensed fonts; never discover platform fonts implicitly.
    pub fn new(font_data: impl IntoIterator<Item = Vec<u8>>, families: &str) -> Result<Self> {
        let mut fonts = FontContext {
            collection: parley::fontique::Collection::new(parley::fontique::CollectionOptions {
                shared: false,
                system_fonts: false,
            }),
            source_cache: Default::default(),
        };
        let mut count = 0;
        for bytes in font_data {
            if fonts
                .collection
                .register_fonts(bytes.into(), None)
                .is_empty()
            {
                return Err("ARA4030: font bytes contain no valid font faces".into());
            }
            count += 1;
        }
        if count == 0 {
            return Err("ARA4030: at least one explicit font is required".into());
        }
        Ok(Self {
            fonts,
            context: LayoutContext::new(),
            assets: Vec::new(),
            shapes: 0,
            families: families.into(),
            shaped: Default::default(),
        })
    }
    /// Width is the content-box constraint from Taffy's leaf measure callback.
    pub fn measure(&mut self, text: &mut Text, width: Option<f32>, size: f32) -> Measure {
        let width = width.map(|w| w.max(1.0));
        if text.dirty || text.size != size {
            let key = (text.value.clone(), text.family.clone(), size.to_bits());
            if let Some(layout) = self.shaped.get(&key) {
                text.layout.clone_from(layout);
            } else {
                let mut builder =
                    self.context
                        .ranged_builder(&mut self.fonts, &text.value, 1.0, true);
                builder.push_default(StyleProperty::FontSize(size));
                builder.push_default(FontFamily::from(
                    text.family.as_deref().unwrap_or(self.families.as_str()),
                ));
                builder.push_default(LineHeight::FontSizeRelative(1.5));
                // Prefer normal word boundaries, then break an overlong word at
                // Parley's grapheme/shape-atom boundaries. An unconstrained width
                // (nowrap and single-line fields) still remains unbroken.
                builder.push_default(StyleProperty::OverflowWrap(parley::OverflowWrap::Anywhere));
                builder.build_into(&mut text.layout, &text.value);
                self.shapes += 1;
                if self.shaped.len() >= SHAPE_CACHE {
                    self.shaped.clear();
                }
                self.shaped.insert(key, text.layout.clone());
            }
            text.size = size;
            text.dirty = false;
            text.width = None;
        }
        if text.width != width || text.layout.is_empty() {
            text.layout.break_all_lines(width);
            text.layout.align(
                parley::Alignment::Start,
                parley::AlignmentOptions::default(),
            );
            text.width = width;
            for line in text.layout.lines() {
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(run) = item {
                        let font = run.run().font();
                        if !self
                            .assets
                            .iter()
                            .any(|a| a.source_id == font.data.id() && a.index == font.index)
                        {
                            self.assets.push(FontAsset {
                                source_id: font.data.id(),
                                index: font.index,
                                bytes: font.data.as_ref().to_vec(),
                            });
                        }
                    }
                }
            }
        }
        Measure {
            width: text.layout.width(),
            height: text.layout.height(),
        }
    }
    /// Return the registered renderer font handle for a shaped Parley run.
    pub fn handle(&self, font: &parley::FontData) -> u32 {
        self.assets
            .iter()
            .position(|a| a.source_id == font.data.id() && a.index == font.index)
            .expect("every shaped font was registered during measure") as u32
    }
}
/// Structured shaping coverage and bidi/cluster evidence for agents.
#[derive(Serialize)]
pub struct TextEvidence {
    /// Cached paragraph, when this control displays text.
    pub text: String,
    /// Number of broken text lines.
    pub lines: usize,
    /// Number of .notdef glyphs, indicating missing font coverage.
    pub missing_glyphs: usize,
    /// Total shaped glyph count.
    pub glyph_count: usize,
    /// Right-to-left shaped run count.
    pub rtl_runs: usize,
    /// Left-to-right shaped run count.
    pub ltr_runs: usize,
    /// Original-byte cluster records for selection/coverage inspection.
    pub clusters: Vec<ClusterEvidence>,
}
/// Original text byte range and shaped glyphs for one Unicode cluster.
#[derive(Serialize)]
pub struct ClusterEvidence {
    /// Immutable font data owned independently of Parley layouts.
    pub bytes: std::ops::Range<usize>,
    /// Resolved cluster writing direction.
    pub rtl: bool,
    /// Contiguous positioned glyph arena addressed by command ranges.
    pub glyphs: Vec<u32>,
}
/// Return computed geometry, text coverage, provenance, focus graph and batch diagnostics.
pub fn inspect(text: &Text) -> TextEvidence {
    let mut out = TextEvidence {
        text: text.value.to_owned(),
        lines: text.layout.len(),
        missing_glyphs: 0,
        glyph_count: 0,
        rtl_runs: 0,
        ltr_runs: 0,
        clusters: Vec::new(),
    };
    for line in text.layout.lines() {
        for run in line.runs() {
            if run.is_rtl() {
                out.rtl_runs += 1;
            } else {
                out.ltr_runs += 1;
            }
            for cluster in run.clusters() {
                let glyphs: Vec<u32> = cluster.glyphs().map(|g| g.id).collect();
                out.missing_glyphs += glyphs.iter().filter(|g| **g == 0).count();
                out.glyph_count += glyphs.len();
                out.clusters.push(ClusterEvidence {
                    bytes: cluster.text_range(),
                    rtl: cluster.is_rtl(),
                    glyphs,
                });
            }
        }
    }
    out
}
