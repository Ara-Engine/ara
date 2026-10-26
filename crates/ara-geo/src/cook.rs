use crate::*;
use anyhow::{Result, ensure};
mod compact;

/// Builds meshlets, repeatedly partitions/reduces/reclusters, then packs pages.
/// meshoptimizer preserves the open boundary of each group with LockBorder.
pub fn cook(source: &SourceMesh, settings: Settings) -> Result<Geometry> {
    ensure!(
        cfg!(target_endian = "little"),
        "ARA_GEO_ENDIAN: little-endian cook required"
    );
    ensure!(
        !source.vertices.is_empty()
            && source.vertices.len() <= u32::MAX as usize
            && !source.indices.is_empty()
            && source.indices.len().is_multiple_of(3)
            && source.indices.len() <= u32::MAX as usize
            && source
                .indices
                .iter()
                .all(|&i| (i as usize) < source.vertices.len()),
        "ARA_GEO_INPUT: invalid triangle list"
    );
    ensure!(
        (2..=32).contains(&settings.group_size)
            && (1..=32).contains(&settings.max_levels)
            && settings.relative_error.is_finite()
            && (0.0..=1.0).contains(&settings.relative_error),
        "ARA_GEO_SETTINGS: invalid hierarchy policy"
    );
    ensure!(
        source.vertices.iter().all(|v| v
            .position
            .iter()
            .chain(&v.normal)
            .chain(&v.uv)
            .all(|x| x.is_finite())
            && v.uv.iter().all(|x| x.abs() <= 65504.0)
            && v.normal.iter().any(|v| v.abs() > 1e-10)
            && v.normal.iter().all(|v| v.abs() < 1e10)),
        "ARA_GEO_VERTEX: nonfinite/unrepresentable attributes"
    );
    let adapter = meshopt::VertexDataAdapter::new(
        bytemuck::cast_slice(&source.vertices),
        size_of::<Vertex>(),
        0,
    )?;
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in &source.vertices {
        for i in 0..3 {
            min[i] = min[i].min(v.position[i]);
            max[i] = max[i].max(v.position[i]);
        }
    }
    let scale = std::array::from_fn::<_, 3, _>(|i| (max[i] - min[i]) / 65535.0);
    ensure!(
        scale.iter().all(|v| v.is_finite()),
        "ARA_GEO_RANGE: position range overflow"
    );
    let quantization = scale.iter().map(|v| v * v).sum::<f32>().sqrt() * 0.5;
    let mut geometry = Geometry {
        min,
        scale,
        clusters: Vec::new(),
        groups: Vec::new(),
        children: Vec::new(),
        payload: Vec::new(),
        diagnostics: Diagnostics {
            source_triangles: source.indices.len() as u64 / 3,
            quantization_error: quantization,
            ..Default::default()
        },
    };
    // This offline scratch retains original indices for exact shared boundary locks.
    let mut triangles = Vec::<Vec<u32>>::new();
    let mut remap = compact::Remap::new(source);
    let mut active = append(
        &mut geometry,
        &mut triangles,
        &mut remap,
        &source.indices,
        ROOT,
        0,
    )?;
    geometry.diagnostics.leaf_clusters = u32::try_from(active.len())?;
    for level in 1..settings.max_levels {
        if active.len() <= 1 {
            break;
        }
        let counts: Vec<_> = active
            .iter()
            .map(|&i| triangles[i as usize].len() as u32)
            .collect();
        let indices: Vec<_> = active
            .iter()
            .flat_map(|&i| triangles[i as usize].iter().copied())
            .collect();
        let mut partition = vec![0; active.len()];
        let count = meshopt::partition_clusters_with_positions(
            &mut partition,
            &indices,
            &counts,
            &adapter,
            settings.group_size as usize,
        );
        let mut bins = vec![Vec::new(); count];
        for (&id, &bin) in active.iter().zip(&partition) {
            bins[bin as usize].push(id);
        }
        let mut next = Vec::new();
        let mut progress = false;
        for bin in bins {
            let input: Vec<_> = bin
                .iter()
                .flat_map(|&i| triangles[i as usize].iter().copied())
                .collect();
            let (local, source_ids) = remap.compact(&input);
            let local_adapter = meshopt::VertexDataAdapter::new(
                bytemuck::cast_slice(&local.vertices),
                size_of::<Vertex>(),
                0,
            )?;
            let mut error = 0.0;
            let output = meshopt::simplify(
                &local.indices,
                &local_adapter,
                input.len() / 6 * 3,
                settings.relative_error,
                meshopt::SimplifyOptions::LockBorder,
                Some(&mut error),
            );
            let output: Vec<_> = output.into_iter().map(|i| source_ids[i as usize]).collect();
            if output.is_empty() || output.len() * 10 >= input.len() * 9 {
                geometry.diagnostics.stalled_groups += 1;
                next.extend(bin);
                continue;
            }
            // compute_cluster_bounds only accepts <=256 triangles. A group can
            // contain thousands, so enclose its already conservative spheres.
            let mut low = [f32::INFINITY; 3];
            let mut high = [f32::NEG_INFINITY; 3];
            for &id in &bin {
                let b = geometry.clusters[id as usize].bounds;
                for axis in 0..3 {
                    low[axis] = low[axis].min(b.center[axis] - b.radius);
                    high[axis] = high[axis].max(b.center[axis] + b.radius);
                }
            }
            let center = std::array::from_fn(|i| low[i] * 0.5 + high[i] * 0.5);
            let mut bounds = Sphere {
                center,
                radius: bin
                    .iter()
                    .map(|&id| {
                        let b = geometry.clusters[id as usize].bounds;
                        distance(center, b.center) + b.radius
                    })
                    .fold(0.0, f32::max),
            };
            let mut previous_error = 0.0f32;
            // Enclose descendant decision spheres. This makes projected error
            // monotonic even when a replacement group is repartitioned later.
            for &id in &bin {
                let child = &geometry.clusters[id as usize];
                if child.group != ROOT {
                    let child_group = &geometry.groups[child.group as usize];
                    bounds.radius = bounds.radius.max(
                        distance(bounds.center, child_group.bounds.center)
                            + child_group.bounds.radius,
                    );
                    previous_error = previous_error.max(child_group.error);
                }
            }
            let group = u32::try_from(geometry.groups.len())?;
            geometry.groups.push(Group {
                bounds,
                error: previous_error
                    + error * meshopt::simplify_scale(&local_adapter)
                    + quantization,
                first_child: u32::try_from(geometry.children.len())?,
                child_count: bin.len() as u32,
                level,
            });
            for &id in &bin {
                geometry.clusters[id as usize].parent = group;
            }
            geometry.children.extend(bin);
            next.extend(append(
                &mut geometry,
                &mut triangles,
                &mut remap,
                &output,
                group,
                level,
            )?);
            progress = true;
            geometry.diagnostics.levels = level + 1;
        }
        active = next;
        if !progress {
            break;
        }
    }
    geometry.diagnostics.levels = geometry.diagnostics.levels.max(1);
    geometry.validate()?;
    Ok(geometry)
}

