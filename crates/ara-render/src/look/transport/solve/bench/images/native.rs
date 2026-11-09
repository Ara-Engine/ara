//! Native-instance capture for texture-free G3 fixtures. Shared source geometry
//! stays resident during motion; physical oracles still use the full geometry.
use super::{Fixture, Renderer, Result};
use anyhow::ensure;
use ara_gpu::{Access, GpuDevice, Resource};
use ara_render::instanced::{self, Changes, TextureStats, Textures};
use std::path::Path;

struct NoTextures;
impl Textures for NoTextures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn refine(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn binding(&self) -> Option<instanced::TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut GpuDevice) -> Result<()> {
        Ok(())
    }
}

pub(super) struct CaptureScene {
    layout: (usize, usize),
}

impl CaptureScene {
    pub fn new(f: &Fixture, out: &Path, size: [u32; 2]) -> Result<(Renderer, Self)> {
        ensure!(
            f.textures.is_empty()
                && f.scene.materials.iter().all(|m| {
                    m.pbr
                        .textures
                        .iter()
                        .all(|&texture| texture == instanced::NO_TEXTURE)
                }),
            "native G3 captures require texture-free fixtures"
        );
        let renderer = Renderer::new_native(
            &f.scene,
            Box::new(NoTextures),
            size,
            &[],
            out.join("gpu-native"),
        )?;
        Ok((
            renderer,
            Self {
                layout: (f.scene.geometry.len(), f.scene.instances.len()),
            },
        ))
    }

    pub fn update(
        &mut self,
        renderer: &mut Renderer,
        f: &Fixture,
        changes: &Changes,
    ) -> Result<()> {
        let layout = (f.scene.geometry.len(), f.scene.instances.len());
        if layout != self.layout || !changes.geometries.is_empty() {
            renderer.replace_native_scene(&f.scene, Box::new(NoTextures))?;
            self.layout = layout;
        } else {
            renderer.update_native_changed(&f.scene, changes)?;
        }
        Ok(())
    }
}
