//! Native scene synchronization. Mesh hashing/building is confined to publication
//! or changed geometry revisions; ordinary motion only refits instance bounds.
use crate::{instanced, look::trace};
use anyhow::Result;
use glam::Affine3A;

pub(crate) struct Scene {
    pub trace: trace::TraceScene,
    meshes: Vec<([u8; 32], u64, ara_bvh::MeshHash)>,
    instances: Vec<(usize, usize, u8)>,
}

impl Scene {
    pub fn new() -> Self {
        Self {
            trace: trace::TraceScene::default(),
            meshes: Vec::new(),
            instances: Vec::new(),
        }
    }

    /// Returns true when publication replaced the trace scene's version domain.
    pub fn update(&mut self, scene: &instanced::Scene, cache: &trace::BlasCache) -> Result<bool> {
        // A publication may reorder rows; user always names this generation's native row.
        let replaced = self.meshes.len() != scene.geometry.len()
            || self.instances.len() != scene.instances.len();
        if replaced {
            *self = Self::new();
        }
        let mut meshes_changed = false;
        for (i, g) in scene.geometry.iter().enumerate() {
            if self
                .meshes
                .get(i)
                .is_some_and(|&(key, revision, _)| key == g.key && revision == g.revision)
            {
                continue;
            }
            // UVs keep the native residency alpha callback enabled for every mesh.
            let (mesh, _) = trace::mesh_from_geometry(g, Some(cache), true)?;
            let hash = mesh.blas.hash;
            self.trace.add_mesh(mesh);
            meshes_changed = true;
            if i == self.meshes.len() {
                self.meshes.push((g.key, g.revision, hash));
            } else {
                self.meshes[i] = (g.key, g.revision, hash);
            }
            for state in &mut self.instances {
                if state.0 == i {
                    state.0 = usize::MAX;
                }
            }
        }
        for (i, material) in scene.materials.iter().enumerate() {
            self.trace.set_alpha_material(
                i as u32,
                (material.pbr.flags[0] == 1).then_some(trace::AlphaMaterial {
                    texture: 0,
                    uv_set: 0,
                    cutoff: 0.5,
                    factor: 1.,
                    offset: [0.; 2],
                    scale: [1.; 2],
                    rotation: 0.,
                }),
            );
        }
        for (i, instance) in scene.instances.iter().enumerate() {
            let material = &scene.materials[instance.material];
            let transparent =
                material.pbr.flags[0] == 2 || material.extras.transmission_factor > 0.;
            let masked = material.pbr.flags[0] == 1;
            let mask = match (transparent, masked, instance.casts_shadows) {
                (true, _, true) => 4,
                (true, _, false) => 32,
                (false, true, true) => 2,
                (false, true, false) => 16,
                (false, false, true) => 1,
                (false, false, false) => 8,
            };
            let state = (instance.geometry, instance.material, mask);
            if self.instances.get(i) == Some(&state) {
                self.trace
                    .set_transform(i as u64, Affine3A::from_mat4(instance.world));
            } else {
                self.trace.set_instance(
                    i as u64,
                    trace::TraceInstance {
                        mesh: self.meshes[instance.geometry].2,
                        world: Affine3A::from_mat4(instance.world),
                        mask,
                        materials: vec![instance.material as u32],
                        user: i as u32,
                    },
                )?;
                if i == self.instances.len() {
                    self.instances.push(state);
                } else {
                    self.instances[i] = state;
                }
            }
        }
        if meshes_changed {
            self.trace.remove_unused_meshes();
        }
        self.trace.commit();
        Ok(replaced)
    }
}
