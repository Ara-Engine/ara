//! Glint's UI icons, drawn from signed-distance shapes (no third-party art): an orb, a flag, a
//! fall arrow and three medals. Written as small PNGs into `content/ui/icons/`.
use anyhow::Result;
use image::{Rgba, RgbaImage};
use std::path::Path;

type Color = [f32; 4];

const NAVY: Color = [0.06, 0.16, 0.33, 1.0];
const CORAL: Color = [1.0, 0.36, 0.14, 1.0];

/// A shape: signed distance in pixels (negative inside) and the colour it paints.
struct Shape {
    distance: Box<dyn Fn(f32, f32) -> f32>,
    color: Color,
}

fn circle(cx: f32, cy: f32, r: f32, color: Color) -> Shape {
    Shape {
        distance: Box::new(move |x, y| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r),
        color,
    }
}

fn ring(cx: f32, cy: f32, r: f32, width: f32, color: Color) -> Shape {
    Shape {
        distance: Box::new(move |x, y| {
            (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).abs() - width * 0.5
        }),
        color,
    }
}

fn rect(cx: f32, cy: f32, hx: f32, hy: f32, color: Color) -> Shape {
    Shape {
        distance: Box::new(move |x, y| {
            let (dx, dy) = ((x - cx).abs() - hx, (y - cy).abs() - hy);
            dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0)
        }),
        color,
    }
}

fn triangle(a: (f32, f32), b: (f32, f32), c: (f32, f32), color: Color) -> Shape {
    // Distance to each edge's line, signed by which side the centroid is on (any winding).
    let edge = move |p: (f32, f32), q: (f32, f32), x: f32, y: f32| {
        let (ex, ey) = (q.0 - p.0, q.1 - p.1);
        ((x - p.0) * ey - (y - p.1) * ex) / ex.hypot(ey)
    };
    let (gx, gy) = ((a.0 + b.0 + c.0) / 3.0, (a.1 + b.1 + c.1) / 3.0);
    let side = if edge(a, b, gx, gy)
        .max(edge(b, c, gx, gy))
        .max(edge(c, a, gx, gy))
        > 0.0
    {
        -1.0
    } else {
        1.0
    };
    Shape {
        distance: Box::new(move |x, y| {
            (side * edge(a, b, x, y))
                .max(side * edge(b, c, x, y))
                .max(side * edge(c, a, x, y))
        }),
        color,
    }
}

fn paint(size: u32, shapes: &[Shape]) -> RgbaImage {
    let mut image = RgbaImage::new(size, size);
    let samples = 4;
    for py in 0..size {
        for px in 0..size {
            let mut accumulated = [0f32; 4];
            for sy in 0..samples {
                for sx in 0..samples {
                    let x = px as f32 + (sx as f32 + 0.5) / samples as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / samples as f32;
                    let mut out = [0f32; 4];
                    for shape in shapes {
                        // Half a pixel of feather keeps edges smooth after the supersample.
                        let coverage =
                            ((0.5 - (shape.distance)(x, y)) / 1.0).clamp(0.0, 1.0) * shape.color[3];
                        for (channel, color) in out.iter_mut().zip(&shape.color[..3]) {
                            *channel = color * coverage + *channel * (1.0 - coverage);
                        }
                        out[3] = coverage + out[3] * (1.0 - coverage);
                    }
                    for (sum, channel) in accumulated.iter_mut().zip(out) {
                        *sum += channel;
                    }
                }
            }
            let n = (samples * samples) as f32;
            let a = accumulated[3] / n;
            let rgb = |c: usize| {
                if a > 0.0 {
                    (accumulated[c] / n / a).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            };
            image.put_pixel(
                px,
                py,
                Rgba([
                    (rgb(0) * 255.0) as u8,
                    (rgb(1) * 255.0) as u8,
                    (rgb(2) * 255.0) as u8,
                    (a * 255.0) as u8,
                ]),
            );
        }
    }
    image
}

fn medal(rim: Color, face: Color) -> Vec<Shape> {
    vec![
        // Ribbon tails.
        triangle(
            (38.0, 6.0),
            (58.0, 6.0),
            (48.0, 44.0),
            [0.1, 0.45, 0.85, 1.0],
        ),
        triangle(
            (58.0, 6.0),
            (78.0, 6.0),
            (48.0, 44.0),
            [0.95, 0.95, 0.98, 1.0],
        ),
        circle(48.0, 62.0, 30.0, rim),
        circle(48.0, 62.0, 25.0, face),
        ring(48.0, 62.0, 17.0, 3.0, rim),
        // A star-ish pip: a small diamond.
        triangle((48.0, 50.0), (40.0, 62.0), (56.0, 62.0), rim),
        triangle((40.0, 62.0), (48.0, 74.0), (56.0, 62.0), rim),
    ]
}

/// Writes every icon into `dir`.
pub fn write(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let orb = vec![
        circle(32.0, 32.0, 24.0, CORAL),
        ring(32.0, 32.0, 24.0, 3.0, NAVY),
        circle(24.0, 24.0, 6.0, [1.0, 1.0, 1.0, 0.8]),
    ];
    paint(64, &orb).save(dir.join("orb.png"))?;
    let flag = vec![
        rect(18.0, 34.0, 3.0, 26.0, NAVY),
        triangle((21.0, 10.0), (21.0, 34.0), (52.0, 22.0), CORAL),
        circle(18.0, 8.0, 5.0, NAVY),
    ];
    paint(64, &flag).save(dir.join("flag.png"))?;
    let fall = vec![
        rect(32.0, 22.0, 6.0, 14.0, NAVY),
        triangle((14.0, 34.0), (50.0, 34.0), (32.0, 58.0), NAVY),
    ];
    paint(64, &fall).save(dir.join("fall.png"))?;
    paint(96, &medal([0.98, 0.72, 0.1, 1.0], [1.0, 0.86, 0.35, 1.0]))
        .save(dir.join("medal_gold.png"))?;
    paint(96, &medal([0.62, 0.68, 0.76, 1.0], [0.86, 0.9, 0.96, 1.0]))
        .save(dir.join("medal_silver.png"))?;
    paint(96, &medal([0.62, 0.36, 0.18, 1.0], [0.86, 0.55, 0.3, 1.0]))
        .save(dir.join("medal_bronze.png"))?;
    Ok(())
}
