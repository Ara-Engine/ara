//! Reusable visible-instance lists; source vertices never move into world space.
use super::*;
use std::collections::BTreeMap;

pub(super) fn is_transparent(material: &crate::instanced::Material) -> bool {
    material.pbr.flags[0] == 2 || is_refractive(material)
}

/// Transmissive materials draw in the forward pass through the refractive PSOs
/// (`NativePipelines::refract`), over a copy of the lit opaque scene.
pub(super) fn is_refractive(material: &crate::instanced::Material) -> bool {
    material.extras.transmission_factor > 0.
}

#[derive(Clone, Copy, Default)]
pub(super) struct Batch {
    pub first: u32,
    pub count: u32,
    pub vertices: u32,
}
pub(super) struct Group {
    pub geometry: usize,
    pub material: usize,
    pub first: u32,
    pub counts: Vec<u32>,
    pub errors: Vec<f32>,
    pub instances: Vec<usize>,
    pub transparent: bool,
    /// A transmissive group (`is_refractive`), drawn by the refractive forward PSOs.
    pub refract: bool,
    pub offset: usize,
    pub slot: usize,
}
pub(super) struct Groups {
    pub groups: Vec<Group>,
    pub instance_group: Vec<usize>,
    pub material_groups: Vec<Vec<usize>>,
    pub geometry_instances: Vec<Vec<usize>>,
    pub material_instances: Vec<Vec<usize>>,
    pub batches: usize,
    identities: Vec<(u64, usize, usize)>,
}
impl Groups {
    pub(super) fn new(scene: &NativeScene, pool: &GeometryGpu) -> Result<Self> {
        let mut grouped = BTreeMap::<(usize, usize), Vec<usize>>::new();
        for (index, instance) in scene.instances.iter().enumerate() {
            grouped
                .entry((instance.geometry, instance.material))
                .or_default()
                .push(index);
        }
        let mut offset = 0;
        let mut slot = 0;
        let groups: Vec<_> = grouped
            .into_iter()
            .map(|((geometry, material), instances)| {
                let range = pool
                    .lods(geometry)
                    .expect("validated instance geometry has LODs");
                let first_instance = offset;
                let first_batch = slot;
                offset += instances.len();
                slot += range.count as usize;
                Group {
                    offset: first_instance,
                    slot: first_batch,
                    geometry,
                    material,
                    first: range.first,
                    counts: (0..range.count)
                        .map(|lod| {
                            pool.index_count(range.first + lod)
                                .expect("uploaded LOD has indices")
                        })
                        .collect(),
                    errors: scene.geometry[geometry]
                        .lods
                        .iter()
                        .map(|lod| lod.error)
                        .collect(),
                    instances,
                    transparent: is_transparent(&scene.materials[material]),
                    refract: is_refractive(&scene.materials[material]),
                }
            })
            .collect();
        let mut instance_group = vec![0; scene.instances.len()];
        let mut material_groups = vec![Vec::new(); scene.materials.len()];
        let mut geometry_instances = vec![Vec::new(); scene.geometry.len()];
        let mut material_instances = vec![Vec::new(); scene.materials.len()];
        for (g, group) in groups.iter().enumerate() {
            material_groups[group.material].push(g);
            for &i in &group.instances {
                instance_group[i] = g;
                geometry_instances[group.geometry].push(i);
                material_instances[group.material].push(i);
            }
        }
        Ok(Self {
            groups,
            instance_group,
            material_groups,
            geometry_instances,
            material_instances,
            batches: slot,
            identities: scene
                .instances
                .iter()
                .map(|i| (i.entity, i.geometry, i.material))
                .collect(),
        })
    }

    pub(super) fn validate(&self, scene: &NativeScene) -> Result<()> {
        for (instance, identity) in scene.instances.iter().zip(&self.identities) {
            ensure!(
                (instance.entity, instance.geometry, instance.material) == *identity,
                "Native instance identity/layout changed"
            );
        }
        for group in &self.groups {
            ensure!(
                scene.geometry[group.geometry].lods.len() == group.counts.len(),
                "Native LOD layout changed"
            );
        }
        Ok(())
    }
    pub(super) fn validate_instance(
        &self,
        index: usize,
        instance: &crate::instanced::Instance,
    ) -> Result<()> {
        ensure!(
            (instance.entity, instance.geometry, instance.material) == self.identities[index],
            "Native instance identity/layout changed"
        );
        Ok(())
    }
}
