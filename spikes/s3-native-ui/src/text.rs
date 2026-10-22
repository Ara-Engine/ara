//! Parley owns bidi, shaping and breaking; the display-list boundary uses font handles and glyph IDs.
use crate::source::Result;
use parley::{
    FontContext, FontFamily, Layout, LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty,
};
use serde::Serialize;

pub struct FontAsset {
    pub source_id: u64,
    pub index: u32,
    pub bytes: Vec<u8>,
}
pub struct TextSystem {
    pub fonts: FontContext,
    pub context: LayoutContext<u8>,
    pub assets: Vec<FontAsset>,
    pub shapes: usize,
}
pub struct Text {
    pub value: String,
    pub layout: Layout<u8>,
    pub width: Option<f32>,
    pub size: f32,
    pub dirty: bool,
}
impl Text {
    pub fn new(value: String) -> Self {
        Self {
            value,
            layout: Layout::new(),
            width: None,
            size: 0.0,
            dirty: true,
        }
    }
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
    pub fn new() -> Result<Self> {
        let mut fonts = FontContext {
            collection: parley::fontique::Collection::new(parley::fontique::CollectionOptions {
                shared: false,
                system_fonts: false,
            }),
            source_cache: Default::default(),
        };
        // Explicit font selection makes fallback repeatable on this machine; no OS discovery.
        for file in [
            "segoeui.ttf",
            "NotoNaskhArabic-Regular.ttf",
            "msyh.ttc",
            "malgun.ttf",
            "seguiemj.ttf",
        ] {
            let bytes = std::fs::read(format!("C:/Windows/Fonts/{file}"))?;
            fonts.collection.register_fonts(bytes.into(), None);
        }
        Ok(Self {
            fonts,
            context: LayoutContext::new(),
            assets: Vec::new(),
            shapes: 0,
        })
    }
    /// Width is the content-box constraint from Taffy's leaf measure callback.
    pub fn measure(&mut self, text: &mut Text, width: Option<f32>, size: f32) -> taffy::Size<f32> {
        let width = width.map(|w| w.max(1.0));
        if text.dirty || text.size != size {
            let mut builder = self
                .context
                .ranged_builder(&mut self.fonts, &text.value, 1.0, true);
            builder.push_default(StyleProperty::FontSize(size));
            builder.push_default(FontFamily::from("'Segoe UI', 'Noto Naskh Arabic', 'Microsoft YaHei', 'Malgun Gothic', 'Segoe UI Emoji'"));
            builder.push_default(LineHeight::FontSizeRelative(1.5));
            builder.build_into(&mut text.layout, &text.value);
            text.size = size;
            text.dirty = false;
            text.width = None;
            self.shapes += 1;
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
        taffy::Size {
            width: text.layout.width(),
            height: text.layout.height(),
        }
    }
    pub fn handle(&self, font: &parley::FontData) -> u32 {
        self.assets
            .iter()
            .position(|a| a.source_id == font.data.id() && a.index == font.index)
            .expect("every shaped font was registered during measure") as u32
    }
}

#[derive(Serialize)]
pub struct TextEvidence {
    pub text: String,
    pub lines: usize,
    pub missing_glyphs: usize,
    pub glyph_count: usize,
    pub rtl_runs: usize,
    pub ltr_runs: usize,
    pub clusters: Vec<ClusterEvidence>,
}
#[derive(Serialize)]
pub struct ClusterEvidence {
    pub bytes: std::ops::Range<usize>,
    pub rtl: bool,
    pub glyphs: Vec<u32>,
}
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
