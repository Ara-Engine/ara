//! Content-boundary preparation. Geometry stays local; material maps stay compressed.
use anyhow::{Context, Result};
use ara_render::instanced::{self as render, Instance, Material, Scene};
use ara_runtime::{
    extract::{Draw, RenderFrame},
    material::MaterialAsset,
};
pub use ara_texture_stream::NativeConfig;
use ara_texture_stream::NativeResidency;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};

mod document;
mod geometry;
pub use document::{document_frame, document_frame_selected};
mod material;
mod pending;
pub use pending::when_published;
mod resolve;
mod sync;
pub use sync::FrameChange;
mod terrain;
#[cfg(test)]
mod tests;

/// Assets resolved from the workspace/library at a control boundary. No project
/// files, graph ASTs, import workers or scattering are accessed during presentation.
#[derive(Clone, Default)]
pub struct Content {
    /// Requested model assets for root fallback draws, keyed by draw address.
    /// These retain per-asset bounds materials and identify arrivals for fading.
    pub unpublished_models: BTreeMap<String, String>,
    /// Native surface assets keyed by the exact Header.materials reference.
    pub terrain_materials: BTreeMap<String, Arc<MaterialAsset>>,
    /// Model or prefab parts keyed by Rule.asset. Each Draw retains its selected
    /// primitive, source pivot, local part transform and material override.
    pub foliage_parts: BTreeMap<String, Vec<Draw>>,
    /// Model asset IDs behind unpublished foliage part draws, by rule reference
    /// and part identity. Presentation can change their bounds material in place.
    pub unpublished_foliage: BTreeMap<String, BTreeMap<u64, String>>,
    /// Optional linear equirectangular environment; otherwise use Environment.
    pub environment: Option<super::EnvironmentMap>,
}

/// Prepared CPU content and the corresponding compressed resident set. Publish
/// both together; a failed GPU replacement must retain the previously shown view.
pub struct Prepared {
    /// Shared source geometry, resolved materials and real instances.
    pub scene: Scene,
    /// Native BC mip tails, with no RGBA expansion or atlas generation.
    pub textures: NativeResidency,
    bindings: sync::Bindings,
    changes: render::Changes,
    model_sources: BTreeMap<usize, Arc<str>>,
    pub(crate) unpublished_materials: BTreeMap<String, Vec<usize>>,
}

struct Builder {
    scene: Scene,
    textures: NativeResidency,
    texture_keys: BTreeMap<(String, usize), u32>,
    geometry_keys: BTreeMap<[u8; 32], usize>,
    material_keys: BTreeMap<Vec<u8>, usize>,
    model_sources: BTreeMap<usize, Arc<str>>,
    model_assets: BTreeMap<String, Arc<str>>,
    unpublished: BTreeMap<String, BTreeMap<usize, usize>>,
}

/// Prepares one content generation. Repeated static draws share the original
/// model-space allocation; this operation is not the per-frame motion update.
pub fn prepare(frame: &RenderFrame, content: Content, config: NativeConfig) -> Result<Prepared> {
    ara_gpu::cpu_scope!("Native.prepare_scene");
    let (size, pixels) = crate::sky(&frame.environment);
    let (direction, lux) = frame.sun.unwrap_or(([0.3, 1.0, 0.2], [0.0; 3]));
    let mut builder = Builder {
        scene: Scene {
            geometry: Vec::new(),
            instances: Vec::new(),
            materials: Vec::new(),
            textures: 0,
            lights: super::lights(frame),
            sun_direction: glam30::Vec3::from_array(direction).normalize(),
            sun_lux: glam30::Vec3::from_array(lux),
            environment: pixels,
            environment_size: size,
        },
        textures: NativeResidency::new(config)?,
        texture_keys: BTreeMap::new(),
        geometry_keys: BTreeMap::new(),
        material_keys: BTreeMap::new(),
        model_sources: BTreeMap::new(),
        model_assets: BTreeMap::new(),
        unpublished: BTreeMap::new(),
    };
    for draw in &frame.draws {
        builder.draw_source(
            draw,
            frame,
            draw.world,
            draw.entity,
            content
                .unpublished_models
                .get(&draw.address)
                .map(String::as_str),
        )?;
    }
    if let Some(terrain) = &frame.terrain {
        builder.terrain(terrain, &content)?;
    }
    if let Some(environment) = content.environment {
        builder.scene.environment = environment.pixels;
        builder.scene.environment_size = environment.size;
    }
    builder.scene.validate()?;
    let bindings = sync::Bindings::new(frame, builder.texture_keys)?;
    Ok(Prepared {
        scene: builder.scene,
        textures: builder.textures,
        bindings,
        changes: render::Changes::default(),
        model_sources: builder.model_sources,
        unpublished_materials: builder
            .unpublished
            .into_iter()
            .map(|(asset, materials)| (asset, materials.into_values().collect()))
            .collect(),
    })
}

impl Builder {
    fn draw_source(
        &mut self,
        draw: &Draw,
        frame: &RenderFrame,
        world: glam::Mat4,
        entity: u64,
        missing: Option<&str>,
    ) -> Result<()> {
        let geometry = self.geometry(draw, frame)?;
        if let ara_runtime::extract::Geometry::Model { model, .. } = &draw.geometry {
            let source = if let Some(source) = self.model_assets.get(&model.asset) {
                Arc::clone(source)
            } else {
                let source: Arc<str> = Arc::from(model.asset.as_str());
                self.model_assets
                    .insert(model.asset.to_owned(), Arc::clone(&source));
                source
            };
            self.model_sources
                .insert(self.scene.instances.len(), source);
        }
        let mut material = self.material(draw)?;
        if let Some(asset) = missing {
            if let Some(&index) = self
                .unpublished
                .get(asset)
                .and_then(|entries| entries.get(&material))
            {
                material = index;
            } else {
                let index = self.scene.materials.len();
                self.scene.materials.push(self.scene.materials[material]);
                self.unpublished
                    .entry(asset.to_owned())
                    .or_default()
                    .insert(material, index);
                material = index;
            }
        }
        let world = glam30::Mat4::from_cols_array(&world.to_cols_array());
        self.scene.instances.push(Instance {
            entity,
            name: draw.label.to_owned(),
            geometry,
            material,
            world,
            previous_world: world,
        });
        Ok(())
    }

    fn add_material(&mut self, material: Material) -> usize {
        let key = bytemuck::bytes_of(&material);
        if let Some(&index) = self.material_keys.get(key) {
            return index;
        }
        let index = self.scene.materials.len();
        self.material_keys.insert(key.to_vec(), index);
        self.scene.materials.push(material);
        index
    }
}
