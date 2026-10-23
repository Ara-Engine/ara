//! Daylight landscape extraction and a night room illuminated only by emissive panels.
use crate::scenes::{mat, quad};
use anyhow::Result;
use ara_render::scene::{CameraSpec, Mesh, Scene, Texture, Vertex};
use glam::Vec3;

/// Apply the bounded gather preset and the landscape's explicit local probe field.
/// Shared by capture commands and golden tests so fixture settings cannot drift.
pub fn configure_view(
    gpu: &mut ara_gpu::vulkan::VulkanDevice,
    view: &mut ara_render_fidelity::FidelityView,
    outdoor: bool,
) -> Result<()> {
    // Sparse emitters need more short-range paths than the smooth daylight sky.
    // Spend the small-room budget on two events and half-rate rather than quarter-rate rays.
    view.set_gather_events(if outdoor { 1 } else { 2 })?;
    view.set_transport_stride(if outdoor { 4 } else { 2 })?;
    if outdoor {
        view.set_atmosphere(Some(Default::default()))?;
        view.set_probe_volume(
            gpu,
            ara_render_fidelity::probes::ProbeVolume {
                origin: Vec3::new(-40., -12., -40.),
                spacing: Vec3::splat(4.),
                counts: [22, 16, 22],
            },
        )?;
    }
    Ok(())
}

/// Extract one actual landscape sample tile through the terrain's hole-aware mesh seam.
/// Preparation runs offline; the hybrid renderer owns static BLASes for this snapshot.
pub fn outdoor() -> Result<Scene> {
    let mut scene = crate::scenes::scene("glossy_interior", 0);
    scene.meshes.clear();
    scene.lights.clear();
    scene.materials = vec![
        mat([1.; 3], 0.85, 0.),
        mat([0.2, 0.38, 0.08], 0.8, 0.),
        mat([0.35, 0.2, 0.09], 0.9, 0.),
    ];
    scene.sun_direction = Vec3::new(-0.4, 0.65, 0.55).normalize();
    scene.sun_lux = Vec3::new(18000., 16500., 14000.);
    scene.environment = vec![[900., 1250., 1800., 1.]; 8];
    let header = ara_terrain::Header {
        origin: [1024., 960.],
        spacing: 2.,
        cells: 32,
        min: ara_terrain::TileCoord { x: 0, z: 0 },
        max: ara_terrain::TileCoord { x: 1, z: 1 },
        materials: Vec::new(),
    };
    let tile = ara_landscape::sample::tile(&header, ara_terrain::TileCoord { x: 0, z: 0 })?;
    let mesh = tile
        .heightfield(&header, ara_terrain::TileCoord { x: 0, z: 0 })
        .mesh();
    let center = Vec3::new(1056., ara_landscape::sample::height(1056., 992.), 992.);
    let positions = mesh
        .vertices
        .iter()
        .map(|p| Vec3::from_array(*p) - center)
        .collect::<Vec<_>>();
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for triangle in &mesh.triangles {
        let [a, b, c] = triangle.map(|i| positions[i as usize]);
        let n = (b - a).cross(c - a);
        for i in triangle {
            normals[*i as usize] += n;
        }
    }
    let vertices = positions
        .iter()
        .zip(&normals)
        .map(|(p, n)| {
            let n = n.normalize();
            let tangent = n.cross(Vec3::Z).normalize();
            Vertex {
                position: p.to_array(),
                normal: n.to_array(),
                uv: [(p.x + 32.) / 64., (p.z + 32.) / 64.],
                tangent: tangent.extend(1.).to_array(),
            }
        })
        .collect();
    scene.meshes.push(Mesh {
        entity: 100,
        name: "Landscape ARTH tile, exact hole-aware extraction".into(),
        vertices,
        indices: mesh.triangles.iter().flatten().copied().collect(),
        material: 0,
    });
    let mut texture = Vec::new();
    for z in 0..1024 {
        for x in 0..1024 {
            let sample =
                ara_landscape::sample::surface([1024. + x as f32 / 16., 960. + z as f32 / 16.])
                    .ok_or_else(|| anyhow::anyhow!("Missing sample terrain surface"))?;
            let rock = sample.weights[1];
            let variation = 0.78 + 0.22 * ((x * 1709 + z * 7307) % 137) as f32 / 136.;
            let colour =
                Vec3::new(92., 118., 54.).lerp(Vec3::new(150., 141., 126.), rock) * variation;
            texture.extend([colour.x as u8, colour.y as u8, colour.z as u8, 255]);
        }
    }
    scene.materials[0].textures[0] = scene.textures.len() as u32;
    scene.textures.push(Texture {
        size: [1024, 1024],
        rgba: texture,
        srgb: true,
    });
    let mut leaves = Vec::new();
    for y in 0..32 {
        for x in 0..32 {
            let u = (x as f32 - 15.5) / 15.5;
            let v = (y as f32 - 15.5) / 15.5;
            let alpha = if u * u + v * v * 0.8 < 0.9 && (x + y * 3) % 11 != 0 {
                255
            } else {
                0
            };
            leaves.extend([70 + (x % 5) * 5, 120 + (y % 5) * 5, 35, alpha]);
        }
    }
    scene.materials[1].textures[0] = scene.textures.len() as u32;
    scene.materials[1].flags[0] = 1;
    scene.textures.push(Texture {
        size: [32, 32],
        rgba: leaves,
        srgb: true,
    });
    for i in 0..18 {
        let x = -20. + ((i * 17) % 41) as f32;
        let z = -18. + ((i * 13) % 37) as f32;
        let h = ara_landscape::sample::height(1056. + x, 992. + z) - center.y;
        for axis in 0..2 {
            let u = if axis == 0 {
                Vec3::new(2.8, 0., 0.)
            } else {
                Vec3::new(0., 0., 2.8)
            };
            scene.meshes.push(quad(
                200 + i * 2 + axis,
                "Static alpha canopy snapshot",
                Vec3::new(x, h, z) - u * 0.5,
                u,
                Vec3::new(0., 4., 0.),
                1,
            ));
        }
    }
    crate::vegetation::populate(&mut scene)?;
    Ok(scene)
}

