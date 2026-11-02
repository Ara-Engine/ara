//! Readout-only awake body volume and 25 cm media splats.
use super::*;

/// First body in stable identity order owns a shared volume sample, matching the
/// serial acquisition order even when independent samples run on other threads.
fn receiver_cells(scene: &Scene, bodies: &BTreeMap<u64, BodyClass>) -> Vec<(IVec3, u64)> {
    let mut cells = BTreeMap::new();
    for (&id, body) in bodies {
        if body.owns_links() {
            continue;
        }
        let instance = &scene.instances[id as usize];
        let bounds = instance.bounds(&scene.geometry[instance.geometry]);
        let lo = (bounds[0] / 0.25).floor().as_ivec3();
        let hi = (bounds[1] / 0.25).ceil().as_ivec3();
        for z in lo.z..=hi.z {
            for y in lo.y..=hi.y {
                for x in lo.x..=hi.x {
                    cells.entry([x, y, z]).or_insert(id);
                }
            }
        }
    }
    cells
        .into_iter()
        .map(|(cell, id)| (IVec3::from(cell), id))
        .collect()
}

struct Acquisition<'a> {
    cpu: CpuTrace<'a>,
    materials: NativeMaterials<'a>,
    surfaces: &'a SurfaceMap,
    radiance: &'a [Vec3],
    directions: u32,
    distance: f32,
}

impl Acquisition<'_> {
    /// Ray and accumulation order stay serial within each independent cell.
    fn sample(&self, cells: &[(IVec3, u64)], output: &mut [[Vec3; 6]]) {
        for (&(cell, id), lobes) in cells.iter().zip(output) {
            let position = cell.as_vec3() * 0.25;
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
                // Pixels already evaluate direct lighting. The volume contains
                // only environment and reflected radiance.
                let mut incoming = Vec3::ZERO;
                for (direction, weight) in
                    graph::directions(normal, id, self.directions, DirectionScheme::PerPatch)
                {
                    let mut ray = Ray {
                        origin: position,
                        direction,
                        t_min: 1e-4,
                        t_max: self.distance,
                    };
                    let mut transmission = Vec3::ONE;
                    let radiance = loop {
                        let hit = self.cpu.closest(ray, |h| {
                            h.id != id && !matches!(self.materials.optical(h), Optical::Ignore)
                        });
                        let Some(hit) = hit else {
                            break environment(self.materials.scene, direction);
                        };
                        if let Optical::Transmit(rgb) = self.materials.optical(hit) {
                            transmission *= rgb;
                            ray.t_min = f32::from_bits(hit.distance.to_bits() + 1);
                            continue;
                        }
                        break self
                            .surfaces
                            .patch(hit.id, hit.primitive, hit.barycentric)
                            .filter(|_| hit.front)
                            .map_or(Vec3::ZERO, |p| self.radiance[p as usize]);
                    };
                    incoming += transmission * radiance * (std::f32::consts::PI * weight);
                }
                lobes[axis] = incoming;
            }
        }
    }
}

impl PatchTransport {
    /// Acquisition is read-only against the preceding surface iterate. Workers
    /// own disjoint output cells; ordered publication preserves deterministic sums.
    fn receiver_samples(
        &self,
        scene: &Scene,
        textures: &[crate::scene::Texture],
        workers: usize,
    ) -> Vec<(IVec3, [Vec3; 6])> {
        let cells = receiver_cells(scene, &self.bodies);
        let mut samples = vec![[Vec3::ZERO; 6]; cells.len()];
        let acquisition = Acquisition {
            cpu: CpuTrace::new(&self.trace),
            materials: NativeMaterials {
                scene,
                textures,
                bodies: &self.bodies,
                generations: &self.geometry_generations,
            },
            surfaces: &self.surfaces,
            radiance: &self.solver.radiance,
            directions: self.options.directions,
            distance: self.distance,
        };
        let workers = workers.clamp(1, 8).min(cells.len().div_ceil(128).max(1));
        if workers == 1 {
            acquisition.sample(&cells, &mut samples);
        } else {
            let chunk = cells.len().div_ceil(workers);
            std::thread::scope(|scope| {
                for (cells, output) in cells.chunks(chunk).zip(samples.chunks_mut(chunk)) {
                    let acquisition = &acquisition;
                    scope.spawn(move || acquisition.sample(cells, output));
                }
            });
        }
        cells
            .into_iter()
            .zip(samples)
            .map(|((cell, _), sample)| (cell, sample))
            .collect()
    }

