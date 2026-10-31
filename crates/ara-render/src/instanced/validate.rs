use super::*;
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

/// A checked immutable borrow, shared by the publication and GPU upload layers.
#[derive(Clone, Copy)]
pub(crate) struct ValidatedScene<'a>(&'a Scene);

/// Owns the exact validated scene; no mutable access can invalidate the proof
/// while a worker packs or allocates its publication.
pub(crate) struct CheckedScene(Scene);

impl CheckedScene {
    pub(crate) fn new(scene: Scene) -> Result<Self> {
        scene.validate()?;
        Ok(Self(scene))
    }
    pub(crate) fn validated(&self) -> ValidatedScene<'_> {
        ValidatedScene(&self.0)
    }
    pub(crate) fn into_scene(self) -> Scene {
        self.0
    }
}

impl<'a> ValidatedScene<'a> {
    pub(crate) fn scene(self) -> &'a Scene {
        self.0
    }
}

impl Scene {
    pub(crate) fn validated(&self) -> Result<ValidatedScene<'_>> {
        self.validate()?;
        Ok(ValidatedScene(self))
    }

    /// Validates newly published content. This walks source attributes/topology;
    /// moving instances must not repeat content validation or source hashing.
    pub fn validate(&self) -> Result<()> {
        ara_gpu::cpu_scope!("NativeScene.validate_content");
        ensure!(
            self.instances.len() <= 0x1000000,
            "native instances exceed the 24-bit ray-query ABI"
        );
        ensure!(
            !self.environment_size.contains(&0)
                && self.environment.len() as u64
                    == u64::from(self.environment_size[0]) * u64::from(self.environment_size[1])
                && self
                    .environment
                    .iter()
                    .flatten()
                    .all(|v| v.is_finite() && *v >= 0.0),
            "invalid native environment"
        );
        ensure!(
            self.sun_direction.is_finite()
                && self.sun_direction.length_squared() > 1e-8
                && self.sun_lux.is_finite()
                && self.sun_lux.cmpge(Vec3::ZERO).all(),
            "invalid native sun"
        );
        let mut keys = BTreeSet::new();
        for (geometry_index, geometry) in self.geometry.iter().enumerate() {
            ensure!(
                keys.insert(geometry.key)
                    && !geometry.vertices.is_empty()
                    && !geometry.lods.is_empty()
                    && geometry.bounds.iter().all(|v| v.is_finite())
                    && geometry.bounds[0].cmple(geometry.bounds[1]).all(),
                "invalid or duplicated native geometry"
            );
            for vertex in &geometry.vertices {
                let position = Vec3::from_array(vertex.position);
                ensure!(
                    position.is_finite()
                        && position.cmpge(geometry.bounds[0] - Vec3::splat(1e-4)).all()
                        && position.cmple(geometry.bounds[1] + Vec3::splat(1e-4)).all()
                        && vertex
                            .normal
                            .iter()
                            .chain(&vertex.uv)
                            .chain(&vertex.uv1)
                            .chain(&vertex.tangent)
                            .all(|v| v.is_finite())
                        && Vec3::from_array(vertex.normal).length_squared() > 1e-8
                        && Vec3::from_slice(&vertex.tangent).length_squared() > 1e-8
                        && (geometry.vertex_colors || vertex.color == Vertex::WHITE),
                    "invalid native vertex, culling bounds or undeclared vertex colours"
                );
            }
            let mut previous_error = 0.0;
            for (index, lod) in geometry.lods.iter().enumerate() {
                ensure!(
                    !lod.indices.is_empty()
                        && lod.indices.len().is_multiple_of(3)
                        && lod
                            .indices
                            .iter()
                            .all(|i| (*i as usize) < geometry.vertices.len())
                        && lod.error.is_finite()
                        && lod.error >= previous_error
                        && (index != 0 || lod.error == 0.0),
                    "invalid native LOD topology or geometric error: {} (geometry {geometry_index}, LOD {index}, error {}, previous {previous_error})",
                    self.instances
                        .iter()
                        .find(|instance| instance.geometry == geometry_index)
                        .map_or("uninstanced", |instance| instance.name.as_str()),
                    lod.error
                );
                previous_error = lod.error;
            }
        }
        let mut entities = BTreeSet::new();
        for instance in &self.instances {
            ensure!(
                entities.insert(instance.entity)
                    && instance.geometry < self.geometry.len()
                    && instance.material < self.materials.len()
                    && instance.paint.as_ref().is_none_or(|paint| {
                        paint.len() == self.geometry[instance.geometry].vertices.len()
                    }),
                "invalid or duplicated native instance, or paint not one colour per vertex"
            );
            for world in [instance.world, instance.previous_world] {
                ensure!(
                    world.is_finite()
                        && world.x_axis.w.abs() < 1e-6
                        && world.y_axis.w.abs() < 1e-6
                        && world.z_axis.w.abs() < 1e-6
                        && (world.w_axis.w - 1.0).abs() < 1e-6,
                    "native instance transform must be finite and affine"
                );
            }
        }
        self.validate_decals()?;
        self.validate_parameters()
    }

    /// Checks decal boxes and parameters (also on every streamed decal edit).
    pub fn validate_decals(&self) -> Result<()> {
        for decal in &self.decals {
            let w = decal.world;
            ensure!(
                w.is_finite()
                    && w.x_axis.w.abs() < 1e-6
                    && w.y_axis.w.abs() < 1e-6
                    && w.z_axis.w.abs() < 1e-6
                    && (w.w_axis.w - 1.0).abs() < 1e-6
                    && w.determinant().abs() > 1e-12,
                "native decal `{}` box must be finite, affine and invertible",
                decal.name
            );
            ensure!(
                decal.material < self.materials.len()
                    && decal.opacity.iter().all(|o| (0.0..=1.0).contains(o))
                    && decal.opacity_channel <= 4
                    && decal.occlusion[1] <= 4.0
                    && decal.occlusion[0].is_finite()
                    && decal.specular.is_finite()
                    && decal
                        .fade_distance
                        .iter()
                        .all(|d| d.is_finite() && *d >= 0.0)
                    && decal.fade_angle.iter().all(|a| (0.0..=180.0).contains(a))
                    && (0.0..=1.0).contains(&decal.fade_screen_size),
                "invalid native decal `{}`",
                decal.name
            );
        }
        Ok(())
    }

    /// Checks mutable surface/light parameters without rewalking immutable vertex,
    /// index, environment or identity data. Views validate their cached pose layout.
    pub fn validate_parameters(&self) -> Result<()> {
        ensure!(
            self.sun_direction.is_finite()
                && self.sun_direction.length_squared() > 1e-8
                && self.sun_lux.is_finite()
                && self.sun_lux.cmpge(Vec3::ZERO).all(),
            "invalid native sun"
        );
        for material in &self.materials {
            let pbr = &material.pbr;
            let extra = &material.extras;
            ensure!(
                pbr.base_color.iter().all(|v| (0.0..=1.0).contains(v))
                    && pbr.emissive_roughness[..3]
                        .iter()
                        .all(|v| v.is_finite() && *v >= 0.0)
                    && (0.0..=1.0).contains(&pbr.emissive_roughness[3])
                    && pbr.surface.iter().all(|v| v.is_finite())
                    && (0.0..=1.0).contains(&pbr.surface[0])
                    && (0.0..=1.0).contains(&pbr.surface[3])
                    && pbr.flags[0] <= 2,
                "invalid native PBR factors"
            );
            for texture in pbr
                .textures
                .into_iter()
                .chain([extra.occlusion_texture, extra.transmission_texture])
            {
                ensure!(
                    texture == NO_TEXTURE || texture < self.textures,
                    "native material texture ID out of range"
                );
            }
            for uv in &extra.uv {
                ensure!(
                    uv.offset
                        .iter()
                        .chain(&uv.scale)
                        .chain([&uv.rotation])
                        .all(|v| v.is_finite())
                        && uv.tex_coord <= 1
                        && uv.reserved == [0; 2],
                    "invalid native texture transform"
                );
            }
            ensure!(
                extra.terrain_layers <= 4
                    && extra.reconstruct_normal_basis <= 1
                    // A finite thickness half (Volume::pack); every IOR and RGB9E5 code is valid.
                    && (extra.volume[0] & 0xffff) < 0x7c00
                    && (0.0..=1.0).contains(&extra.occlusion_strength)
                    && (0.0..=1.0).contains(&extra.transmission_factor),
                "invalid native material extensions"
            );
            for (channel, &index) in extra.layer_materials.iter().enumerate() {
                ensure!(
                    if channel < extra.terrain_layers as usize {
                        self.materials
                            .get(index as usize)
                            .is_some_and(|m| m.extras.terrain_layers == 0)
                    } else {
                        index == NO_TEXTURE
                    },
                    "invalid or nested native terrain material"
                );
            }
        }
        for light in &self.lights {
            ensure!(
                light
                    .position_range
                    .iter()
                    .chain(&light.color_lumens)
                    .chain(&light.direction_outer)
                    .chain(&light.parameters)
                    .all(|v| v.is_finite())
                    && light.position_range[3] > 0.0
                    && light.color_lumens.iter().all(|v| *v >= 0.0)
                    && (light.parameters[0] == 0.0 || light.parameters[0] == 1.0),
                "invalid native light"
            );
            if light.parameters[0] == 1.0 {
                ensure!(
                    Vec3::from_slice(&light.direction_outer).length_squared() > 1e-8
                        && (-1.0..=1.0).contains(&light.direction_outer[3])
                        && (light.direction_outer[3]..=1.0).contains(&light.parameters[1]),
                    "invalid native spot cone"
                );
            }
        }
        Ok(())
    }
}
