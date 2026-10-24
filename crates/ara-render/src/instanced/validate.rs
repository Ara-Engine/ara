use super::*;
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

impl Scene {
    /// Validates newly published content. This walks source attributes/topology;
    /// moving instances must not repeat content validation or source hashing.
    pub fn validate(&self) -> Result<()> {
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
        for geometry in &self.geometry {
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
                        && vertex.reserved == 0,
                    "invalid native vertex or culling bounds"
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
                    "invalid native LOD topology or geometric error"
                );
                previous_error = lod.error;
            }
        }
        let mut entities = BTreeSet::new();
        for instance in &self.instances {
            ensure!(
                entities.insert(instance.entity)
                    && instance.geometry < self.geometry.len()
                    && instance.material < self.materials.len(),
                "invalid or duplicated native instance"
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
        self.validate_parameters()
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
                    && extra.reserved == [0; 2]
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
