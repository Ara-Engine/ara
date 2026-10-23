use anyhow::{Context, Result, ensure};
use ara_geo::{SourceMesh, Vertex};
use ara_render_geo::Instance;
use std::path::Path;

pub fn photogrammetry(root: &Path, side: u32) -> Result<(SourceMesh, Vec<Instance>)> {
    let path = root.join("captures/assets/coast_rocks_01/coast_rocks_01_2k.gltf");
    let bytes = std::fs::read(&path)
        .context("Run crates/ara-render-geo/fetch-scene.ps1 to fetch the CC0 scan")?;
    let gltf = gltf::Gltf::from_slice(&bytes)?;
    let binary = std::fs::read(path.with_file_name("coast_rocks_01.bin"))?;
    let primitive = gltf
        .meshes()
        .next()
        .context("scan mesh")?
        .primitives()
        .next()
        .context("scan primitive")?;
    let reader = primitive.reader(|_| Some(binary.as_slice()));
    let positions: Vec<_> = reader.read_positions().context("scan positions")?.collect();
    let normals: Vec<_> = reader.read_normals().context("scan normals")?.collect();
    let uv: Vec<_> = reader
        .read_tex_coords(0)
        .context("scan UV")?
        .into_f32()
        .collect();
    ensure!(
        positions.len() == normals.len() && positions.len() == uv.len(),
        "invalid scan attribute counts"
    );
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in &positions {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    let scale = 7.5 / (max[0] - min[0]).max(max[2] - min[2]);
    let center = [(min[0] + max[0]) * 0.5, min[1], (min[2] + max[2]) * 0.5];
    let mut vertices: Vec<_> = positions
        .iter()
        .zip(normals)
        .zip(uv)
        .map(|((p, normal), uv)| Vertex {
            position: std::array::from_fn(|i| (p[i] - center[i]) * scale),
            normal,
            uv,
        })
        .collect();
    let mut indices: Vec<_> = reader
        .read_indices()
        .context("scan indices")?
        .into_u32()
        .collect();
    // A broad neutral surface below each scan patch closes the open scan edges.
    // Constant UV samples sand from the actual albedo atlas, without inventing
    // a second photographic material or counting synthetic detail as scan faces.
    let first = vertices.len() as u32;
    for p in [
        [-4., -0.025, -4.],
        [-4., -0.025, 4.],
        [4., -0.025, 4.],
        [4., -0.025, -4.],
    ] {
        vertices.push(Vertex {
            position: p,
            normal: [0., 1., 0.],
            uv: [0.05, 0.05],
        });
    }
    indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    let mut instances = Vec::new();
    for z in 0..side {
        for x in 0..side {
            let hash = x
                .wrapping_mul(747796405)
                .wrapping_add(z.wrapping_mul(2891336453));
            let jitter = ((hash >> 8) & 255) as f32 / 255.;
            instances.push(Instance {
                position_scale: [
                    (x as f32 - (side - 1) as f32 * 0.5 + jitter * 0.2) * 5.3,
                    -jitter * 0.2,
                    (z as f32 - (side - 1) as f32 * 0.5 + ((hash >> 16) & 255) as f32 / 1275.)
                        * 5.3,
                    1.0 + jitter * 0.35,
                ],
                color_roughness: [
                    0.92 + jitter * 0.08,
                    0.92 + jitter * 0.08,
                    0.92 + jitter * 0.08,
                    0.82,
                ],
                identity: [
                    z * side + x + 1,
                    0,
                    0,
                    (jitter * std::f32::consts::TAU).to_bits(),
                ],
            });
        }
    }
    Ok((SourceMesh { vertices, indices }, instances))
}
