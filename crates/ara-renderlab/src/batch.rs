//! Static batches: what keeps a frame with more mesh-material groups than Renderlab
//! draws (`TRACED_DRAWS`) whole. Opaque static groups that share a material merge into
//! one world-space mesh per material, which costs a single group; groups with the fewest
//! objects merge first, so widely instanced meshes keep their instancing. Draws are left
//! out only when merging every candidate still does not fit.
use ara_runtime::model::ModelVertex;
use glam34::{Mat3, Mat4, Vec3};
use std::collections::HashMap;

/// One mesh-material group of a frame, in order of first appearance.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Group {
    pub material: u64,
    /// Static and opaque: it may join its material's batch.
    pub mergeable: bool,
    /// Objects drawing the group.
    pub objects: usize,
}

/// What a frame does with each group (`plan`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fate {
    /// Drawn as its own group.
    Own,
    /// Merged into its material's batch.
    Merged,
    /// Left out: even merging does not fit the budget.
    Dropped,
}

/// Decides which groups merge and which, as a last resort, are left out, so that at
/// most `budget` groups (own groups plus one per batch) remain. Materials with the
/// most mergeable groups merge first, and within a material the groups with the fewest
/// objects; a material merges only when that saves groups (two or more join it).
/// Groups left out are the last to appear.
pub(crate) fn plan(groups: &[Group], budget: usize) -> Vec<Fate> {
    let mut fates = vec![Fate::Own; groups.len()];
    let mut count = groups.len();
    if count <= budget {
        return fates;
    }
    let mut by_material: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, group) in groups.iter().enumerate().filter(|(_, g)| g.mergeable) {
        by_material.entry(group.material).or_default().push(i);
    }
    let mut materials: Vec<Vec<usize>> = by_material
        .into_values()
        .filter(|members| members.len() > 1)
        .collect();
    // Most groups first; ties by first appearance, so the plan is stable across frames.
    materials.sort_by_key(|members| (std::cmp::Reverse(members.len()), members[0]));
    for mut members in materials {
        if count <= budget {
            break;
        }
        members.sort_by_key(|i| (groups[*i].objects, *i));
        // Merging j of a material's groups replaces them with one batch: saves j - 1.
        let take = (count - budget + 1).min(members.len());
        for i in &members[..take] {
            fates[*i] = Fate::Merged;
        }
        count -= take - 1;
    }
    for fate in fates.iter_mut().rev() {
        if count <= budget {
            break;
        }
        if *fate == Fate::Own {
            *fate = Fate::Dropped;
            count -= 1;
        }
    }
    // Only batches left over the budget (more merged materials than it holds): the
    // last-appearing batches go out whole.
    if count > budget {
        let mut first: Vec<(usize, u64)> = Vec::new();
        for (i, group) in groups.iter().enumerate() {
            if fates[i] == Fate::Merged && !first.iter().any(|(_, m)| *m == group.material) {
                first.push((i, group.material));
            }
        }
        for (_, material) in first.iter().rev().take(count - budget) {
            for (i, group) in groups.iter().enumerate() {
                if group.material == *material && fates[i] == Fate::Merged {
                    fates[i] = Fate::Dropped;
                }
            }
        }
    }
    fates
}

