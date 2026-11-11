//! What `scene`'s alpha-mask cut needs beside Renderlab's `cutout`: the mask at the
//! cut grid's scale, a test for masks that hide nothing, and tangent frames for the
//! cut mesh, so a masked material keeps its normal map.
use renderlab::{
    asset::TangentBasis,
    engine::{AlphaMask, TangentStream, Vertex},
};

/// The mask's alpha averaged over blocks of `factor` texels: its coverage at the
/// grid's scale. The cut reads the mask at the grid's corners, so detail finer than a
/// cell (a speaker grille, a fly screen) would be kept or deleted by where the corners
/// happen to fall; averaged, such a surface counts by how much of each cell it covers.
pub(crate) fn coverage(size: [u32; 2], rgba: &[u8], factor: u32) -> AlphaMask {
    let factor = factor.max(1);
    let [width, height] = size.map(|side| side.div_ceil(factor).max(1));
    let mut alpha = Vec::with_capacity((width * height) as usize);
    for block_y in 0..height {
        for block_x in 0..width {
            let (mut sum, mut texels) = (0u32, 0u32);
            for y in (block_y * factor..((block_y + 1) * factor).min(size[1])).map(|y| y as usize) {
                for x in block_x * factor..((block_x + 1) * factor).min(size[0]) {
                    sum += u32::from(rgba[(y * size[0] as usize + x as usize) * 4 + 3]);
                    texels += 1;
                }
            }
            alpha.push(sum as f32 / (texels.max(1) * 255) as f32);
        }
    }
    AlphaMask {
        width,
        height,
        alpha,
    }
}

/// What a base-colour map's alpha does over the surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Alpha {
    /// Every texel is there: the surface is opaque whatever its blend mode says.
    Opaque,
    /// Texels are there or not, with soft edges at most: a cut-out.
    Binary,
    /// Much of what shows is partly there: glass, smoke, a decal's fade.
    Partial,
}

/// A texel at or above this alpha is fully there, and at or below `CLEAR` it is gone.
const SOLID: u8 = 243;
const CLEAR: u8 = 12;
/// A map is `Partial` when more than this share of its visible texels is partly there.
/// A cut-out's soft edges stay under it; a pane of glass is all of it.
const PARTIAL_SHARE: f32 = 0.15;

/// The class of a map's alpha from its `rgba` texels.
pub(crate) fn alpha(rgba: &[u8]) -> Alpha {
    let (mut visible, mut partly, mut least) = (0usize, 0usize, u8::MAX);
    for a in rgba.iter().skip(3).step_by(4) {
        least = least.min(*a);
        visible += usize::from(*a > CLEAR);
        partly += usize::from(*a > CLEAR && *a < SOLID);
    }
    if least >= SOLID {
        Alpha::Opaque
    } else if partly as f32 > PARTIAL_SHARE * visible as f32 {
        Alpha::Partial
    } else {
        Alpha::Binary
    }
}

/// Whether a mask of `rgba` texels over `vertices` passes everywhere: a masked material
/// whose map is opaque throughout. Such a mesh is drawn as it is; cutting it would
/// only multiply its triangles along the grid.
pub(crate) fn hides_nothing(rgba: &[u8], vertices: &[Vertex], opacity: f32, cutoff: f32) -> bool {
    let least = |values: &mut dyn Iterator<Item = f32>| values.fold(1f32, f32::min);
    let texel = least(&mut rgba.iter().skip(3).step_by(4).map(|a| f32::from(*a) / 255.));
    let vertex = least(&mut vertices.iter().map(|v| v.color[3]));
    opacity * texel * vertex >= cutoff
}