fn append(
    geometry: &mut Geometry,
    triangles: &mut Vec<Vec<u32>>,
    remap: &mut compact::Remap<'_>,
    indices: &[u32],
    group: u32,
    level: u32,
) -> Result<Vec<u32>> {
    let (local_source, source_ids) = remap.compact(indices);
    let adapter = meshopt::VertexDataAdapter::new(
        bytemuck::cast_slice(&local_source.vertices),
        size_of::<Vertex>(),
        0,
    )?;
    let meshlets = meshopt::build_meshlets(
        &local_source.indices,
        &adapter,
        MAX_VERTICES,
        MAX_TRIANGLES,
        0.0,
    );
    let mut result = Vec::new();
    for meshlet in meshlets.iter() {
        let bounds = meshopt::compute_meshlet_bounds(meshlet, &adapter);
        let vertices: Vec<_> = meshlet
            .vertices
            .iter()
            .map(|&i| {
                PackedVertex::pack(
                    local_source.vertices[i as usize],
                    geometry.min,
                    geometry.scale,
                )
            })
            .collect();
        let local: Vec<u32> = meshlet.triangles.iter().map(|&i| u32::from(i)).collect();
        let encoded_vertices = meshopt::encode_vertex_buffer(&vertices)?;
        let encoded_indices = meshopt::encode_index_buffer(&local, vertices.len())?;
        result.push(u32::try_from(geometry.clusters.len())?);
        geometry.clusters.push(Cluster {
            bounds: Sphere {
                center: bounds.center,
                radius: bounds.radius + geometry.diagnostics.quantization_error,
            },
            group,
            parent: ROOT,
            vertex_count: vertices.len() as u32,
            triangle_count: local.len() as u32 / 3,
            offset: u32::try_from(geometry.payload.len())?,
            vertex_bytes: encoded_vertices.len() as u32,
            index_bytes: encoded_indices.len() as u32,
            level,
        });
        geometry.payload.extend(encoded_vertices);
        geometry.payload.extend(encoded_indices);
        triangles.push(
            local
                .iter()
                .map(|&i| source_ids[meshlet.vertices[i as usize] as usize])
                .collect(),
        );
    }
    Ok(result)
}
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        .sqrt()
}
