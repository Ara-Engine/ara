//! Readout-only awake body volume and 25 cm media splats.
use super::*;
impl PatchTransport {
    pub(super) fn update_receivers(
        &mut self,
        scene: &Scene,
        textures: &[crate::scene::Texture],
    ) -> Result<()> {
        self.media.begin_frame(self.frame + 1)?;
        for source in &self.media_sources {
            self.media.splat(
                Vec3::from(source.min),
                Vec3::from(source.max),
                source.occupancy,
                source.extinction,
                Vec3::from(source.albedo),
                Vec3::from(source.outgoing),
            )?;
        }
        self.volume.clear();
        self.body_radiance.clear();
        let cpu = CpuTrace::new(&self.trace);
        let materials = NativeMaterials {
            scene,
            textures,
            bodies: &self.bodies,
            generation: self.media.generation(),
        };
        for (&id, body) in &self.bodies {
            if body.owns_links() {
                continue;
            }
            let instance = &scene.instances[id as usize];
            let bounds = instance.bounds(&scene.geometry[instance.geometry]);
            let centre = (bounds[0] + bounds[1]) * 0.5;
            let lo = (bounds[0] / 0.25).floor().as_ivec3();
            let hi = (bounds[1] / 0.25).ceil().as_ivec3();
            for z in lo.z..=hi.z {
                for y in lo.y..=hi.y {
                    for x in lo.x..=hi.x {
                        let cell = IVec3::new(x, y, z);
                        if self.volume.contains(cell) {
                            continue;
                        }
                        let position = cell.as_vec3() * 0.25;
                        let mut lobes = [Vec3::ZERO; 6];
                        for (axis, normal) in [
                            Vec3::X,
                            Vec3::NEG_X,
                            Vec3::Y,
                            Vec3::NEG_Y,
                            Vec3::Z,
                            Vec3::NEG_Z,
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let mut incoming =
                                direct(scene, &cpu, &materials, position, normal, self.distance);
                            for (direction, weight) in graph::directions(
                                normal,
                                id,
                                self.options.directions,
                                DirectionScheme::PerPatch,
                            ) {
                                let hit = cpu.closest(
                                    Ray {
                                        origin: position,
                                        direction,
                                        t_min: 1e-4,
                                        t_max: self.distance,
                                    },
                                    |h| {
                                        h.id != id
                                            && !matches!(materials.optical(h), Optical::Ignore)
                                    },
                                );
                                let radiance = hit
                                    .and_then(|h| {
                                        self.surfaces.patch(h.id, h.primitive, h.barycentric)
                                    })
                                    .map_or_else(
                                        || {
                                            if hit.is_none() {
                                                environment(scene, direction)
                                            } else {
                                                Vec3::ZERO
                                            }
                                        },
                                        |p| self.solver.radiance[p as usize],
                                    );
                                incoming += radiance * (std::f32::consts::PI * weight);
                            }
                            lobes[axis] = incoming;
                        }
                        self.volume.insert(cell, lobes);
                    }
                }
            }
            let material = &scene.materials[instance.material];
            let albedo = Vec3::new(
                material.pbr.base_color[0],
                material.pbr.base_color[1],
                material.pbr.base_color[2],
            ) * (1. - material.pbr.surface[0]);
            let emission = Vec3::new(
                material.pbr.emissive_roughness[0],
                material.pbr.emissive_roughness[1],
                material.pbr.emissive_roughness[2],
            );
            let irradiance = [
                Vec3::X,
                Vec3::NEG_X,
                Vec3::Y,
                Vec3::NEG_Y,
                Vec3::Z,
                Vec3::NEG_Z,
            ]
            .into_iter()
            .filter_map(|n| self.volume.sample(centre, n))
            .sum::<Vec3>()
                / 6.;
            let outgoing = emission + albedo * irradiance / std::f32::consts::PI;
            self.body_radiance.insert(id, outgoing);
            if (bounds[1] - bounds[0]).max_element() < 0.5 {
                self.media
                    .splat(bounds[0], bounds[1], 1., 80., albedo, outgoing)?;
                if let Some(region) = Region::new(bounds[0].to_array(), bounds[1].to_array()) {
                    self.graph.discover(region, self.frame);
                }
            }
        }
        Ok(())
    }
}
