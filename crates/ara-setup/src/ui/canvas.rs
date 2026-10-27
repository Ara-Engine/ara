//! Anti-aliased shapes on a 32-bit top-down DIB (`0xAARRGGBB` per pixel): rounded
//! rectangles, polygons and round-capped strokes, each from a signed distance at
//! the pixel centre. GDI draws the text afterwards on the same pixels.

/// An sRGB colour, `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u32);

impl Rgb {
    fn channels(self) -> [f32; 3] {
        [16, 8, 0].map(|shift| ((self.0 >> shift) & 0xff) as f32)
    }
    /// `self` moved toward `other` by `t` (0..=1).
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let (a, b) = (self.channels(), other.channels());
        let c = |i: usize| (a[i] + (b[i] - a[i]) * t).round().clamp(0.0, 255.0) as u32;
        Rgb((c(0) << 16) | (c(1) << 8) | c(2))
    }
    /// GDI `COLORREF` (`0x00BBGGRR`).
    pub fn colorref(self) -> u32 {
        ((self.0 & 0xff) << 16) | (self.0 & 0xff00) | ((self.0 >> 16) & 0xff)
    }
}

/// A rectangle in device pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl RectF {
    /// Shrinks every side by `by`.
    pub fn inset(self, by: f32) -> RectF {
        RectF {
            x: self.x + by,
            y: self.y + by,
            w: (self.w - 2.0 * by).max(0.0),
            h: (self.h - 2.0 * by).max(0.0),
        }
    }
    /// True when the point lies inside.
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// Signed distance from `(px, py)` to a rounded rectangle (negative inside).
pub fn rounded(px: f32, py: f32, rect: RectF, radius: f32) -> f32 {
    let radius = radius.min(rect.w / 2.0).min(rect.h / 2.0);
    let qx = (px - rect.x - rect.w / 2.0).abs() - rect.w / 2.0 + radius;
    let qy = (py - rect.y - rect.h / 2.0).abs() - rect.h / 2.0 + radius;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

fn segment(px: f32, py: f32, (ax, ay): Point, (bx, by): Point) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((px - ax) * dx + (py - ay) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (px - ax - t * dx).hypot(py - ay - t * dy)
}

fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

/// A point in an icon's units or in device pixels.
pub type Point = (f32, f32);

/// Lines and circle outlines of one icon, in its own units (Lucide: a 24 box).
#[derive(Clone, Copy, Debug)]
pub struct Strokes<'a> {
    /// Round-capped segments.
    pub lines: &'a [(Point, Point)],
    /// Circle outlines: centre and radius.
    pub circles: &'a [(Point, f32)],
    /// Stroke width.
    pub width: f32,
}

/// The pixels of a DIB section, and shape drawing over them.
pub struct Canvas<'a> {
    width: usize,
    height: usize,
    pixels: &'a mut [u32],
}

impl<'a> Canvas<'a> {
    /// Wraps `width * height` pixels.
    pub fn new(width: usize, height: usize, pixels: &'a mut [u32]) -> Self {
        debug_assert_eq!(pixels.len(), width * height);
        Self {
            width,
            height,
            pixels,
        }
    }
    /// Fills everything.
    pub fn clear(&mut self, color: Rgb) {
        self.pixels.fill(0xff00_0000 | color.0);
    }
    /// Blends `color` over the pixels of `bounds` by `cover(x, y)` at each pixel
    /// centre (0 = untouched, 1 = replaced).
    pub fn shade(&mut self, bounds: RectF, color: Rgb, cover: impl Fn(f32, f32) -> f32) {
        let x0 = bounds.x.floor().max(0.0) as usize;
        let y0 = bounds.y.floor().max(0.0) as usize;
        let x1 = ((bounds.x + bounds.w).ceil().max(0.0) as usize).min(self.width);
        let y1 = ((bounds.y + bounds.h).ceil().max(0.0) as usize).min(self.height);
        let source = color.channels();
        for y in y0..y1 {
            for x in x0..x1 {
                let alpha = cover(x as f32 + 0.5, y as f32 + 0.5);
                if alpha <= 0.0 {
                    continue;
                }
                let pixel = &mut self.pixels[y * self.width + x];
                let target = Rgb(*pixel & 0x00ff_ffff).channels();
                let c = |i: usize| {
                    (target[i] + (source[i] - target[i]) * alpha.min(1.0)).round() as u32
                };
                *pixel = 0xff00_0000 | (c(0) << 16) | (c(1) << 8) | c(2);
            }
        }
    }
    /// A filled rounded rectangle.
    pub fn round_rect(&mut self, rect: RectF, radius: f32, color: Rgb) {
        self.shade(rect.inset(-1.0), color, |x, y| {
            coverage(rounded(x, y, rect, radius))
        });
    }
    /// Strokes an icon scaled from its units into `rect`.
    pub fn strokes(&mut self, rect: RectF, units: f32, icon: Strokes<'_>, color: Rgb) {
        let scale = rect.w / units;
        let map = |(x, y): (f32, f32)| (rect.x + x * scale, rect.y + y * scale);
        let half = icon.width * scale / 2.0;
        self.shade(rect.inset(-1.0), color, |px, py| {
            let lines = icon
                .lines
                .iter()
                .map(|&(a, b)| segment(px, py, map(a), map(b)));
            let circles = icon.circles.iter().map(|&(centre, radius)| {
                let (cx, cy) = map(centre);
                ((px - cx).hypot(py - cy) - radius * scale).abs()
            });
            let distance = lines.chain(circles).fold(f32::MAX, f32::min);
            coverage(distance - half)
        });
    }
    /// Sets every pixel's alpha: 255 inside a rounded window outline, fading to 0
    /// outside (the Windows 11 corner shape, for offscreen previews).
    pub fn mask_corners(&mut self, radius: f32) {
        let rect = RectF {
            x: 0.0,
            y: 0.0,
            w: self.width as f32,
            h: self.height as f32,
        };
        for y in 0..self.height {
            for x in 0..self.width {
                let alpha = coverage(rounded(x as f32 + 0.5, y as f32 + 0.5, rect, radius));
                let pixel = &mut self.pixels[y * self.width + x];
                *pixel = (*pixel & 0x00ff_ffff) | (((alpha * 255.0).round() as u32) << 24);
            }
        }
    }
    /// Sets every pixel opaque (GDI text clears the alpha byte it touches).
    pub fn opaque(&mut self) {
        for pixel in self.pixels.iter_mut() {
            *pixel |= 0xff00_0000;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_cover_their_inside_and_fade_at_edges() {
        let mut pixels = vec![0; 40 * 20];
        let mut canvas = Canvas::new(40, 20, &mut pixels);
        canvas.clear(Rgb(0x000000));
        let rect = RectF {
            x: 4.0,
            y: 4.0,
            w: 32.0,
            h: 12.0,
        };
        canvas.round_rect(rect, 6.0, Rgb(0xffffff));
        assert_eq!(pixels[10 * 40 + 20] & 0xffffff, 0xffffff, "inside");
        assert_eq!(pixels[40 + 1] & 0xffffff, 0, "outside");
        let corner = pixels[4 * 40 + 4] & 0xff;
        assert!(
            corner < 0x80,
            "rounded corner is mostly outside: {corner:#x}"
        );
        assert_eq!(Rgb(0x000000).mix(Rgb(0xffffff), 0.5), Rgb(0x808080));
        assert_eq!(Rgb(0x112233).colorref(), 0x332211);
    }
}