    /// Cold publications acquire derived volume/media before any pixel can read it.
    pub(super) fn initialize_receivers(
        &mut self,
        scene: &Scene,
        textures: &[crate::scene::Texture],
    ) -> Result<()> {
        self.update_receivers(scene, textures)?;
        self.graph.invalidate_all(self.frame);
        let cpu = CpuTrace::new(&self.trace);
        let materials = NativeMaterials {
            scene,
            textures,
            bodies: &self.bodies,
            generations: &self.geometry_generations,
        };
        let query = Query {
            trace: &cpu,
            surfaces: &self.surfaces,
            materials: &materials,
            media: &self.media,
            bodies: &self.body_sources,
            boundary: self.graph.links.patches.len() as u32,
        };
        self.graph
            .repair(&query, &self.solver.errors(), usize::MAX, self.frame + 1)?;
        self.refresh_forcing(scene, textures, None)?;
        for patch in 0..self.graph.links.patches.len() as u32 {
            self.solver
                .refresh_row(&self.graph.links, patch, self.frame);
        }
        let rows: Vec<_> = (0..self.graph.links.patches.len() as u32).collect();
        self.solver
            .update_contraction_rows(&self.graph.links, &rows)?;
        let stats = self.solver.run(
            &self.graph.links,
            &self.graph,
            Budget {
                patch_passes: usize::MAX,
                reverse_edges: usize::MAX,
                solution_tolerance: self.options.tolerance,
            },
        )?;
        self.report.patch_passes += stats.patch_passes;
        self.report.solution_bound = stats.solution_bound;
        Ok(())
    }
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
        let workers = std::thread::available_parallelism().map_or(1, usize::from);
        for (cell, sample) in self.receiver_samples(scene, textures, workers) {
            self.volume.insert(cell, sample);
        }
        let cpu = CpuTrace::new(&self.trace);
        let materials = NativeMaterials {
            scene,
            textures,
            bodies: &self.bodies,
            generations: &self.geometry_generations,
        };
        for (&id, body) in &self.bodies {
            if body.owns_links() {
                continue;
            }
            let instance = &scene.instances[id as usize];
            let bounds = instance.bounds(&scene.geometry[instance.geometry]);
            let centre = (bounds[0] + bounds[1]) * 0.5;
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
            .map(|n| {
                let face = centre + n * ((bounds[1] - bounds[0]) * 0.5 + Vec3::splat(1e-4));
                self.volume.sample(centre, n).unwrap_or(Vec3::ZERO)
                    + direct(scene, &cpu, &materials, face, n, self.distance)
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::transport::solve::tests::fixtures;

    #[test]
    fn parallel_receiver_acquisition_preserves_serial_bits_and_first_owner() -> Result<()> {
        let case = fixtures::Case {
            name: "avalanche-108",
            frames: 308,
            event: 8,
            bodies: 32,
            triangles: 108,
            lights: 1,
        };
        let mut fixture = fixtures::create(&case)?;
        let ids: Vec<_> = fixture.bodies.keys().copied().collect();
        fixture.scene.instances[ids[1] as usize].world =
            fixture.scene.instances[ids[0] as usize].world;
        let cells = receiver_cells(&fixture.scene, &fixture.bodies);
        assert!(cells.len() > 128, "exercise multiple acquisition workers");
        assert!(cells.iter().any(|(_, id)| *id == ids[0]));
        assert!(
            !cells.iter().any(|(_, id)| *id == ids[1]),
            "shared cells keep the first serial body owner"
        );
        let transport = PatchTransport::new(
            &fixture.scene,
            &fixture.textures,
            fixture.bodies,
            TransportOptions {
                directions: 4,
                ..TransportOptions::default()
            },
            None,
        )?;
        let serial = transport.receiver_samples(&fixture.scene, &fixture.textures, 1);
        let parallel = transport.receiver_samples(&fixture.scene, &fixture.textures, 8);
        let bits = |samples: Vec<(IVec3, [Vec3; 6])>| {
            samples
                .into_iter()
                .map(|(cell, lobes)| (cell, lobes.map(|v| v.to_array().map(f32::to_bits))))
                .collect::<Vec<_>>()
        };
        assert_eq!(bits(serial), bits(parallel));
        Ok(())
    }
}
