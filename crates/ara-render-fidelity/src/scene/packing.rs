//! Validated scene tables and address-independent provenance.
use super::*;
pub(super) fn validate(s: &Scene) -> Result<()> {
    ensure!(
        !s.meshes.is_empty() && s.meshes.len() <= 4096 && !s.materials.is_empty(),
        "Invalid geometry count"
    );
    let mut ids = std::collections::BTreeSet::new();
    for m in &s.meshes {
        ensure!(
            ids.insert(m.entity)
                && m.material < s.materials.len()
                && !m.vertices.is_empty()
                && !m.indices.is_empty()
                && m.indices.len() % 3 == 0
                && m.indices.iter().all(|i| (*i as usize) < m.vertices.len()),
            "Invalid mesh or duplicate entity"
        );
        ensure!(
            m.vertices.iter().all(|v| v
                .position
                .iter()
                .chain(&v.normal)
                .chain(&v.uv)
                .chain(&v.tangent)
                .all(|f| f.is_finite())
                && Vec3::from_array(v.normal).length_squared() > 1e-8
                && Vec3::from_slice(&v.tangent).length_squared() > 1e-8),
            "Nonfinite vertex"
        );
    }
    ensure!(
        !s.textures.is_empty() && s.textures.len() <= 96,
        "Invalid texture count"
    );
    for t in &s.textures {
        ensure!(
            !t.size.contains(&0)
                && t.size[0] <= 8192
                && t.size[1] <= 8192
                && t.rgba.len() as u64 == u64::from(t.size[0]) * u64::from(t.size[1]) * 4,
            "Invalid texture"
        );
    }
    for m in &s.materials {
        ensure!(
            m.textures.iter().all(|t| (*t as usize) < s.textures.len())
                && m.base_color
                    .iter()
                    .chain(&m.emissive_roughness)
                    .chain(&m.surface)
                    .all(|v| v.is_finite())
                && m.flags[0] <= 2,
            "Invalid material"
        );
    }
    ensure!(
        s.lights.len() <= 64
            && s.sun_direction.is_finite()
            && s.sun_direction.length_squared() > 1e-8
            && s.sun_lux.is_finite()
            && s.sun_lux.cmpge(Vec3::ZERO).all(),
        "Invalid lighting"
    );
    for light in &s.lights {
        ensure!(
            light
                .position_range
                .iter()
                .chain(&light.color_lumens)
                .chain(&light.direction_outer)
                .chain(&light.parameters)
                .all(|v| v.is_finite())
                && light.position_range[3] > 0.
                && light.color_lumens.iter().all(|v| *v >= 0.),
            "Invalid local light values"
        );
        ensure!(
            (light.parameters[0] == 0. || light.parameters[0] == 1.)
                && (light.parameters[0] == 0.
                    || (Vec3::from_slice(&light.direction_outer).length_squared() > 1e-8
                        && (-1.0..=1.0).contains(&light.direction_outer[3])
                        && (light.direction_outer[3]..=1.0).contains(&light.parameters[1]))),
            "Invalid spot cone/direction"
        );
    }
    ensure!(
        !s.environment_size.contains(&0)
            && s.environment.len() as u64
                == u64::from(s.environment_size[0]) * u64::from(s.environment_size[1])
            && s.environment
                .iter()
                .flatten()
                .all(|f| f.is_finite() && *f >= 0.),
        "Invalid environment"
    );
    Ok(())
}
// All offsets are byte offsets. Mesh records reference concatenated shading inputs;
// BLAS inputs remain separate so unchanged meshes can retain their native identities.
pub(super) fn pack(
    s: &Scene,
    materials: &[Material],
    ids: &std::collections::BTreeMap<u64, u32>,
    bounds: &mut [Vec3; 2],
) -> Vec<u8> {
    let mut out = vec![0u8; 32 + ids.len() * 16];
    let material_offset = out.len() as u32;
    out.extend_from_slice(bytemuck::cast_slice(materials));
    let light_offset = out.len() as u32;
    out.extend_from_slice(bytemuck::cast_slice(&s.lights));
    for mesh in &s.meshes {
        let v = out.len() as u32;
        out.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
        let i = out.len() as u32;
        out.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
        let at = 32 + ids[&mesh.entity] as usize * 16;
        out[at..at + 16].copy_from_slice(bytemuck::cast_slice(&[
            v,
            i,
            mesh.indices.len() as u32,
            mesh.material as u32,
        ]));
        for p in &mesh.vertices {
            let p = Vec3::from_array(p.position);
            bounds[0] = bounds[0].min(p);
            bounds[1] = bounds[1].max(p);
        }
    }
    out[..32].copy_from_slice(bytemuck::cast_slice(&[
        ids.len() as u32,
        material_offset,
        light_offset,
        s.lights.len() as u32,
        u32::from(
            s.meshes
                .iter()
                .all(|mesh| materials[mesh.material].flags[0] == 0),
        ),
        0,
        0,
        0,
    ]));
    out
}
pub(super) fn texture_hash(scene: &Scene) -> [u8; 32] {
    let mut h = Sha256::new();
    for t in &scene.textures {
        h.update(bytemuck::cast_slice(&t.size));
        h.update([u8::from(t.srgb)]);
        h.update(&t.rgba);
    }
    h.update(bytemuck::cast_slice(&scene.environment_size));
    h.update(bytemuck::cast_slice(&scene.environment));
    h.finalize().into()
}
pub(super) fn content_hash(
    scene: &Scene,
    ids: &std::collections::BTreeMap<u64, u32>,
    bounds: &mut [Vec3; 2],
) -> String {
    let mut h = Sha256::new();
    h.update(pack(scene, &scene.materials, ids, bounds));
    h.update(texture_hash(scene));
    h.update(bytemuck::cast_slice(&scene.sun_direction.to_array()));
    h.update(bytemuck::cast_slice(&scene.sun_lux.to_array()));
    // External IDs participate even when all geometry/material inputs are equal.
    for (entity, id) in ids {
        h.update(entity.to_le_bytes());
        h.update(id.to_le_bytes());
    }
    format!("{:x}", h.finalize())
}
