//! Pre-shaped labels survive pan/zoom; only glyph positions and sizes are transformed.
use ara_ui::{
    paint::{DisplayList, Draw, Glyph},
    style::Color,
};
use ara_ui_text::{Text, TextSystem};
use parley::PositionedLayoutItem;
use std::collections::BTreeMap;
#[derive(Default)]
pub(super) struct Labels {
    cache: BTreeMap<String, Vec<Run>>,
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
        system.measure(&mut text, None, 12.);
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
        self.cache.insert(value.into(), runs);
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
        if let Some(runs) = self.cache.get(value) {
            for run in runs {
                let start = list.glyphs.len();
                list.glyphs.extend(run.glyphs.iter().map(|g| Glyph {
                    id: g.id,
                    x: g.x * scale,
                    y: g.y * scale,
                }));
                list.commands.push(Draw::GlyphRun {
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
