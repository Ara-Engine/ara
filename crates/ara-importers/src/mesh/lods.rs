//! Classic LODs retain the source vertex buffer and a conservative relative error.
use super::{MeshSettings, Vertex, normalize};
use ara_asset_format::Lod;
use meshopt::{SimplifyOptions, VertexDataAdapter};
use rayon::prelude::*;
mod topology;

fn useful(indices: &[u32], error: f32, previous_count: usize) -> bool {
    !indices.is_empty()
        && indices.len() * 4 <= previous_count * 3
        && error.is_finite()
        && error >= 0.0
}

pub(super) fn append(
    vertices: &[Vertex],
    adapter: &VertexDataAdapter<'_>,
    indices: &mut Vec<u32>,
    settings: &MeshSettings,
    deformed: bool,
    preserve_card_area: bool,
    metrics: Option<&ara_asset::ImportMetrics>,
) -> Vec<Lod> {
    let original = indices.clone();
    let mut lods = vec![Lod {
        first_index: 0,
        index_count: original.len() as u32,
        error: 0.0,
        reserved: 0,
    }];
    let goal = (original.len() / 3).div_ceil(50).max(200) * 3;
    // Keep the previous conservative policy for skin/morph deformation: the
    // source pose cannot bound the error introduced in an arbitrary animation.
    let (count, limit) = if deformed {
        (settings.lod_count.min(4), settings.lod_error.min(0.01))
    } else {
        (settings.lod_count, settings.lod_error)
    };
    if count <= 1 || (!deformed && original.len() <= goal) {
        return lods;
    }
    let preparation = metrics.map(|m| m.start(ara_asset::ImportStage::LodPreparation));
    let topology = (!deformed).then(|| topology::Topology::new(vertices, &original));
    let attributes: Vec<f32> = vertices
        .iter()
        .flat_map(|v| {
            normalize(v.normal)
                .into_iter()
                .chain(v.tangent)
                .chain(v.uv0)
                .chain(v.uv1)
                .chain(v.color)
        })
        .collect();
    const WEIGHTS: [f32; 15] = [
        0.05, 0.05, 0.05, // normals
        0.01, 0.01, 0.01, 0.01, // tangent/handedness
        0.01, 0.01, 0.01, 0.01, // both UV sets
        0.01, 0.01, 0.01, 0.01, // vertex colour
    ];
    drop(preparation);
    let mut previous_count = original.len();
    let mut previous_error = 0.0f32;
    let steps: Vec<_> = (1..count)
        .take_while(|step| (original.len() >> step) / 3 * 3 >= 3)
        .collect();
    // Default to sequential LOD0 solves within the already-parallel primitive
    // pool. The opt-in experiment keeps at most two candidates in flight;
    // acceptance/fallback decisions always retain their original order.
    let width = if super::PARALLEL_GEOMETRY {
        rayon::current_num_threads().min(2)
    } else {
        1
    };
    for batch in steps.chunks(width) {
        if !deformed && previous_count <= goal {
            break;
        }
        let candidates: Vec<_> = batch
            .par_iter()
            .map(|&step| {
                let target = (original.len() >> step) / 3 * 3;
                let mut error = 0.0;
                let _timer = metrics.map(|m| m.start(ara_asset::ImportStage::LodSimplification));
                let geometric = deformed || step < 4;
                let simplified = if geometric {
                    meshopt::simplify(
                        &original,
                        adapter,
                        target,
                        limit.min(0.01),
                        SimplifyOptions::LockBorder,
                        Some(&mut error),
                    )
                } else {
                    let topology = topology.as_ref().expect("static meshes have topology");
                    meshopt::simplify_with_attributes_and_locks(
                        &original,
                        adapter,
                        &attributes,
                        &WEIGHTS,
                        std::mem::size_of_val(&WEIGHTS),
                        &topology.locks,
                        target,
                        limit,
                        SimplifyOptions::LockBorder
                            | SimplifyOptions::Permissive
                            | SimplifyOptions::RegularizeLight,
                        Some(&mut error),
                    )
                };
                (step, geometric, simplified, error)
            })
            .collect();
        for (step, geometric, mut simplified, mut error) in candidates {
            if !deformed && previous_count <= goal {
                break;
            }
            let target = (original.len() >> step) / 3 * 3;
            // Retain the previous geometric-error prefix when it is safe. Replacing
            // it with combined attribute errors can make runtime selection choose
            // finer foliage LODs even when the new chain has more levels.
            if geometric
                && let Some(topology) = &topology
                && (!useful(&simplified, error, previous_count)
                    || (preserve_card_area
                        && !preserves_area(topology, vertices, &simplified, metrics)))
            {
                let _timer = metrics.map(|m| m.start(ara_asset::ImportStage::LodSimplification));
                error = 0.0;
                simplified = meshopt::simplify_with_attributes_and_locks(
                    &original,
                    adapter,
                    &attributes,
                    &WEIGHTS,
                    std::mem::size_of_val(&WEIGHTS),
                    &topology.locks,
                    target,
                    limit,
                    SimplifyOptions::LockBorder
                        | SimplifyOptions::Permissive
                        | SimplifyOptions::RegularizeLight,
                    Some(&mut error),
                );
            }
            // Trying a stronger target can bypass a stalled intermediate target.
            // Never publish duplicate/negligible levels or an empty primitive.
            if !useful(&simplified, error, previous_count) {
                continue;
            }
            if preserve_card_area
                && topology
                    .as_ref()
                    .is_some_and(|t| !preserves_area(t, vertices, &simplified, metrics))
            {
                return lods;
            }
            previous_error = previous_error.max(error);
            lods.push(Lod {
                first_index: indices.len() as u32,
                index_count: simplified.len() as u32,
                error: previous_error,
                reserved: 0,
            });
            previous_count = simplified.len();
            indices.extend(meshopt::optimize_vertex_cache(&simplified, vertices.len()));
        }
    }
    lods
}

fn preserves_area(
    topology: &topology::Topology,
    vertices: &[Vertex],
    indices: &[u32],
    metrics: Option<&ara_asset::ImportMetrics>,
) -> bool {
    let _timer = metrics.map(|m| m.start(ara_asset::ImportStage::LodArea));
    topology.preserves_area(vertices, indices)
}
