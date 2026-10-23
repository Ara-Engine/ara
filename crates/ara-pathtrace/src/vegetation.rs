//! Deterministic branch/leaf geometry over the exact landscape height function.
use ara_render::scene::{Mesh, Scene, Texture, Vertex};
use glam::Vec3;

pub(crate) fn populate(scene: &mut Scene) -> anyhow::Result<()> {
    scene.meshes.retain(|m| m.entity == 100);
    let center_height = ara_landscape::sample::height(1056., 992.);
    let height =
        |x: f32, z: f32| ara_landscape::sample::height(x + 1056., z + 992.) - center_height;
    let texture = scene.materials[1].textures[0] as usize;
    let mut leaves = Vec::new();
    for y in 0..128 {
        for x in 0..128 {
            let u = (x as f32 + 0.5) / 64. - 1.;
            let v = (y as f32 + 0.5) / 128.;
            let width = (v * std::f32::consts::PI).sin().powf(0.8) * 0.7;
            let serration = 0.94 + 0.06 * (v * 90.).sin();
            let alpha = if u.abs() < width * serration { 255 } else { 0 };
            let vein = ((u.abs() - (v * 12.).fract() * 0.3).abs() < 0.02) || u.abs() < 0.025;
            let shade = if vein {
                0.65
            } else {
                0.85 + 0.15 * (v * 17. + u * 9.).sin()
            };
            leaves.extend([
                (62. * shade) as u8,
                (112. * shade) as u8,
                (24. * shade) as u8,
                alpha,
            ]);
        }
    }
    scene.textures[texture] = Texture {
        size: [128, 128],
        rgba: leaves,
        srgb: true,
    };
    scene.materials[1].base_color = [0.75, 0.9, 0.65, 1.];
    ara_render_fidelity::material::MaterialDetail {
        transmission: 0.35,
        ..Default::default()
    }
    .apply(&mut scene.materials[1])?;
    let mut wood = Mesh {
        entity: 500,
        name: "Branch geometry".into(),
        vertices: Vec::new(),
        indices: Vec::new(),
        material: 2,
    };
    let mut canopy = Mesh {
        entity: 501,
        name: "Individual serrated alpha leaves, thin-sheet transmission".into(),
        vertices: Vec::new(),
        indices: Vec::new(),
        material: 1,
    };
    for tree in 0..14 {
        let x = -25. + ((tree * 17) % 47) as f32;
        let z = -23. + ((tree * 23) % 43) as f32;
        let base = Vec3::new(x, height(x, z), z);
        let top = base + Vec3::new(0.3, 5.5 + (tree % 4) as f32 * 0.5, 0.2);
        branch(&mut wood, base, top, 0.18, 0.045);
        for j in 0..10 {
            let angle = j as f32 * 2.399963 + tree as f32;
            let start = base.lerp(top, 0.4 + j as f32 * 0.045);
            let end = start + Vec3::new(angle.cos() * 2., 1.1, angle.sin() * 2.);
            branch(&mut wood, start, end, 0.055, 0.009);
            for k in 0..128 {
                let phase = k as f32 * 2.399963 + angle;
                let center = start.lerp(end, 0.3 + k as f32 / 183.)
                    + Vec3::new(
                        phase.cos() * 0.55,
                        (phase * 1.7).sin() * 0.35,
                        phase.sin() * 0.55,
                    );
                let u = Vec3::new(phase.cos(), 0., phase.sin()) * 0.065;
                let v = Vec3::new(-phase.sin() * 0.045, 0.09, phase.cos() * 0.045);
                leaf(&mut canopy, center, u, v);
            }
        }
    }
    // Low ground cover makes alpha silhouettes and close leaf shadows visible.
    for i in 0..480 {
        let random = |n: u32| {
            let n = (n ^ (n >> 16)).wrapping_mul(0x7feb352d);
            let n = (n ^ (n >> 15)).wrapping_mul(0x846ca68b);
            (n ^ (n >> 16)) as f32 / u32::MAX as f32
        };
        let x = -30. + random(i * 2 + 17) * 60.;
        let z = -30. + random(i * 2 + 18) * 60.;
        let phase = i as f32 * 2.399963;
        leaf(
            &mut canopy,
            Vec3::new(x, height(x, z) + 0.3, z),
            Vec3::new(phase.cos(), 0., phase.sin()) * 0.11,
            Vec3::new(0., 0.45, 0.),
        );
    }
    scene.meshes.extend([wood, canopy]);
    // Coarse far terrain comes from the same landscape function. The central
    // hole leaves the exact ARTH tile visible and avoids coincident triangles.
    let mut distant_material = scene.materials[0];
    distant_material.textures[0] = scene.textures.len() as u32;
    let mut distant_texture = Vec::new();
    for z in 0..512 {
        for x in 0..512 {
            let px = 1056. + (x as f32 / 511. - 0.5) * 13000.;
            let pz = 992. + (z as f32 / 511. - 0.5) * 13000.;
            let rock = ara_landscape::sample::surface([px, pz]).map_or(0., |s| s.weights[1]);
            let c = Vec3::new(83., 106., 48.).lerp(Vec3::new(137., 130., 117.), rock);
            distant_texture.extend([c.x as u8, c.y as u8, c.z as u8, 255]);
        }
    }
    scene.textures.push(Texture {
        size: [512, 512],
        rgba: distant_texture,
        srgb: true,
    });
    let material = scene.materials.len();
    scene.materials.push(distant_material);
    let mut hills = Mesh {
        entity: 502,
        name: "Distant landscape heightfield, logarithmic LOD grid".into(),
        vertices: Vec::new(),
        indices: Vec::new(),
        material,
    };
    for z in 0..65 {
        for x in 0..65 {
            let coordinate = |i: u32| {
                if i < 32 {
                    -32. - (2_f32.powf((31 - i) as f32 / 6.) - 1.) * 180.
                } else if i == 32 {
                    0.
                } else {
                    32. + (2_f32.powf((i - 33) as f32 / 6.) - 1.) * 180.
                }
            };
            let px = coordinate(x);
            let pz = coordinate(z);
            let n = Vec3::new(
                height(px - 1., pz) - height(px + 1., pz),
                2.,
                height(px, pz - 1.) - height(px, pz + 1.),
            )
            .normalize();
            hills.vertices.push(Vertex {
                position: [px, height(px, pz) - 0.03, pz],
                normal: n.to_array(),
                uv: [px / 13000. + 0.5, pz / 13000. + 0.5],
                tangent: Vec3::X.extend(1.).to_array(),
            });
        }
    }
    for z in 0..64 {
        for x in 0..64 {
            if (31..33).contains(&x) && (31..33).contains(&z) {
                continue;
            }
            let a = z * 65 + x;
            hills
                .indices
                .extend([a, a + 65, a + 1, a + 1, a + 65, a + 66]);
        }
    }
    scene.meshes.push(hills);
    Ok(())
}
fn leaf(mesh: &mut Mesh, center: Vec3, u: Vec3, v: Vec3) {
    let index = mesh.vertices.len() as u32;
    let n = u.cross(v).normalize();
    for (p, uv) in [
        (center - u - v, [0., 1.]),
        (center + u - v, [1., 1.]),
        (center + u + v, [1., 0.]),
        (center - u + v, [0., 0.]),
    ] {
        mesh.vertices.push(Vertex {
            position: p.to_array(),
            normal: n.to_array(),
            uv,
            tangent: u.normalize().extend(-1.).to_array(),
        });
    }
    mesh.indices
        .extend([index, index + 1, index + 2, index, index + 2, index + 3]);
}
fn branch(mesh: &mut Mesh, a: Vec3, b: Vec3, ra: f32, rb: f32) {
    let axis = (b - a).normalize();
    let u = axis.cross(Vec3::Z).normalize();
    let v = axis.cross(u);
    let base = mesh.vertices.len() as u32;
    for (p, r, t) in [(a, ra, 0.), (b, rb, 1.)] {
        for i in 0..9 {
            let phi = i as f32 * std::f32::consts::TAU / 8.;
            let n = u * phi.cos() + v * phi.sin();
            mesh.vertices.push(Vertex {
                position: (p + n * r).to_array(),
                normal: n.to_array(),
                uv: [i as f32 / 8., t],
                tangent: axis.extend(1.).to_array(),
            });
        }
    }
    for i in 0..8 {
        mesh.indices.extend([
            base + i,
            base + i + 1,
            base + i + 9,
            base + i + 1,
            base + i + 10,
            base + i + 9,
        ]);
    }
}
