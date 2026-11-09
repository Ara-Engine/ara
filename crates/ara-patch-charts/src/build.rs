//! Cell-local connected components; no hash-map iteration or parallel reductions.

use crate::{
    Charts, Error, Fragment, Mesh, Patch, Settings,
    clip::{self, Vertex, cross, dot, sub},
};
use std::collections::BTreeMap;

struct Piece {
    triangle: u32,
    vertices: Vec<Vertex>,
    normal: [f64; 3],
    albedo: [f32; 3],
}

/// Clips scaled source triangles to a 25 cm grid and joins only edge-connected,
/// same-material pieces across non-sharp edges. No rotation/translation enters the
/// key: rigid movement preserves chart identity. A scale change requires recharting.
pub fn build(mesh: Mesh<'_>, settings: Settings) -> Result<Charts, Error> {
    validate(mesh, settings)?;
    let mut cells: BTreeMap<([i32; 3], u32, u8), Vec<Piece>> = BTreeMap::new();
    let mut fragment_count = 0usize;
    for (triangle_index, triangle) in mesh.triangles.iter().enumerate() {
        let positions = triangle.vertices.map(|index| {
            std::array::from_fn(|axis| {
                f64::from(mesh.positions[index as usize][axis]) * f64::from(settings.scale[axis])
            })
        });
        let n = cross(
            sub(positions[1], positions[0]),
            sub(positions[2], positions[0]),
        );
        let length = dot(n, n).sqrt();
        if length == 0.0 {
            continue;
        }
        let normal = n.map(|v| v / length);
        // Keep a curved surface inside one cell from wrapping into a closed shell
        // with a zero average normal. Each bin lies in a strict normal hemisphere.
        let dominant = (0..3)
            .max_by(|&a, &b| normal[a].abs().total_cmp(&normal[b].abs()))
            .expect("three normal axes");
        let hemisphere = dominant as u8 * 2 + u8::from(normal[dominant] < 0.0);
        let vertices = std::array::from_fn(|i| Vertex {
            position: positions[i],
            bary: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]][i],
        });
        for polygon in clip::cells(
            vertices,
            f64::from(settings.edge),
            settings.max_fragments as usize,
        )? {
            fragment_count += polygon.vertices.len() - 2;
            if fragment_count > settings.max_fragments as usize {
                return Err(Error::FragmentLimit);
            }
            cells
                .entry((polygon.cell, triangle.material, hemisphere))
                .or_default()
                .push(Piece {
                    triangle: triangle_index as u32,
                    vertices: polygon.vertices,
                    normal,
                    albedo: triangle.albedo,
                });
        }
    }
    let mut patches = Vec::new();
    let mut fragments = Vec::with_capacity(fragment_count);
    for ((_, material, _), pieces) in cells {
        // Edges, not mere vertex contacts, establish connectivity. Nonmanifold edges
        // compare every incident face; no arbitrary "first two" adjacency loss.
        let mut edges: BTreeMap<[[u64; 3]; 2], Vec<usize>> = BTreeMap::new();
        let mut parent: Vec<usize> = (0..pieces.len()).collect();
        for (i, piece) in pieces.iter().enumerate() {
            for corner in 0..piece.vertices.len() {
                let mut key = [
                    clip::position_key(piece.vertices[corner].position),
                    clip::position_key(
                        piece.vertices[(corner + 1) % piece.vertices.len()].position,
                    ),
                ];
                key.sort_unstable();
                if key[0] == key[1] {
                    continue;
                }
                let neighbors = edges.entry(key).or_default();
                for &j in neighbors.iter() {
                    if dot(piece.normal, pieces[j].normal) >= f64::from(settings.crease_cos) {
                        let a = root(&mut parent, i);
                        let b = root(&mut parent, j);
                        parent[a.max(b)] = a.min(b);
                    }
                }
                neighbors.push(i);
            }
        }
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..pieces.len() {
            groups.entry(root(&mut parent, i)).or_default().push(i);
        }
        for indices in groups.values() {
            if let Some(patch) = aggregate(&pieces, indices, material, &mut fragments)? {
                patches.push(patch);
            }
        }
    }
    Ok(Charts {
        settings,
        patches: patches.into_boxed_slice(),
        fragments: fragments.into_boxed_slice(),
    })
}

pub(crate) fn validate(mesh: Mesh<'_>, settings: Settings) -> Result<(), Error> {
    if !settings.edge.is_finite()
        || settings.edge <= 0.0
        || settings.scale.iter().any(|v| !v.is_finite() || *v == 0.0)
        || !settings.crease_cos.is_finite()
        || !(0.0..=1.0).contains(&settings.crease_cos)
        || settings.max_fragments == 0
        || mesh.triangles.len() > u32::MAX as usize
        || mesh.positions.iter().flatten().any(|v| !v.is_finite())
        || mesh.triangles.iter().any(|t| {
            t.vertices
                .iter()
                .any(|&v| v as usize >= mesh.positions.len())
                || t.albedo
                    .iter()
                    .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        })
    {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn aggregate(
    pieces: &[Piece],
    indices: &[usize],
    material: u32,
    fragments: &mut Vec<Fragment>,
) -> Result<Option<Patch>, Error> {
    let first_fragment = u32::try_from(fragments.len()).map_err(|_| Error::FragmentLimit)?;
    let mut area = 0.0;
    let mut centroid = [0.0; 3];
    let mut normal = [0.0; 3];
    let mut albedo = [0.0; 3];
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for &index in indices {
        let piece = &pieces[index];
        for corner in 1..piece.vertices.len() - 1 {
            let vertices = [
                piece.vertices[0],
                piece.vertices[corner],
                piece.vertices[corner + 1],
            ];
            let [a, b, c] = vertices.map(|v| v.position);
            let n = cross(sub(b, a), sub(c, a));
            let weight = dot(n, n).sqrt() * 0.5;
            if weight == 0.0 {
                continue;
            }
            area += weight;
            for axis in 0..3 {
                centroid[axis] += ((a[axis] + b[axis] + c[axis]) / 3.0) * weight;
                normal[axis] += piece.normal[axis] * weight;
                albedo[axis] += f64::from(piece.albedo[axis]) * weight;
                min[axis] = min[axis].min(a[axis]).min(b[axis]).min(c[axis]);
                max[axis] = max[axis].max(a[axis]).max(b[axis]).max(c[axis]);
            }
            fragments.push(Fragment {
                triangle: piece.triangle,
                barycentric: vertices.map(|v| v.bary.map(|b| b as f32)),
            });
        }
    }
    if (area as f32) == 0.0 {
        // A clipped boundary residue can be nonzero in f64 yet round to exactly
        // zero in the required FP32 surface representation (Polar exercises this).
        // Do not publish a zero-area receiver or reject the remaining mesh.
        fragments.truncate(first_fragment as usize);
        return Ok(None);
    }
    let length = dot(normal, normal).sqrt();
    let patch = Patch {
        area: area as f32,
        centroid: centroid.map(|v| (v / area) as f32),
        normal: normal.map(|v| (v / length) as f32),
        albedo: albedo.map(|v| (v / area) as f32),
        material,
        min: min.map(|v| v as f32),
        max: max.map(|v| v as f32),
        first_fragment,
        fragment_count: u32::try_from(fragments.len()).map_err(|_| Error::FragmentLimit)?
            - first_fragment,
    };
    if !patch.area.is_finite()
        || patch.area <= 0.0
        || patch
            .centroid
            .iter()
            .chain(&patch.normal)
            .any(|v| !v.is_finite())
    {
        return Err(Error::CoordinateRange);
    }
    Ok(Some(patch))
}
