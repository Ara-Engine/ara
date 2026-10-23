//! Renderer-neutral ordered commands. Ranges reference one contiguous glyph arena, ready for batching.
use crate::style::Color;
use serde::Serialize;
use std::ops::Range;
/// Logical-pixel rectangle in the UI viewport.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Box2 {
    /// Horizontal coordinate in logical pixels.
    pub x: f32,
    /// Vertical coordinate in logical pixels.
    pub y: f32,
    /// Width in logical pixels.
    pub w: f32,
    /// Height in logical pixels.
    pub h: f32,
}
impl Box2 {
    /// Test a logical point using half-open rectangle bounds.
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    /// Test nonempty rectangle overlap.
    pub fn intersects(self, b: Self) -> bool {
        self.w > 0.0
            && self.h > 0.0
            && b.w > 0.0
            && b.h > 0.0
            && self.x < b.x + b.w
            && self.x + self.w > b.x
            && self.y < b.y + b.h
            && self.y + self.h > b.y
    }
    /// Compute the intersected clipping rectangle.
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
/// Positioned glyph in the display-list arena; coordinates are relative to its run origin.
#[derive(Clone, Copy, Serialize)]
pub struct Glyph {
    /// Resource or stable authored identity, according to the containing command.
    pub id: u32,
    /// Horizontal coordinate in logical pixels.
    pub x: f32,
    /// Vertical coordinate in logical pixels.
    pub y: f32,
}
/// Ordered renderer-neutral paint command; resources are handles rather than engine objects.
#[derive(Serialize)]
#[serde(tag = "op")]
pub enum Draw {
    /// Antialiased round-capped line in logical viewport coordinates.
    Line {
        /// Stable widget or connection identity.
        node: u64,
        /// First endpoint.
        from: [f32; 2],
        /// Second endpoint.
        to: [f32; 2],
        /// Full stroke width in logical pixels.
        width: f32,
        /// Straight working-space stroke color.
        color: Color,
    },
    /// Solid rectangular fill.
    Rect {
        /// Stable source node identity.
        node: u64,
        /// Computed logical viewport rectangle.
        rect: Box2,
        /// Foreground or glyph tint color.
        color: Color,
    },
    /// Analytic rounded rectangular fill.
    RoundedRect {
        /// Stable source node identity.
        node: u64,
        /// Computed logical viewport rectangle.
        rect: Box2,
        /// Clamped corner radius in logical pixels.
        radius: f32,
        /// Foreground or glyph tint color.
        color: Color,
    },
    /// Inset rounded border with independent thickness.
    Border {
        /// Stable source node identity.
        node: u64,
        /// Computed logical viewport rectangle.
        rect: Box2,
        /// Clamped corner radius in logical pixels.
        radius: f32,
        /// Last line-breaking width constraint in logical pixels.
        width: f32,
        /// Foreground or glyph tint color.
        color: Color,
    },
    /// Positioned shaped glyphs in a registered font resource.
    GlyphRun {
        /// Stable source node identity.
        node: u64,
        /// Registered font resource handle.
        font: u32,
        /// Logical viewport width and height.
        size: f32,
        /// Run origin in logical pixels.
        origin: [f32; 2],
        /// Contiguous positioned glyph arena addressed by command ranges.
        glyphs: Range<usize>,
        /// Foreground or glyph tint color.
        color: Color,
    },
    /// Cached outline-distance-field glyphs for continuously zoomable canvases.
    ScalableGlyphRun {
        /// Stable source node identity.
        node: u64,
        /// Registered font resource handle.
        font: u32,
        /// Logical font size after the view transform.
        size: f32,
        /// Logical run origin.
        origin: [f32; 2],
        /// Range in the containing glyph arena.
        glyphs: Range<usize>,
        /// Straight working-space tint.
        color: Color,
    },
    /// Decoded host image resource stretched to the destination rectangle.
    Image {
        /// Stable source node identity.
        node: u64,
        /// Computed logical viewport rectangle.
        rect: Box2,
        /// Registered image resource handle.
        image: u32,
    },
    /// Intersect a rectangular clip with the current clip stack.
    PushClip {
        /// Computed logical viewport rectangle.
        rect: Box2,
    },
    /// Rounded mask/stencil intersection; radius zero is equivalent to PushClip.
    PushRoundedClip {
        /// Logical clip rectangle.
        rect: Box2,
        /// Mask corner radius.
        radius: f32,
    },
    /// Restore the preceding clip.
    PopClip,
    /// Accumulate a paint translation without changing layout.
    PushTransform {
        /// Translation in logical pixels.
        translation: [f32; 2],
    },
    /// Restore the preceding paint translation.
    PopTransform,
}
/// Painter-ordered commands with one contiguous glyph arena.
#[derive(Default, Serialize)]
pub struct DisplayList {
    /// Painter-ordered commands, preserving transparency order.
    pub commands: Vec<Draw>,
    /// Contiguous positioned glyph arena addressed by command ranges.
    pub glyphs: Vec<Glyph>,
}
/// Conservative display-list batching boundary; the renderer reports actual atlas batches.
#[derive(Serialize)]
pub struct BatchBreak {
    /// Display-list command offset causing this boundary.
    pub command: usize,
    /// Human-readable batch break reason.
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
                Draw::ScalableGlyphRun { font, .. } => (1, *font),
                Draw::Image { image, .. } => (2, *image),
                Draw::Line { .. } => (3, 0),
                Draw::Rect { .. } | Draw::RoundedRect { .. } | Draw::Border { .. } => (0, 0),
                _ => {
                    breaks.push(BatchBreak {
                        command: i,
                        cause: match draw {
                            Draw::PushClip { .. }
                            | Draw::PushRoundedClip { .. }
                            | Draw::PopClip => "clip barrier",
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
}
