use super::*;

// Native header 64B, mesh rows 16B, instance rows 192B. First header word's high
// bit selects this ABI; its low 24 bits remain the hardware instance ID count.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct InstanceRow {
    pub(super) meta: [u32; 4],
    pub(super) world: glam::Mat4,
    pub(super) previous: glam::Mat4,
    pub(super) normal: [[f32; 4]; 3],
}

pub(super) fn pack(
    scene: &NativeScene,
    pool: &GeometryGpu,
    materials: u32,
    textures: u32,
    bounds: &mut [Vec3; 2],
) -> Result<Vec<u8>> {
    let records = 64usize
        .checked_add(
            scene
                .instances
                .len()
                .checked_mul(16)
                .ok_or_else(|| anyhow::anyhow!("native instance-table overflow"))?,
        )
        .ok_or_else(|| anyhow::anyhow!("native scene-table overflow"))?;
    let mut out = vec![0; records];
    for (id, instance) in scene.instances.iter().enumerate() {
        let geometry = &scene.geometry[instance.geometry];
        let lod = pool
            .lods(instance.geometry)
            .ok_or_else(|| anyhow::anyhow!("native LOD range missing"))?
            .first;
        let at = u32::try_from(out.len())?;
        let determinant = instance.world.determinant();
        ensure!(
            determinant.is_finite() && determinant.abs() > 1e-12,
            "singular native instance transform"
        );
        let normal = glam::Mat3::from_mat4(instance.world).inverse().transpose();
        let normal = std::array::from_fn(|row| {
            [
                normal.x_axis[row],
                normal.y_axis[row],
                normal.z_axis[row],
                0.0,
            ]
        });
        let row = InstanceRow {
            meta: [
                lod,
                instance.material as u32,
                determinant.signum().to_bits(),
                lod,
            ],
            world: instance.world,
            previous: instance.previous_world,
            normal,
        };
        out.extend_from_slice(bytemuck::bytes_of(&row));
        out[64 + id * 16..80 + id * 16].copy_from_slice(bytemuck::cast_slice(&[
            at,
            lod,
            geometry.lods[0].indices.len() as u32,
            instance.material as u32,
        ]));
        let aabb = instance.bounds(geometry);
        bounds[0] = bounds[0].min(aabb[0]);
        bounds[1] = bounds[1].max(aabb[1]);
    }
    if scene.instances.is_empty() {
        *bounds = [Vec3::splat(-1.0), Vec3::ONE];
    }
    let lights = u32::try_from(out.len())?;
    out.extend_from_slice(bytemuck::cast_slice(&scene.lights));
    let (emitters, count, power) = emitters(scene, &mut out)?;
    let header = [
        0x80000000 | scene.instances.len() as u32,
        materials,
        lights,
        scene.lights.len() as u32,
        u32::from(
            scene
                .instances
                .iter()
                .all(|i| scene.materials[i.material].pbr.flags[0] == 0),
        ),
        emitters,
        count,
        power.to_bits(),
        pool.table_index().unwrap_or(0),
        textures,
        scene.textures,
        records as u32,
        192,
        64,
        320,
        0,
    ];
    out[..64].copy_from_slice(bytemuck::cast_slice(&header));
    u32::try_from(out.len())?;
    Ok(out)
}

pub(super) fn instances(
    scene: &NativeScene,
    pool: &GeometryGpu,
) -> Result<Vec<AccelerationInstance>> {
    scene
        .instances
        .iter()
        .enumerate()
        .map(|(id, instance)| {
            let row = pool
                .lods(instance.geometry)
                .ok_or_else(|| anyhow::anyhow!("native LOD range missing"))?
                .first;
            let geometry = pool
                .blas(row)
                .ok_or_else(|| anyhow::anyhow!("native BLAS missing"))?;
            let w = instance.world.to_cols_array_2d();
            Ok(AccelerationInstance {
                geometry,
                custom_index: id as u32,
                mask: if scene.materials[instance.material].pbr.flags[0] == 0 {
                    1
                } else {
                    2
                },
                transform: [
                    w[0][0], w[1][0], w[2][0], w[3][0], w[0][1], w[1][1], w[2][1], w[3][1],
                    w[0][2], w[1][2], w[2][2], w[3][2],
                ],
            })
        })
        .collect()
}

fn emitters(scene: &NativeScene, out: &mut Vec<u8>) -> Result<(u32, u32, f32)> {
    let offset = u32::try_from(out.len())?;
    let mut count = 0u32;
    let mut total = 0.0;
    for (id, instance) in scene.instances.iter().enumerate() {
        let m = &scene.materials[instance.material].pbr;
        let power = Vec3::from_slice(&m.emissive_roughness).dot(Vec3::new(0.2126, 0.7152, 0.0722));
        if power <= 0.0 {
            continue;
        }
        let geometry = &scene.geometry[instance.geometry];
        for (primitive, ix) in geometry.lods[0]
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
        {
            let p: [Vec3; 3] = ix.map(|index| {
                instance
                    .world
                    .transform_point3(Vec3::from_array(geometry.vertices[index as usize].position))
            });
            let area = (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
            if area <= 1e-10 {
                continue;
            }
            let weight = area * power;
            total += weight;
            for value in [p[0].extend(total), p[1].extend(area), p[2].extend(weight)] {
                out.extend_from_slice(bytemuck::bytes_of(&value));
            }
            out.extend_from_slice(bytemuck::cast_slice(&[
                id as u32,
                primitive as u32,
                instance.material as u32,
                0,
            ]));
            let cached = if m.textures[3] == ara_render::instanced::NO_TEXTURE {
                [
                    m.emissive_roughness[0],
                    m.emissive_roughness[1],
                    m.emissive_roughness[2],
                    1.0,
                ]
            } else {
                [0.0; 4]
            };
            out.extend_from_slice(bytemuck::cast_slice(&cached));
            count += 1;
        }
    }
    Ok((if count == 0 { 0 } else { offset }, count, total))
}
