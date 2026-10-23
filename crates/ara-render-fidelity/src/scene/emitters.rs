//! Area/power-weighted emissive triangle distribution for explicit light sampling.
use super::*;

/// Append area/power CDF records using stable scene IDs and world-space triangles.
pub(super) fn append(
    scene: &Scene,
    materials: &[Material],
    ids: &std::collections::BTreeMap<u64, u32>,
    out: &mut Vec<u8>,
) -> (u32, u32, f32) {
    let offset = out.len() as u32;
    let mut count = 0;
    let mut total = 0.;
    for mesh in &scene.meshes {
        let m = materials[mesh.material];
        let power = Vec3::from_slice(&m.emissive_roughness).dot(Vec3::new(0.2126, 0.7152, 0.0722));
        if power <= 0. {
            continue;
        }
        for (primitive, triangle) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
            let p: [Vec3; 3] = std::array::from_fn(|i| {
                Vec3::from_array(mesh.vertices[triangle[i] as usize].position)
            });
            let area = (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
            if area <= 1e-10 {
                continue;
            }
            let weight = area * power;
            total += weight;
            for v in [
                p[0].extend(total).to_array(),
                p[1].extend(area).to_array(),
                p[2].extend(weight).to_array(),
            ] {
                out.extend_from_slice(bytemuck::cast_slice(&v));
            }
            out.extend_from_slice(bytemuck::cast_slice(&[
                ids[&mesh.entity],
                primitive as u32,
                0,
                0,
            ]));
            count += 1;
        }
    }
    (if count == 0 { 0 } else { offset }, count, total)
}
