//! Reusable visible-instance lists; source vertices never move into world space.
use super::*;
use std::collections::BTreeMap;

pub(super) fn is_transparent(material: &crate::instanced::Material) -> bool {
    material.pbr.flags[0] == 2 || is_refractive(material)
}

/// Blended (alpha-film) and transmissive materials cast coloured sun shadows
/// through the translucent sun map (`look::translucent`), not opaque ones.
pub(super) fn is_translucent(material: &crate::instanced::Material) -> bool {
    material.pbr.flags[0] == 2 || is_refractive(material)
}

/// Blended double-sided volumes (`Group::planar` false) draw their back faces, then
/// their front faces, so their far side composites under their near side; every other
/// blended draw takes one pass over both sides, as single-sided and flat ones need.
pub(super) fn is_two_pass(material: &crate::instanced::Material, planar: bool) -> bool {
    material.pbr.flags[0] == 2
        && !is_refractive(material)
        && material.pbr.flags[1] & crate::scene::DOUBLE_SIDED != 0
        && !planar
}

/// Whether local bounds are flat (a card, quad or sheet): no face can lie behind
/// another of the same draw, so drawing order within it never matters.
pub(super) fn is_planar([lo, hi]: [Vec3; 2]) -> bool {
    let extent = hi - lo;
    extent.min_element() <= (extent.max_element() * 0.01).max(1e-4)
}

/// Transmissive materials draw in the forward pass through the refractive PSOs
/// (`NativePipelines::refract`), over a copy of the lit opaque scene.
pub(super) fn is_refractive(material: &crate::instanced::Material) -> bool {
    material.extras.transmission_factor > 0.
}

/// Whether an opaque or masked draw of `material` reads vertex colours and so needs
/// the vertex-colour PSO variants (`NativePipelines::gbuffer_paint`): paint layers
/// always do; a tint only where the colours are not all white (`colored`: the
/// geometry declares colours or the instance carries paint).
pub(super) fn reads_vertex_colors(material: &crate::instanced::Material, colored: bool) -> bool {
    let flags = material.pbr.flags[1];
    flags & crate::scene::VERTEX_PAINT_LAYERS != 0
        || (colored && flags & crate::scene::VERTEX_COLOR_TINT != 0)
}

/// One instanced draw: `count` visible rows from `first`, all of one LOD entry,
/// `vertices` indices from `first_index` of the geometry pool each.
#[derive(Clone, Copy, Default)]
pub(super) struct Batch {
    pub first: u32,
    pub count: u32,
    pub vertices: u32,
    pub first_index: u32,
    /// Draw through the pass's vertex-colour PSO variant: `Group::paint` for the
    /// G-buffer, `Group::paint_shadow` for shadow passes; forward PSOs always read them.
    pub paint: bool,
}
pub(super) struct Group {
    pub geometry: usize,
    pub material: usize,
    pub first: u32,
    pub counts: Vec<u32>,
    /// Per LOD, its first index in the geometry pool (`GeometryGpu::first_index`).
    pub first_indices: Vec<u32>,
    pub errors: Vec<f32>,
    pub instances: Vec<usize>,
    pub transparent: bool,
    /// A transmissive group (`is_refractive`), drawn by the refractive forward PSOs.
    pub refract: bool,
    /// A blended group (`is_translucent`), drawn into the translucent sun map.
    pub translucent: bool,
    /// Its instances block light (`Instance::casts_shadows`); every shadow pass and
    /// shadow ray skips a group that doesn't.
    pub casts: bool,
    /// Its geometry is flat (`is_planar`).
    pub planar: bool,
    /// Its forward draws take a back-face and a front-face pass (`is_two_pass`).
    pub two_pass: bool,
    /// Its vertex colours may differ from white: the geometry declares colours or
    /// its instances carry paint (`Instance::paint`).
    pub colored: bool,
    /// Its opaque/masked draws read vertex colours (`reads_vertex_colors`), so they
    /// draw through the vertex-colour PSO variants.
    pub paint: bool,
    /// A masked `paint` group: its shadow alpha test reads vertex colours, so its
    /// shadow draws take the vertex-colour shadow PSO (opaque casters never test).
    pub paint_shadow: bool,
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
    identities: Vec<Identity>,
}

/// What groups an instance: its geometry, material, shadow casting and whether
/// it carries per-instance paint. Changing any of these is a structural edit.
type Identity = (u64, usize, usize, bool, bool);
fn identity(instance: &crate::instanced::Instance) -> Identity {
    (
        instance.entity,
        instance.geometry,
        instance.material,
        instance.casts_shadows,
        instance.paint.is_some(),
    )
}

impl Groups {
    pub(super) fn new(scene: &NativeScene, pool: &GeometryGpu) -> Result<Self> {
        let mut grouped = BTreeMap::<(usize, usize, bool, bool), Vec<usize>>::new();
        for (index, instance) in scene.instances.iter().enumerate() {
            let (_, geometry, material, casts, painted) = identity(instance);
            grouped
                .entry((geometry, material, casts, painted))
                .or_default()
                .push(index);
        }
        let mut offset = 0;
        let mut slot = 0;
        let groups: Vec<_> = grouped
            .into_iter()
            .map(|((geometry, material, casts, painted), instances)| {
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
                    first_indices: (0..range.count)
                        .map(|lod| {
                            pool.first_index(range.first + lod)
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
                    translucent: is_translucent(&scene.materials[material]),
                    casts,
                    planar: is_planar(scene.geometry[geometry].bounds),
                    two_pass: is_two_pass(
                        &scene.materials[material],
                        is_planar(scene.geometry[geometry].bounds),
                    ),
                    colored: painted || scene.geometry[geometry].vertex_colors,
                    paint: reads_vertex_colors(
                        &scene.materials[material],
                        painted || scene.geometry[geometry].vertex_colors,
                    ),
                    paint_shadow: scene.materials[material].pbr.flags[0] == 1
                        && reads_vertex_colors(
                            &scene.materials[material],
                            painted || scene.geometry[geometry].vertex_colors,
                        ),
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
            identities: scene.instances.iter().map(identity).collect(),
        })
    }

    pub(super) fn validate(&self, scene: &NativeScene) -> Result<()> {
        for (instance, known) in scene.instances.iter().zip(&self.identities) {
            ensure!(
                identity(instance) == *known,
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
            identity(instance) == self.identities[index],
            "Native instance identity/layout changed"
        );
        Ok(())
    }
}
