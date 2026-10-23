//! CPU implementation of the GPU adapter contract, using a tiny-skia premultiplied target.
use crate::gpu::{Batch, Clip, Instance, Pipeline, TextureUpload, UiGpu};
use ara_ui::{Result, paint::Box2};
use std::collections::BTreeMap;
struct Texture {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}
/// Working CPU adapter for the exact atlas/instance/batch stream sent to `ara-gpu`.
#[derive(Default)]
pub struct CpuGpu {
    target: Option<tiny_skia::Pixmap>,
    textures: BTreeMap<u32, Texture>,
}
impl CpuGpu {
    /// Current target for golden comparison and capture.
    pub fn pixmap(&self) -> Result<&tiny_skia::Pixmap> {
        self.target
            .as_ref()
            .ok_or_else(|| "ARA7052: no CPU frame has been rendered".into())
    }
    /// Write the completed frame as PNG.
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        self.pixmap()?.save_png(path)?;
        Ok(())
    }
}
impl UiGpu for CpuGpu {
    fn upload(&mut self, t: TextureUpload<'_>) -> Result<()> {
        if t.width
            .checked_mul(t.height)
            .and_then(|n| n.checked_mul(4))
            .map(|n| n as usize)
            != Some(t.rgba.len())
        {
            return Err("ARA7052: invalid texture upload".into());
        }
        self.textures.insert(
            t.id,
            Texture {
                width: t.width,
                height: t.height,
                rgba: t.rgba.to_vec(),
            },
        );
        Ok(())
    }
    fn begin(&mut self, size: [u32; 2]) -> Result<()> {
        let resize = self
            .target
            .as_ref()
            .is_none_or(|p| p.width() != size[0] || p.height() != size[1]);
        if resize {
            self.target = Some(
                tiny_skia::Pixmap::new(size[0], size[1])
                    .ok_or("ARA7052: invalid target dimensions")?,
            );
        }
        if let Some(p) = &mut self.target {
            p.fill(tiny_skia::Color::TRANSPARENT);
        }
        Ok(())
    }
    fn draw(&mut self, batch: &Batch, instances: &[Instance], clips: &[Clip]) -> Result<()> {
        let target = self.target.as_mut().ok_or("ARA7052: draw before begin")?;
        let width = target.width();
        let height = target.height();
        let texture = if matches!(batch.pipeline, Pipeline::Shape | Pipeline::Line) {
            None
        } else {
            Some(
                self.textures
                    .get(&batch.texture)
                    .ok_or("ARA7052: texture not uploaded")?,
            )
        };
        if batch.clip >= clips.len() {
            return Err("ARA7052: invalid clip chain".into());
        }
        for i in instances {
            let r = Box2 {
                x: i.rect[0],
                y: i.rect[1],
                w: i.rect[2],
                h: i.rect[3],
            };
            if r.w <= 0.0 || r.h <= 0.0 {
                continue;
            }
            for y in (r.y.floor() as i32).max(0)..((r.y + r.h).ceil() as i32).min(height as i32) {
                for x in (r.x.floor() as i32).max(0)..((r.x + r.w).ceil() as i32).min(width as i32)
                {
                    let p = [x as f32 + 0.5, y as f32 + 0.5];
                    let mut clip = Some(batch.clip);
                    let mut visible = true;
                    while let Some(c) = clip {
                        let c = clips.get(c).ok_or("ARA7052: invalid clip parent")?;
                        if !c.rect.contains(p[0], p[1]) || distance(c.rect, c.radius, p) > 0.0 {
                            visible = false;
                            break;
                        }
                        clip = c.parent;
                    }
                    if !visible {
                        continue;
                    }
                    let mut color = i.color;
                    match batch.pipeline {
                        Pipeline::Line => {
                            let a = [r.x + i.uv[0], r.y + i.uv[1]];
                            let b = [r.x + i.uv[2], r.y + i.uv[3]];
                            let d = [b[0] - a[0], b[1] - a[1]];
                            let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1])
                                / (d[0] * d[0] + d[1] * d[1]).max(0.0001))
                            .clamp(0., 1.);
                            let distance = ((p[0] - a[0] - t * d[0]).powi(2)
                                + (p[1] - a[1] - t * d[1]).powi(2))
                            .sqrt();
                            color[3] *= (i.params[0] - distance + 0.5).clamp(0., 1.);
                        }
                        Pipeline::Shape => {
                            let outer = (0.5 - distance(r, i.params[0], p)).clamp(0.0, 1.0);
                            let coverage = if i.params[1] > 0.0 {
                                let border = i.params[1];
                                let inner = Box2 {
                                    x: r.x + border,
                                    y: r.y + border,
                                    w: (r.w - 2.0 * border).max(0.0),
                                    h: (r.h - 2.0 * border).max(0.0),
                                };
                                let hole = if inner.w > 0.0 && inner.h > 0.0 {
                                    (0.5 - distance(inner, (i.params[0] - border).max(0.0), p))
                                        .clamp(0.0, 1.0)
                                } else {
                                    0.0
                                };
                                outer - hole
                            } else {
                                outer
                            };
                            color[3] *= coverage;
                        }
                        pipeline => {
                            let t = texture.expect("textured pipeline resolved a texture");
                            let uv = [
                                i.uv[0] + ((p[0] - r.x) / r.w) * i.uv[2],
                                i.uv[1] + ((p[1] - r.y) / r.h) * i.uv[3],
                            ];
                            let tx = ((uv[0] * t.width as f32).floor() as i32)
                                .clamp(0, t.width as i32 - 1)
                                as u32;
                            let ty = ((uv[1] * t.height as f32).floor() as i32)
                                .clamp(0, t.height as i32 - 1)
                                as u32;
                            let offset = ((ty * t.width + tx) * 4) as usize;
                            let sample = [
                                t.rgba[offset],
                                t.rgba[offset + 1],
                                t.rgba[offset + 2],
                                t.rgba[offset + 3],
                            ]
                            .map(|v| v as f32 / 255.0);
                            match pipeline {
                                Pipeline::Mask => color[3] *= sample[3],
                                Pipeline::Msdf => {
                                    let mut channels = [sample[0], sample[1], sample[2]];
                                    channels.sort_by(f32::total_cmp);
                                    let screen_range =
                                        2.0 * i.params[2] * r.w / (i.uv[2] * t.width as f32);
                                    color[3] *=
                                        ((channels[1] - 0.5) * screen_range + 0.5).clamp(0.0, 1.0);
                                }
                                _ => {
                                    for c in 0..4 {
                                        color[c] *= sample[c];
                                    }
                                }
                            }
                        }
                    }
                    let offset = ((y as u32 * width + x as u32) * 4) as usize;
                    let data = target.data_mut();
                    let alpha = color[3].clamp(0.0, 1.0);
                    for channel in 0..3 {
                        data[offset + channel] = ((color[channel] * alpha
                            + data[offset + channel] as f32 / 255.0 * (1.0 - alpha))
                            * 255.0)
                            .round()
                            .clamp(0.0, 255.0)
                            as u8;
                    }
                    data[offset + 3] = ((alpha + data[offset + 3] as f32 / 255.0 * (1.0 - alpha))
                        * 255.0)
                        .round() as u8;
                }
            }
        }
        Ok(())
    }
    fn end(&mut self) -> Result<()> {
        Ok(())
    }
}
fn distance(r: Box2, radius: f32, p: [f32; 2]) -> f32 {
    let radius = radius.max(0.0).min(r.w / 2.0).min(r.h / 2.0);
    let q = [
        (p[0] - r.x - r.w / 2.0).abs() - r.w / 2.0 + radius,
        (p[1] - r.y - r.h / 2.0).abs() - r.h / 2.0 + radius,
    ];
    q[0].max(0.0).hypot(q[1].max(0.0)) + q[0].max(q[1]).min(0.0) - radius
}
