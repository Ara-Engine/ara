//! Pre-shaped labels survive pan/zoom; only glyph positions and sizes are transformed.
//! Labels are shaped once at [`SIZE`]; painting scales them (11 px captions use 11/12).
use ara_ui::{
    paint::{DisplayList, Draw, Glyph},
    style::Color,
};
use ara_ui_text::{Text, TextSystem};
use parley::PositionedLayoutItem;
use std::collections::BTreeMap;
/// Shaping size in logical pixels.
pub(super) const SIZE: f32 = 12.;
#[derive(Default)]
pub(super) struct Labels {
    cache: BTreeMap<String, Label>,
}
struct Label {
    runs: Vec<Run>,
    width: f32,
}
struct Run {
    font: u32,
    size: f32,
    glyphs: Vec<Glyph>,
}
impl Labels {
    pub fn prepare(&mut self, value: &str, system: &mut TextSystem) {
        if self.cache.contains_key(value) {
            return;
        }
        let mut text = Text::new(value.into());
        system.measure(&mut text, None, SIZE);
        let mut runs = Vec::new();
        for line in text.layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(run) = item {
                    runs.push(Run {
                        font: system.handle(run.run().font()),
                        size: run.run().font_size(),
                        glyphs: run
                            .positioned_glyphs()
                            .map(|g| Glyph {
                                id: g.id,
                                x: g.x,
                                y: g.y,
                            })
                            .collect(),
                    });
                }
            }
        }
        let width = text.layout.width();
        self.cache.insert(value.into(), Label { runs, width });
    }
    /// Shaped advance width at [`SIZE`], before any paint scale; zero if unprepared.
    pub fn width(&self, value: &str) -> f32 {
        self.cache.get(value).map_or(0., |label| label.width)
    }
    pub fn paint(
        &self,
        list: &mut DisplayList,
        id: u64,
        value: &str,
        origin: [f32; 2],
        scale: f32,
        color: Color,
    ) {
        if let Some(label) = self.cache.get(value) {
            for run in &label.runs {
                let start = list.glyphs.len();
                list.glyphs.extend(run.glyphs.iter().map(|g| Glyph {
                    id: g.id,
                    x: g.x * scale,
                    y: g.y * scale,
                }));
                list.commands.push(Draw::ScalableGlyphRun {
                    node: id,
                    font: run.font,
                    size: run.size * scale,
                    origin,
                    glyphs: start..list.glyphs.len(),
                    color,
                });
            }
        }
    }
}