/// Appends `vertices` and `indices` to a batch in world space: positions by `world`,
/// normals by its inverse transpose, tangents by its linear part. A mirroring transform
/// flips the winding and the tangents' handedness, as rendering it in place would.
pub(crate) fn append_world(
    world: &Mat4,
    vertices: &[ModelVertex],
    indices: &[u32],
    out_vertices: &mut Vec<ModelVertex>,
    out_indices: &mut Vec<u32>,
) {
    let base = out_vertices.len() as u32;
    let linear = Mat3::from_mat4(*world);
    let normals = linear.inverse().transpose();
    let mirrored = linear.determinant() < 0.;
    out_vertices.extend(vertices.iter().map(|v| {
        let mut out = *v;
        out.position = world.transform_point3(Vec3::from(v.position)).to_array();
        out.normal = (normals * Vec3::from(v.normal)).to_array();
        let tangent = linear * Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]);
        let sign = if mirrored {
            -v.tangent[3]
        } else {
            v.tangent[3]
        };
        out.tangent = [tangent.x, tangent.y, tangent.z, sign];
        out
    }));
    for triangle in indices.as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|i| base + i);
        out_indices.extend(if mirrored { [a, c, b] } else { [a, b, c] });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(material: u64, objects: usize) -> Group {
        Group {
            material,
            mergeable: true,
            objects,
        }
    }

    #[test]
    fn under_the_budget_nothing_changes() {
        let groups = [group(1, 1), group(1, 1), group(2, 1)];
        assert!(plan(&groups, 3).iter().all(|f| *f == Fate::Own));
    }

    #[test]
    fn shared_materials_merge_fewest_objects_first() {
        // Five groups of material 1 (one instanced 40 times), two of material 2.
        let groups = [
            group(1, 40),
            group(1, 1),
            group(1, 1),
            group(1, 2),
            group(1, 1),
            group(2, 1),
            group(2, 1),
        ];
        let fates = plan(&groups, 5);
        // Seven groups into five: three of material 1 become one batch (saves two),
        // and the instanced group keeps its own.
        assert_eq!(fates[0], Fate::Own);
        assert_eq!(fates.iter().filter(|f| **f == Fate::Merged).count(), 3);
        assert_eq!(fates[3], Fate::Own, "the two-object group merges last");
        assert!(!fates.contains(&Fate::Dropped));
    }

    #[test]
    fn unique_materials_and_skinned_groups_never_merge() {
        let mut groups: Vec<Group> = (0..6).map(|m| group(m, 1)).collect();
        groups[0].mergeable = false;
        let fates = plan(&groups, 4);
        // Nothing saves a group, so the last two are left out.
        assert_eq!(&fates[..4], &[Fate::Own; 4]);
        assert_eq!(&fates[4..], &[Fate::Dropped; 2]);
    }

    #[test]
    fn every_kept_plan_fits_the_budget() {
        let groups: Vec<Group> = (0..300u64)
            .map(|i| group(i % 37, 1 + (i % 3) as usize))
            .collect();
        for budget in [1, 10, 37, 64, 128, 256] {
            let fates = plan(&groups, budget);
            let own = fates.iter().filter(|f| **f == Fate::Own).count();
            let mut batches: Vec<u64> = groups
                .iter()
                .zip(&fates)
                .filter(|(_, f)| **f == Fate::Merged)
                .map(|(g, _)| g.material)
                .collect();
            batches.sort_unstable();
            batches.dedup();
            assert!(own + batches.len() <= budget, "budget {budget}");
            if budget >= 37 {
                assert!(!fates.contains(&Fate::Dropped), "budget {budget} drops");
            }
        }
    }

    #[test]
    fn world_space_mirrors_flip_winding_and_handedness() {
        let v = |x: f32, y: f32| ModelVertex {
            position: [x, y, 0.],
            normal: [0., 0., 1.],
            tangent: [1., 0., 0., 1.],
            ..Default::default()
        };
        let (mut vertices, mut indices) = (vec![v(9., 9.)], vec![]);
        let mirror = Mat4::from_translation(Vec3::X) * Mat4::from_scale(Vec3::new(-2., 1., 1.));
        append_world(
            &mirror,
            &[v(0., 0.), v(1., 0.), v(0., 1.)],
            &[0, 1, 2],
            &mut vertices,
            &mut indices,
        );
        assert_eq!(indices, [1, 3, 2]);
        assert_eq!(vertices[2].position, [-1., 0., 0.]);
        assert_eq!(vertices[2].normal, [0., 0., 1.]);
        assert_eq!(vertices[2].tangent, [-2., 0., 0., -1.]);
    }
}
