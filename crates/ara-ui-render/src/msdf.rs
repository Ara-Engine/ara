//! Three edge-color distance channels from flattened font outlines (not a raster SDF).
use ara_ui::Result;
use swash::zeno::{Command, PathData};
pub(crate) struct Image {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
}
struct Edge {
    a: [f32; 2],
    b: [f32; 2],
    color: usize,
}
fn line(edges: &mut Vec<Edge>, a: [f32; 2], b: [f32; 2], color: usize) {
    if a != b {
        edges.push(Edge { a, b, color });
    }
}
pub(crate) fn generate(outline: &swash::scale::outline::Outline, range: f32) -> Result<Image> {
    let bounds = outline.bounds();
    let left = (bounds.min.x - range).floor() as i32;
    let top = (bounds.max.y + range).ceil() as i32;
    let width = (bounds.max.x + range).ceil() as i32 - left;
    let height = top - (bounds.min.y - range).floor() as i32;
    if width <= 0 || height <= 0 || width > 4096 || height > 4096 {
        return Err("ARA4050: invalid scalable glyph bounds".into());
    }
    let mut edges = Vec::new();
    let mut current = [0.0; 2];
    let mut start = current;
    let mut color = 0;
    for command in outline.path().commands() {
        match command {
            Command::MoveTo(p) => {
                current = [p.x, p.y];
                start = current;
            }
            Command::LineTo(p) => {
                let to = [p.x, p.y];
                line(&mut edges, current, to, color);
                current = to;
                color = (color + 1) % 3;
            }
            Command::QuadTo(c, p) => {
                let from = current;
                for step in 1..=16 {
                    let t = step as f32 / 16.0;
                    let u = 1.0 - t;
                    let to = [
                        u * u * from[0] + 2.0 * u * t * c.x + t * t * p.x,
                        u * u * from[1] + 2.0 * u * t * c.y + t * t * p.y,
                    ];
                    line(&mut edges, current, to, color);
                    current = to;
                }
                color = (color + 1) % 3;
            }
            Command::CurveTo(a, b, p) => {
                let from = current;
                for step in 1..=24 {
                    let t = step as f32 / 24.0;
                    let u = 1.0 - t;
                    let to = [
                        u * u * u * from[0]
                            + 3.0 * u * u * t * a.x
                            + 3.0 * u * t * t * b.x
                            + t * t * t * p.x,
                        u * u * u * from[1]
                            + 3.0 * u * u * t * a.y
                            + 3.0 * u * t * t * b.y
                            + t * t * t * p.y,
                    ];
                    line(&mut edges, current, to, color);
                    current = to;
                }
                color = (color + 1) % 3;
            }
            Command::Close => {
                line(&mut edges, current, start, color);
                current = start;
                color = (color + 1) % 3;
            }
        }
    }
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let p = [left as f32 + x as f32 + 0.5, top as f32 - y as f32 - 0.5];
            let mut distances = [f32::INFINITY; 3];
            let mut winding = 0i32;
            for e in &edges {
                let d = [e.b[0] - e.a[0], e.b[1] - e.a[1]];
                let v = [p[0] - e.a[0], p[1] - e.a[1]];
                let t = ((v[0] * d[0] + v[1] * d[1]) / (d[0] * d[0] + d[1] * d[1])).clamp(0.0, 1.0);
                let distance = ((v[0] - t * d[0]).powi(2) + (v[1] - t * d[1]).powi(2)).sqrt();
                // Each edge colors two channels; the median reconstructs a sharp boundary.
                for (i, channel) in distances.iter_mut().enumerate() {
                    if i != e.color {
                        *channel = channel.min(distance);
                    }
                }
                let cross = d[0] * v[1] - d[1] * v[0];
                if e.a[1] <= p[1] && e.b[1] > p[1] && cross > 0.0 {
                    winding += 1;
                }
                if e.a[1] > p[1] && e.b[1] <= p[1] && cross < 0.0 {
                    winding -= 1;
                }
            }
            let sign = if winding != 0 { 1.0 } else { -1.0 };
            for d in distances {
                pixels
                    .push(((0.5 + sign * d / (2.0 * range)).clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            pixels.push(255);
        }
    }
    Ok(Image {
        pixels,
        width: width as u32,
        height: height as u32,
        left,
        top,
    })
}
