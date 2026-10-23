//! Minimal adapter contract: uploads, target begin, ordered batches, and target completion.
use crate::atlas::{GlyphAtlas, TextMode};
use ara_ui::{
    Result,
    paint::{Box2, DisplayList, Draw},
    style::Color,
};
use ara_ui_text::FontAsset;
use serde::Serialize;
use std::{collections::BTreeMap, ops::Range};
/// Shader entry mode; all modes preserve premultiplied-alpha painter order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[repr(u32)]
pub enum Pipeline {
    /// Analytic rounded rectangle and inset border SDF.
    Shape = 0,
    /// Straight-alpha RGBA image converted to premultiplied output.
    Image = 1,
    /// Hinted raster glyph alpha.
    Mask = 2,
    /// Three-channel scalable outline distance field.
    Msdf = 3,
    /// Color glyph, preserving authored glyph color.
    ColorGlyph = 4,
}
/// Exact 64-byte Slang instance ABI, four float4 records, alignment 16.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Instance {
    /// Logical target rectangle x/y/width/height.
    pub rect: [f32; 4],
    /// Atlas UV origin and extent.
    pub uv: [f32; 4],
    /// Linear or sRGB working-space straight color; see shader contract.
    pub color: [f32; 4],
    /// Radius, border width, MSDF pixel range, reserved.
    pub params: [f32; 4],
}
/// Clip arena node; rounded clips require stencil/mask intersection in the adapter.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Clip {
    /// Logical viewport rectangle, after translation.
    pub rect: Box2,
    /// Radius zero uses scissor; positive uses a stencil/mask.
    pub radius: f32,
    /// Ancestor clip index, absent at viewport root.
    pub parent: Option<usize>,
}
/// An adjacent batch never reorders transparent draws.
#[derive(Clone, Debug, Serialize)]
pub struct Batch {
    /// Shader mode.
    pub pipeline: Pipeline,
    /// Image/atlas handle; zero for untextured shapes.
    pub texture: u32,
    /// Clip arena index.
    pub clip: usize,
    /// Contiguous instance range.
    pub instances: Range<usize>,
    /// Reason this batch could not join the preceding one.
    pub break_cause: &'static str,
}
/// Borrowed complete RGBA texture update. Upload must complete/copy before returning.
pub struct TextureUpload<'a> {
    /// Opaque texture ID (high bit reserved for glyph pages).
    pub id: u32,
    /// Texture width.
    pub width: u32,
    /// Texture height.
    pub height: u32,
    /// Straight-alpha RGBA8, row-major.
    pub rgba: &'a [u8],
}
/// Host adapter seam. No layout, font shaping, source parsing, or widget decisions belong here.
pub trait UiGpu {
    /// Create or update a persistent texture by handle.
    fn upload(&mut self, texture: TextureUpload<'_>) -> Result<()>;
    /// Begin a transparent logical-pixel target; host sets viewport/DPI and composition policy.
    fn begin(&mut self, size: [u32; 2]) -> Result<()>;
    /// Submit one ordered batch, installing its intersected scissor/stencil clip chain.
    fn draw(&mut self, batch: &Batch, instances: &[Instance], clips: &[Clip]) -> Result<()>;
    /// End target commands and make it available for screen or world-surface composition.
    fn end(&mut self) -> Result<()>;
}
/// Owned image resource; source asset handles are resolved by the host.
pub struct Image {
    /// Width in source pixels.
    pub width: u32,
    /// Height in source pixels.
    pub height: u32,
    /// Straight-alpha RGBA8 bytes.
    pub rgba: Vec<u8>,
}
/// Persistent renderer state and frame scratch, reusable across unchanged retained frames.
#[derive(Default)]
pub struct Renderer {
    /// Persistent glyph atlas and cache.
    pub atlas: GlyphAtlas,
    /// Actual submitted batches and break reasons for inspection.
    pub batches: Vec<Batch>,
    /// Contiguous GPU-ready instances.
    pub instances: Vec<Instance>,
    /// Clip arena referenced by submitted batches.
    pub clips: Vec<Clip>,
    images: BTreeMap<u32, Image>,
    dirty_images: Vec<u32>,
}
impl Renderer {
    /// Invalidate device-owned textures after loss or when attaching a new adapter instance.
    pub fn device_lost(&mut self) {
        self.atlas.reset();
        self.dirty_images = self.images.keys().copied().collect();
    }
    /// Register/hot-reload a decoded image. Image handles cannot use the glyph-atlas high bit.
    pub fn image(&mut self, id: u32, image: Image) -> Result<()> {
        if id & 0x8000_0000 != 0
            || image.width == 0
            || image.height == 0
            || image
                .width
                .checked_mul(image.height)
                .and_then(|n| n.checked_mul(4))
                .map(|n| n as usize)
                != Some(image.rgba.len())
        {
            return Err("ARA7051: invalid image resource".into());
        }
        self.images.insert(id, image);
        if !self.dirty_images.contains(&id) {
            self.dirty_images.push(id);
        }
        Ok(())
    }
    fn push(&mut self, pipeline: Pipeline, texture: u32, clip: usize, instance: Instance) {
        let index = self.instances.len();
        self.instances.push(instance);
        if let Some(last) = self.batches.last_mut()
            && last.pipeline == pipeline
            && last.texture == texture
            && last.clip == clip
        {
            last.instances.end += 1;
            return;
        }
        let cause = self.batches.last().map_or("first draw", |last| {
            if last.clip != clip {
                "clip change"
            } else if last.pipeline != pipeline {
                "pipeline change"
            } else {
                "atlas/image change"
            }
        });
        self.batches.push(Batch {
            pipeline,
            texture,
            clip,
            instances: index..index + 1,
            break_cause: cause,
        });
    }
    /// Compile an ordered display list to batches, retaining glyph caches across frames.
    pub fn prepare(
        &mut self,
        list: &DisplayList,
        fonts: &[FontAsset],
        size: [u32; 2],
        mode: TextMode,
    ) -> Result<()> {
        if size[0] == 0 || size[1] == 0 {
            return Err("ARA7051: invalid render target".into());
        }
        self.batches.clear();
        self.instances.clear();
        self.clips.clear();
        self.clips.push(Clip {
            rect: Box2 {
                x: 0.0,
                y: 0.0,
                w: size[0] as f32,
                h: size[1] as f32,
            },
            radius: 0.0,
            parent: None,
        });
        let mut clip = 0;
        let mut transforms = vec![[0.0; 2]];
        for command in &list.commands {
            let t = *transforms.last().expect("root transform cannot be popped");
            let rect = |r: Box2| Box2 {
                x: r.x + t[0],
                y: r.y + t[1],
                ..r
            };
            let mut instance = Instance {
                rect: [0.0; 4],
                uv: [0.0, 0.0, 1.0, 1.0],
                color: [1.0; 4],
                params: [0.0; 4],
            };
            match command {
                Draw::PushTransform { translation } => {
                    transforms.push([t[0] + translation[0], t[1] + translation[1]]);
                }
                Draw::PopTransform => {
                    if transforms.len() == 1 {
                        return Err("ARA7051: unbalanced transform pop".into());
                    }
                    transforms.pop();
                }
                Draw::PushClip { rect: r } => {
                    self.clips.push(Clip {
                        rect: rect(*r),
                        radius: 0.0,
                        parent: Some(clip),
                    });
                    clip = self.clips.len() - 1;
                }
                Draw::PushRoundedClip { rect: r, radius } => {
                    self.clips.push(Clip {
                        rect: rect(*r),
                        radius: *radius,
                        parent: Some(clip),
                    });
                    clip = self.clips.len() - 1;
                }
                Draw::PopClip => {
                    clip = self.clips[clip]
                        .parent
                        .ok_or("ARA7051: unbalanced clip pop")?;
                }
                Draw::Rect { rect: r, color, .. }
                | Draw::RoundedRect { rect: r, color, .. }
                | Draw::Border { rect: r, color, .. } => {
                    instance.rect = array(rect(*r));
                    instance.color = color_array(*color);
                    match command {
                        Draw::RoundedRect { radius, .. } => instance.params[0] = *radius,
                        Draw::Border { radius, width, .. } => {
                            instance.params[0] = *radius;
                            instance.params[1] = *width;
                        }
                        _ => {}
                    }
                    self.push(Pipeline::Shape, 0, clip, instance);
                }
                Draw::Image { rect: r, image, .. } => {
                    if !self.images.contains_key(image) {
                        return Err(format!("ARA7051: missing image resource {image}").into());
                    }
                    instance.rect = array(rect(*r));
                    self.push(Pipeline::Image, *image, clip, instance);
                }
                Draw::GlyphRun {
                    font,
                    size,
                    origin,
                    glyphs,
                    color,
                    ..
                } => {
                    let font = fonts
                        .get(*font as usize)
                        .ok_or("ARA7051: unknown font resource")?;
                    let glyphs = list
                        .glyphs
                        .get(glyphs.clone())
                        .ok_or("ARA7051: invalid glyph arena range")?;
                    for g in glyphs {
                        if let Some(entry) = self.atlas.glyph(font, g.id, *size, mode)? {
                            instance.rect = [
                                origin[0] + g.x + t[0] + entry.offset[0],
                                origin[1] + g.y + t[1] + entry.offset[1],
                                entry.size[0],
                                entry.size[1],
                            ];
                            instance.uv = entry.uv;
                            instance.color = color_array(*color);
                            instance.params = [0.0, 0.0, entry.range, 0.0];
                            self.push(entry.pipeline, entry.texture, clip, instance);
                        }
                    }
                }
            }
        }
        if transforms.len() != 1 || clip != 0 {
            return Err("ARA7051: unclosed display-list stack".into());
        }
        Ok(())
    }
    /// Render through any adapter, including the CPU adapter used by the committed goldens.
    pub fn render(
        &mut self,
        gpu: &mut impl UiGpu,
        list: &DisplayList,
        fonts: &[FontAsset],
        size: [u32; 2],
        mode: TextMode,
    ) -> Result<()> {
        self.prepare(list, fonts, size, mode)?;
        for id in &self.dirty_images {
            let image = &self.images[id];
            gpu.upload(TextureUpload {
                id: *id,
                width: image.width,
                height: image.height,
                rgba: &image.rgba,
            })?;
        }
        self.dirty_images.clear();
        self.atlas.uploads(gpu)?;
        gpu.begin(size)?;
        for batch in &self.batches {
            gpu.draw(batch, &self.instances[batch.instances.clone()], &self.clips)?;
        }
        gpu.end()
    }
}
fn array(r: Box2) -> [f32; 4] {
    [r.x, r.y, r.w, r.h]
}
fn color_array(c: Color) -> [f32; 4] {
    [c.0, c.1, c.2, c.3].map(|v| v as f32 / 255.0)
}