/// Tangent frames for cut geometry from its uv mapping: each triangle's change of
/// position along u, summed per vertex and made orthogonal to the vertex normal, with
/// glTF's handedness (the bitangent runs up the image, against v). None when a vertex
/// gets no usable tangent or a triangle's corners disagree on handedness, which
/// Renderlab rejects.
pub(crate) fn tangents(vertices: &[Vertex], indices: &[u32]) -> Option<TangentStream> {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let mut sums = vec![[0f32; 3]; vertices.len()];
    let mut votes = vec![0f32; vertices.len()];
    for triangle in indices.as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|i| vertices[i as usize]);
        let (e1, e2) = (sub(b.position, a.position), sub(c.position, a.position));
        let (d1, d2) = (
            [b.uv[0] - a.uv[0], b.uv[1] - a.uv[1]],
            [c.uv[0] - a.uv[0], c.uv[1] - a.uv[1]],
        );
        let det = d1[0] * d2[1] - d2[0] * d1[1];
        if det.abs() < 1e-20 {
            continue;
        }
        // Left unnormalized by the uv area, so larger triangles weigh more.
        let along_u: [f32; 3] =
            std::array::from_fn(|k| (e1[k] * d2[1] - e2[k] * d1[1]) * det.signum());
        let up_the_image: [f32; 3] =
            std::array::from_fn(|k| (e1[k] * d2[0] - e2[k] * d1[0]) * det.signum());
        for index in triangle {
            let v = &vertices[*index as usize];
            votes[*index as usize] += dot(cross(v.normal, along_u), up_the_image).signum();
            for k in 0..3 {
                sums[*index as usize][k] += along_u[k];
            }
        }
    }
    let mut values = Vec::with_capacity(vertices.len());
    for ((vertex, sum), vote) in vertices.iter().zip(&sums).zip(&votes) {
        let along = dot(*sum, vertex.normal);
        let tangent: [f32; 3] = std::array::from_fn(|k| sum[k] - vertex.normal[k] * along);
        let length = dot(tangent, tangent).sqrt();
        if !length.is_finite() || length < 1e-12 {
            return None;
        }
        let sign = if *vote < 0. { -1. } else { 1. };
        values.push([
            tangent[0] / length,
            tangent[1] / length,
            tangent[2] / length,
            sign,
        ]);
    }
    let agree = indices.as_chunks::<3>().0.iter().all(|t| {
        t.iter()
            .all(|i| values[*i as usize][3] == values[t[0] as usize][3])
    });
    agree.then_some(TangentStream {
        basis: TangentBasis::Authored,
        values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_is_opaque_cut_out_or_partly_there_by_its_texels() {
        let map =
            |alphas: &[u8]| -> Vec<u8> { alphas.iter().flat_map(|a| [9, 9, 9, *a]).collect() };
        // No alpha at all (a JPG under a BLEND material): opaque.
        assert_eq!(alpha(&map(&[255; 64])), Alpha::Opaque);
        // A leaf card: gone or there, with a soft edge on one texel in sixteen.
        let mut leaf = [0u8; 64];
        leaf[32..].fill(255);
        leaf[30] = 128;
        leaf[31] = 128;
        assert_eq!(alpha(&map(&leaf)), Alpha::Binary);
        // A pane of glass: partly there everywhere.
        assert_eq!(alpha(&map(&[80; 64])), Alpha::Partial);
        // A decal that fades over a third of what shows.
        let mut fade = [255u8; 64];
        fade[..24].fill(120);
        assert_eq!(alpha(&map(&fade)), Alpha::Partial);
    }

    /// A unit quad facing +Z with u to the right and v down the image, as glTF maps it.
    fn quad() -> (Vec<Vertex>, Vec<u32>) {
        let corner = |x: f32, y: f32| Vertex {
            position: [x, y, 0.],
            normal: [0., 0., 1.],
            uv: [x, 1. - y],
            color: [1.; 4],
        };
        (
            vec![
                corner(0., 0.),
                corner(1., 0.),
                corner(1., 1.),
                corner(0., 1.),
            ],
            vec![0, 1, 2, 0, 2, 3],
        )
    }

    #[test]
    fn generated_tangents_follow_u_with_gltf_handedness() {
        let (vertices, indices) = quad();
        let stream = tangents(&vertices, &indices).expect("a regular mapping");
        for value in &stream.values {
            assert_eq!(*value, [1., 0., 0., 1.]);
        }
        // Mirrored in u: the tangent turns round and the handedness flips.
        let mirrored: Vec<Vertex> = vertices
            .iter()
            .map(|v| Vertex {
                uv: [1. - v.uv[0], v.uv[1]],
                ..*v
            })
            .collect();
        let stream = tangents(&mirrored, &indices).expect("a mirrored mapping");
        assert_eq!(stream.values[0], [-1., 0., 0., -1.]);
    }

    #[test]
    fn detail_finer_than_a_cell_becomes_its_coverage() {
        // A one-texel checkerboard, 4 by 4: half covered at any scale above a texel.
        let rgba: Vec<u8> = (0..16)
            .flat_map(|i| [0, 0, 0, if (i % 4 + i / 4) % 2 == 0 { 255 } else { 0 }])
            .collect();
        let halves = coverage([4, 4], &rgba, 2);
        assert_eq!((halves.width, halves.height), (2, 2));
        assert!(halves.alpha.iter().all(|a| (a - 0.5).abs() < 1e-6));
        // A factor of one keeps the texels; a ragged edge averages what is there.
        assert_eq!(coverage([4, 4], &rgba, 1).alpha[..2], [1., 0.]);
        assert_eq!(coverage([4, 4], &rgba, 3).width, 2);
    }

    #[test]
    fn a_mask_that_passes_everywhere_hides_nothing() {
        let (vertices, _) = quad();
        let opaque = [9, 9, 9, 255].repeat(4);
        assert!(hides_nothing(&opaque, &vertices, 1., 0.5));
        // A lower opacity factor, or one clear texel, and there is something to cut.
        assert!(!hides_nothing(&opaque, &vertices, 0.4, 0.5));
        let mut holed = opaque.clone();
        holed[7] = 0;
        assert!(!hides_nothing(&holed, &vertices, 1., 0.5));
    }
}
