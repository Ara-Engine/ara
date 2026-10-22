//! Renderer-neutral ordered commands. Ranges reference one contiguous glyph arena, ready for batching.
use crate::{source::Result, style::Color, text::FontAsset};
use serde::Serialize;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Box2 {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
impl Box2 {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    pub fn intersects(self, b: Self) -> bool {
        self.x < b.x + b.w && self.x + self.w > b.x && self.y < b.y + b.h && self.y + self.h > b.y
    }
    pub fn intersection(self, b: Self) -> Self {
        let x = self.x.max(b.x);
        let y = self.y.max(b.y);
        Self {
            x,
            y,
            w: (self.x + self.w).min(b.x + b.w).max(x) - x,
            h: (self.y + self.h).min(b.y + b.h).max(y) - y,
        }
    }
}
#[derive(Clone, Copy, Serialize)]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}
#[derive(Serialize)]
#[serde(tag = "op")]
pub enum Draw {
    Rect {
        node: u64,
        rect: Box2,
        color: Color,
    },
    RoundedRect {
        node: u64,
        rect: Box2,
        radius: f32,
        color: Color,
    },
    Border {
        node: u64,
        rect: Box2,
        radius: f32,
        width: f32,
        color: Color,
    },
    GlyphRun {
        node: u64,
        font: u32,
        size: f32,
        origin: [f32; 2],
        glyphs: Range<usize>,
        color: Color,
    },
    Image {
        node: u64,
        rect: Box2,
        image: u32,
    },
    PushClip {
        rect: Box2,
    },
    PopClip,
    PushTransform {
        translation: [f32; 2],
    },
    PopTransform,
}
#[derive(Default, Serialize)]
pub struct DisplayList {
    pub commands: Vec<Draw>,
    pub glyphs: Vec<Glyph>,
}
#[derive(Serialize)]
pub struct BatchBreak {
    pub command: usize,
    pub cause: &'static str,
}
impl DisplayList {
    /// Adjacent draws batch only when pipeline/atlas match; clips and transforms are hard barriers.
    pub fn breaks(&self) -> Vec<BatchBreak> {
        let mut previous: Option<(u8, u32)> = None;
        let mut breaks = Vec::new();
        for (i, draw) in self.commands.iter().enumerate() {
            let key = match draw {
                Draw::GlyphRun { font, .. } => (1, *font),
                Draw::Image { image, .. } => (2, *image),
                Draw::Rect { .. } | Draw::RoundedRect { .. } | Draw::Border { .. } => (0, 0),
                _ => {
                    breaks.push(BatchBreak {
                        command: i,
                        cause: match draw {
                            Draw::PushClip { .. } | Draw::PopClip => "clip barrier",
                            _ => "transform barrier",
                        },
                    });
                    previous = None;
                    continue;
                }
            };
            if let Some(old) = previous
                && old != key
            {
                breaks.push(BatchBreak {
                    command: i,
                    cause: if old.0 == key.0 {
                        "texture/font atlas change"
                    } else {
                        "pipeline change"
                    },
                });
            }
            previous = Some(key);
        }
        breaks
    }
    /// Slow reference rasterization is deliberately outside the UI CPU benchmark.
    pub fn render(
        &self,
        fonts: &[FontAsset],
        width: u32,
        height: u32,
        path: &std::path::Path,
    ) -> Result<()> {
        let mut pixmap =
            tiny_skia::Pixmap::new(width, height).ok_or("invalid raster dimensions")?;
        pixmap.fill(tiny_skia::Color::from_rgba8(13, 19, 28, 255));
        let mut clips = vec![Box2 {
            x: 0.0,
            y: 0.0,
            w: width as f32,
            h: height as f32,
        }];
        let mut transforms = vec![[0.0, 0.0]];
        let mut scale = swash::scale::ScaleContext::new();
        for draw in &self.commands {
            let [tx, ty] = *transforms.last().expect("transform stack has a root");
            let clip = *clips.last().expect("clip stack has a root");
            match draw {
                Draw::PushClip { rect } => clips.push(clip.intersection(Box2 {
                    x: rect.x + tx,
                    y: rect.y + ty,
                    ..*rect
                })),
                Draw::PopClip => {
                    if clips.len() > 1 {
                        clips.pop();
                    }
                }
                Draw::PushTransform { translation } => {
                    transforms.push([tx + translation[0], ty + translation[1]])
                }
                Draw::PopTransform => {
                    if transforms.len() > 1 {
                        transforms.pop();
                    }
                }
                Draw::Rect { rect, color, .. } => {
                    paint_box(&mut pixmap, *rect, *color, 0.0, 0.0, [tx, ty], clip)
                }
                Draw::RoundedRect {
                    rect,
                    color,
                    radius,
                    ..
                } => paint_box(&mut pixmap, *rect, *color, *radius, 0.0, [tx, ty], clip),
                Draw::Border {
                    rect,
                    color,
                    radius,
                    width,
                    ..
                } => paint_box(&mut pixmap, *rect, *color, *radius, *width, [tx, ty], clip),
                Draw::Image { rect, image, .. } => {
                    // A procedural checker asset exercises the image texture boundary without disk media.
                    for row in 0..4 {
                        for col in 0..4 {
                            paint_box(
                                &mut pixmap,
                                Box2 {
                                    x: rect.x + col as f32 * rect.w / 4.0,
                                    y: rect.y + row as f32 * rect.h / 4.0,
                                    w: rect.w / 4.0,
                                    h: rect.h / 4.0,
                                },
                                if (row + col + image) % 2 == 0 {
                                    Color(94, 202, 176, 255)
                                } else {
                                    Color(34, 81, 85, 255)
                                },
                                0.0,
                                0.0,
                                [tx, ty],
                                clip,
                            );
                        }
                    }
                }
                Draw::GlyphRun {
                    font,
                    size,
                    origin,
                    glyphs,
                    color,
                    ..
                } => {
                    let asset = &fonts[*font as usize];
                    let font = swash::FontRef::from_index(&asset.bytes, asset.index as usize)
                        .ok_or("invalid raster font")?;
                    let mut scaler = scale.builder(font).size(*size).hint(true).build();
                    for glyph in &self.glyphs[glyphs.start..glyphs.end] {
                        let x = origin[0] + tx + glyph.x;
                        let y = origin[1] + ty + glyph.y;
                        let Some(image) = swash::scale::Render::new(&[
                            swash::scale::Source::ColorOutline(0),
                            swash::scale::Source::ColorBitmap(swash::scale::StrikeWith::BestFit),
                            swash::scale::Source::Outline,
                        ])
                        .format(swash::zeno::Format::Alpha)
                        .render(&mut scaler, glyph.id as u16) else {
                            continue;
                        };
                        let p = image.placement;
                        for row in 0..p.height {
                            for col in 0..p.width {
                                let px = x.floor() as i32 + p.left + col as i32;
                                let py = y.floor() as i32 - p.top + row as i32;
                                if px < 0
                                    || py < 0
                                    || px >= width as i32
                                    || py >= height as i32
                                    || !clip.contains(px as f32, py as f32)
                                {
                                    continue;
                                }
                                let index = (row * p.width + col) as usize;
                                let rgba = match image.content {
                                    swash::scale::image::Content::Mask => {
                                        let a =
                                            u16::from(image.data[index]) * u16::from(color.3) / 255;
                                        [color.0, color.1, color.2, a as u8]
                                    }
                                    _ => {
                                        let i = index * 4;
                                        [
                                            image.data[i],
                                            image.data[i + 1],
                                            image.data[i + 2],
                                            image.data[i + 3],
                                        ]
                                    }
                                };
                                blend(
                                    pixmap.data_mut(),
                                    ((py as u32 * width + px as u32) * 4) as usize,
                                    rgba,
                                );
                            }
                        }
                    }
                }
            }
        }
        pixmap.save_png(path)?;
        Ok(())
    }
}
fn blend(data: &mut [u8], i: usize, rgba: [u8; 4]) {
    let alpha = u32::from(rgba[3]);
    for channel in 0..3 {
        data[i + channel] = ((u32::from(rgba[channel]) * alpha
            + u32::from(data[i + channel]) * (255 - alpha)
            + 127)
            / 255) as u8;
    }
    data[i + 3] = (alpha + (u32::from(data[i + 3]) * (255 - alpha) + 127) / 255) as u8;
}
fn rounded(rect: Box2, radius: f32) -> Option<tiny_skia::Path> {
    let r = radius.min(rect.w / 2.0).min(rect.h / 2.0).max(0.0);
    let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
    let k = 0.5522848 * r;
    let mut p = tiny_skia::PathBuilder::new();
    p.move_to(x + r, y);
    p.line_to(x + w - r, y);
    p.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    p.line_to(x + w, y + h - r);
    p.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    p.line_to(x + r, y + h);
    p.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    p.line_to(x, y + r);
    p.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    p.close();
    p.finish()
}
#[allow(clippy::too_many_arguments)]
fn paint_box(
    pixmap: &mut tiny_skia::Pixmap,
    mut rect: Box2,
    color: Color,
    radius: f32,
    border: f32,
    translation: [f32; 2],
    clip: Box2,
) {
    rect.x += translation[0];
    rect.y += translation[1];
    if rect.w <= 0.0 || rect.h <= 0.0 || !rect.intersects(clip) {
        return;
    }
    let Some(path) = rounded(rect, radius) else {
        return;
    };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(color.0, color.1, color.2, color.3);
    let mut mask =
        tiny_skia::Mask::new(pixmap.width(), pixmap.height()).expect("pixmap dimensions are valid");
    if let Some(rect) = tiny_skia::Rect::from_xywh(clip.x, clip.y, clip.w, clip.h) {
        let path = tiny_skia::PathBuilder::from_rect(rect);
        mask.fill_path(
            &path,
            tiny_skia::FillRule::Winding,
            false,
            tiny_skia::Transform::identity(),
        );
    }
    if border > 0.0 {
        pixmap.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: border,
                ..Default::default()
            },
            tiny_skia::Transform::identity(),
            Some(&mask),
        );
    } else {
        pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            Some(&mask),
        );
    }
}