/// Night room with no environment, sun or analytic lights; five small emissive panels.
pub fn night() -> Scene {
    let mut scene = crate::scenes::scene("glossy_interior", 0);
    scene.lights.clear();
    scene.sun_lux = Vec3::ZERO;
    scene.environment = vec![[0., 0., 0., 1.]; 8];
    let mut emitter = mat([0.15; 3], 0.5, 0.);
    emitter.emissive_roughness = [18000., 11000., 6000., 0.5];
    scene.materials.push(emitter);
    for i in 0..5 {
        let x = -2.2 + i as f32 * 1.1;
        scene.meshes.push(quad(
            300 + i,
            "12 cm emissive ceiling panel",
            Vec3::new(x - 0.06, 2.85, -0.2),
            Vec3::new(0.12, 0., 0.),
            Vec3::new(0., 0., 0.12),
            3,
        ));
    }
    scene
}

/// Camera for the original diagnostic room, or the landscape slope and alpha cards.
pub fn camera(outdoor: bool) -> CameraSpec {
    if outdoor {
        CameraSpec::LookAt {
            position: Vec3::new(
                23.,
                ara_landscape::sample::height(1079., 1019.)
                    - ara_landscape::sample::height(1056., 992.)
                    + 5.,
                27.,
            ),
            target: Vec3::new(
                -70.,
                ara_landscape::sample::height(1079., 1019.)
                    - ara_landscape::sample::height(1056., 992.)
                    + 12.,
                -140.,
            ),
            vertical_fov: 1.0,
        }
    } else {
        crate::scenes::camera()
    }
}
