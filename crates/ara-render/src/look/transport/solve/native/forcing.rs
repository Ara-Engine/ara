//! Independent source quadrature; publication keeps the original patch order.
use super::*;

#[derive(Clone, Copy, Default)]
struct Forcing {
    albedo: Vec3,
    external: Vec3,
    value: Vec3,
}

impl PatchTransport {
    fn forcing_samples(
        &self,
        scene: &Scene,
        textures: &[crate::scene::Texture],
        rows: &[u32],
        workers: usize,
    ) -> Vec<Forcing> {
        let cpu = CpuTrace::new(&self.trace);
        let materials = NativeMaterials {
            scene,
            textures,
            bodies: &self.bodies,
            generations: &self.geometry_generations,
        };
        let sample = |rows: &[u32], output: &mut [Forcing]| {
            for (&patch, output) in rows.iter().zip(output) {
                let p = patch as usize;
                let position = Vec3::from(self.graph.links.patches.position(patch));
                let normal =
                    Vec3::from_array(self.graph.links.patches.normal.each_ref().map(|v| v[p]));
                let instance = self.graph.links.patches.stamps[p].id.instance as usize;
                let stamp = self.graph.links.patches.stamps[p];
                let (albedo, emission) = materials.patch(
                    instance as u64,
                    &self.charts[&(instance as u64)].charts,
                    stamp.id.local,
                );
                let direct = direct(scene, &cpu, &materials, position, normal, self.distance);
                let mut incident = Vec3::ZERO;
                for link in self.graph.links.receiver_links(patch) {
                    let i = link as usize;
                    if self.graph.links.source[i] as usize == self.graph.links.patches.len() {
                        let segment = self.graph.links.segment(link);
                        incident +=
                            environment(scene, (Vec3::from(segment.end) - position).normalize())
                                * Vec3::from_array(
                                    self.graph.links.transmission.each_ref().map(|v| v[i]),
                                )
                                * (std::f32::consts::PI * self.graph.links.weight[i]);
                    } else if self.graph.links.source[i] as usize > self.graph.links.patches.len() {
                        let source = &self.graph.links.external
                            [self.graph.links.source[i] as usize - self.graph.links.patches.len()];
                        incident += self
                            .body_radiance
                            .get(&source.id)
                            .copied()
                            .unwrap_or(Vec3::ZERO)
                            * Vec3::from_array(
                                self.graph.links.transmission.each_ref().map(|v| v[i]),
                            )
                            * (std::f32::consts::PI * self.graph.links.weight[i]);
                    }
                    incident +=
                        self.graph.media[i] * (std::f32::consts::PI * self.graph.links.weight[i]);
                }
                *output = Forcing {
                    albedo,
                    external: incident,
                    value: (emission + albedo * (incident + direct) / std::f32::consts::PI
                        - self.channels.forcing(patch))
                    .max(Vec3::ZERO),
                };
            }
        };
        let mut output = vec![Forcing::default(); rows.len()];
        let workers = workers.clamp(1, 8).min(rows.len().div_ceil(128).max(1));
        if workers == 1 {
            sample(rows, &mut output);
        } else {
            let chunk = rows.len().div_ceil(workers);
            std::thread::scope(|scope| {
                for (rows, output) in rows.chunks(chunk).zip(output.chunks_mut(chunk)) {
                    let sample = &sample;
                    scope.spawn(move || sample(rows, output));
                }
            });
        }
        output
    }

    /// Compute only selected rows. Each row retains its source/link accumulation
    /// order; serial publication retains scheduler ordering and existing debt.
    pub(super) fn refresh_forcing(
        &mut self,
        scene: &Scene,
        textures: &[crate::scene::Texture],
        rows: Option<&[u32]>,
    ) -> Result<()> {
        let all: Vec<_>;
        let rows = if let Some(rows) = rows {
            rows
        } else {
            all = (0..self.graph.links.patches.len() as u32).collect();
            &all
        };
        let workers = std::thread::available_parallelism().map_or(1, usize::from);
        for (&patch, sample) in rows
            .iter()
            .zip(self.forcing_samples(scene, textures, rows, workers))
        {
            let p = patch as usize;
            self.solver.albedo[p] = sample.albedo;
            self.external_irradiance[p] = sample.external;
            self.solver.set_forcing(patch, sample.value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::transport::solve::tests::fixtures;

    #[test]
    fn parallel_forcing_preserves_serial_bits_and_requested_row_order() -> Result<()> {
        let case = fixtures::Case {
            name: "200-light",
            frames: 308,
            event: 8,
            bodies: 0,
            triangles: 108,
            lights: 4,
        };
        let fixture = fixtures::create(&case)?;
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
        let rows: Vec<_> = (0..transport.graph.links.patches.len() as u32)
            .rev()
            .collect();
        assert!(rows.len() > 128);
        let bits = |samples: Vec<Forcing>| {
            samples
                .into_iter()
                .map(|s| [s.albedo, s.external, s.value].map(|v| v.to_array().map(f32::to_bits)))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            bits(transport.forcing_samples(&fixture.scene, &fixture.textures, &rows, 1)),
            bits(transport.forcing_samples(&fixture.scene, &fixture.textures, &rows, 8))
        );
        assert!(
            transport
                .forcing_samples(&fixture.scene, &fixture.textures, &[], 8)
                .is_empty()
        );
        Ok(())
    }
}
