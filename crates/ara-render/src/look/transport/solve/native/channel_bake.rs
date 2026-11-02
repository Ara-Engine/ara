//! Offline optional channel admission. A response is never clipped to fit its
//! memory budget; an open-plan light with large support stays on the event path.
use super::super::channels::{Channel, Channels, Domain};
use super::*;
use std::collections::BTreeSet;

impl PatchTransport {
    pub(super) fn bake_channels(
        &mut self,
        scene: &Scene,
        textures: &[crate::scene::Texture],
    ) -> Result<()> {
        // Receiver/media coupling is not linear in a fixed surface operator.
        if self.bodies.values().any(|body| !body.owns_links()) {
            return Ok(());
        }
        let cpu = CpuTrace::new(&self.trace);
        let materials = NativeMaterials {
            scene,
            textures,
            bodies: &self.bodies,
            generations: &self.geometry_generations,
        };
        let count = self.graph.links.patches.len();
        for (id, light) in scene.lights.iter().enumerate() {
            let mut unit = *light;
            unit.color_lumens = [1.; 4];
            let mut response =
                EventSolver::new_albedo(&self.graph.links, self.solver.albedo.to_vec())?;
            let mut roots = Vec::new();
            let mut forcing = Vec::new();
            for p in 0..count as u32 {
                let position = Vec3::from(self.graph.links.patches.position(p));
                let normal = Vec3::from_array(
                    self.graph
                        .links
                        .patches
                        .normal
                        .each_ref()
                        .map(|c| c[p as usize]),
                );
                let value = response.albedo[p as usize]
                    * lamp(&unit, &cpu, &materials, position, normal)
                    / std::f32::consts::PI;
                if value.max_element() > 0. {
                    roots.push(p);
                    forcing.push((p, value));
                    response.set_forcing(p, value)?;
                }
            }
            // Local support is exact reachability, not a radius chosen to hide tails.
            let limit = (self.options.channel_bytes / 256).min(16_384);
            let Some(support) =
                Channels::support(&self.graph.links, &self.graph.reverse, &roots, limit)
            else {
                continue;
            };
            if support.is_empty() {
                continue;
            }
            let exact = response.exact_support(&self.graph.links, 1e-12, &support)?;
            let mut stamps = BTreeSet::new();
            let mut irradiance = Vec::with_capacity(support.len());
            for &p in &support {
                let owner = self.graph.links.patches.stamps[p as usize].id.instance;
                let i = &scene.instances[owner as usize];
                stamps.extend([
                    (Domain::Instance(owner), self.frame + 1),
                    (Domain::Geometry(i.geometry as u64), self.frame + 1),
                    (Domain::Material(i.material as u64), self.frame + 1),
                ]);
                let mut incoming = Vec3::ZERO;
                for edge in self.graph.links.receiver_links(p) {
                    let e = edge as usize;
                    let source = self.graph.links.source[e] as usize;
                    if source < count {
                        incoming += exact[source]
                            * Vec3::from_array(
                                self.graph.links.transmission.each_ref().map(|c| c[e]),
                            )
                            * self.graph.links.weight[e];
                    }
                }
                irradiance.push((p, incoming * std::f32::consts::PI));
            }
            let values = support.iter().map(|&p| (p, exact[p as usize])).collect();
            let admitted = self.channels.insert(
                Channel {
                    light: id as u64,
                    shape_generation: self.frame + 1,
                    support: stamps.into_iter().collect(),
                    values,
                    forcing,
                    irradiance,
                    valid: true,
                    truncated: false,
                    error_bound: 1e-9,
                },
                light_gain(light),
            )?;
            if !admitted {
                continue;
            }
            for &p in &support {
                self.solver.radiance[p as usize] -= exact[p as usize] * light_gain(light);
            }
        }
        // Re-establish the residual of the base operator after subtracting admitted
        // complete responses. Subsequent gain events update only their scalar rows.
        self.refresh_forcing(scene, textures, None)?;
        for p in 0..count as u32 {
            self.solver.refresh_row(&self.graph.links, p, self.frame);
        }
        Ok(())
    }
}
