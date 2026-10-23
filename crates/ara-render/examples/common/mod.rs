use anyhow::{Context, Result};
use ara_render::{
    capture::{CaptureRequest, ViewMode},
    scene::*,
};
use glam::{Mat4, Vec3};
use std::path::Path;

pub fn camera() -> CameraSpec {
    CameraSpec::LookAt {
        position: Vec3::new(10., 8., 14.),
        target: Vec3::new(0., 1., 0.2),
        vertical_fov: 50f32.to_radians(),
    }
}
pub fn request(size: [u32; 2]) -> CaptureRequest {
    CaptureRequest {
        camera: camera(),
        size,
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 0.0015,
        settle_frames: 8,
        tick: 0,
        seed: 7,
        marks: Some(40),
    }
}
pub fn scene() -> Result<Scene> {
    let mut scene = Scene {
        meshes: Vec::new(),
        materials: vec![Material {
            base_color: [0.45, 0.47, 0.5, 1.],
            textures: [2, 1, 0, 0],
            ..Material::default()
        }],
        textures: vec![
            Texture {
                size: [1, 1],
                rgba: vec![255; 4],
                srgb: false,
            },
            Texture {
                size: [1, 1],
                rgba: vec![128, 128, 255, 255],
                srgb: false,
            },
        ],
        lights: vec![
            Light {
                position_range: [-3., 2.8, 2., 7.],
                color_lumens: [1., 0.12, 0.035, 2400.],
                direction_outer: [0., -1., 0., 0.],
                parameters: [0.; 4],
            },
            Light {
                position_range: [3., 3., 1., 7.],
                color_lumens: [0.05, 0.45, 1., 3200.],
                direction_outer: [0., -1., 0., 0.],
                parameters: [0.; 4],
            },
            Light {
                position_range: [0., 4., -3., 8.],
                color_lumens: [0.75, 0.2, 1., 1800.],
                direction_outer: [0., -1., 0., 0.8],
                parameters: [1., 0.94, 0., 0.],
            },
        ],
        sun_direction: Vec3::new(-0.6, 1., 0.4).normalize(),
        sun_lux: Vec3::splat(3500.),
        environment: Vec::new(),
        environment_size: [0; 2],
    };
    let mut checker = Vec::new();
    for y in 0..64 {
        for x in 0..64 {
            let c = if (x / 8 + y / 8) % 2 == 0 { 170 } else { 205 };
            checker.extend([c, c, c, 255]);
        }
    }
    scene.textures.push(Texture {
        size: [64, 64],
        rgba: checker,
        srgb: true,
    });
    scene.meshes.push(Mesh {
        entity: 100,
        name: "Ground".into(),
        vertices: [
            ([-10., 0., -9.], [0., 0.]),
            ([10., 0., -9.], [4., 0.]),
            ([10., 0., 9.], [4., 4.]),
            ([-10., 0., 9.], [0., 4.]),
        ]
        .map(|(position, uv)| Vertex {
            position,
            normal: [0., 1., 0.],
            uv,
            tangent: [1., 0., 0., -1.],
        })
        .to_vec(),
        indices: vec![0, 1, 2, 0, 2, 3],
        material: 0,
    });
    for row in 0..2 {
        for col in 0..5 {
            let material = scene.materials.len();
            let roughness = 0.08 + col as f32 * 0.22;
            let metal = row as f32;
            scene.materials.push(Material {
                base_color: if row == 0 {
                    [0.42, 0.08, 0.025, 1.]
                } else {
                    [0.83, 0.56, 0.2, 1.]
                },
                emissive_roughness: [0., 0., 0., roughness],
                surface: [metal, 1., 1., 0.5],
                ..Material::default()
            });
            scene.meshes.push(sphere(
                200 + row * 5 + col,
                format!("PBR r={roughness:.2} m={metal:.0}"),
                Vec3::new(col as f32 * 1.9 - 3.8, 0.78, row as f32 * 2.3 + 0.1),
                0.72,
                material,
            ));
        }
    }
    let emissive = scene.materials.len();
    scene.materials.push(Material {
        base_color: [0.06, 0.18, 0.3, 1.],
        emissive_roughness: [70., 420., 900., 0.3],
        ..Material::default()
    });
    scene.meshes.push(sphere(
        300,
        "Emissive cyan".into(),
        Vec3::new(-3.6, 0.9, -3.),
        0.8,
        emissive,
    ));
    let glass = scene.materials.len();
    scene.materials.push(Material {
        base_color: [0.1, 0.6, 0.8, 0.32],
        surface: [0.25, 1., 1., 0.5],
        emissive_roughness: [0., 0., 0., 0.18],
        flags: [2, 0, 0, 0],
        ..Material::default()
    });
    scene.meshes.push(sphere(
        301,
        "Forward transparent".into(),
        Vec3::new(3.6, 1., -2.8),
        0.9,
        glass,
    ));
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    import_gltf(&assets.join("Avocado.glb"), &mut scene)?;
    let hdr = image::open(assets.join("studio_small_09_1k.hdr"))?.to_rgb32f();
    scene.environment_size = [hdr.width(), hdr.height()];
    scene.environment = hdr
        .pixels()
        .map(|p| [p[0] * 500., p[1] * 500., p[2] * 500., 1.])
        .collect();
    Ok(scene)
}
fn import_gltf(path: &Path, scene: &mut Scene) -> Result<()> {
    let (document, buffers, images) = gltf::import(path)?;
    let base = scene.materials.len();
    for material in document.materials() {
        let pbr = material.pbr_metallic_roughness();
        let mut add = |index: usize, srgb: bool| -> Result<u32> {
            let image = &images[index];
            let rgba = match image.format {
                gltf::image::Format::R8G8B8A8 => image.pixels.clone(),
                gltf::image::Format::R8G8B8 => image
                    .pixels
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2], 255])
                    .collect(),
                _ => anyhow::bail!("Example supports RGB8/RGBA8 glTF images"),
            };
            let id = scene.textures.len() as u32;
            scene.textures.push(Texture {
                size: [image.width, image.height],
                rgba,
                srgb,
            });
            Ok(id)
        };
        let albedo = pbr
            .base_color_texture()
            .map(|t| add(t.texture().source().index(), true))
            .transpose()?
            .unwrap_or(0);
        let normal = material
            .normal_texture()
            .map(|t| add(t.texture().source().index(), false))
            .transpose()?
            .unwrap_or(1);
        let mr = pbr
            .metallic_roughness_texture()
            .map(|t| add(t.texture().source().index(), false))
            .transpose()?
            .unwrap_or(0);
        let emissive = material
            .emissive_texture()
            .map(|t| add(t.texture().source().index(), true))
            .transpose()?
            .unwrap_or(0);
        let e = material.emissive_factor();
        scene.materials.push(Material {
            base_color: pbr.base_color_factor(),
            emissive_roughness: [e[0], e[1], e[2], pbr.roughness_factor()],
            surface: [
                pbr.metallic_factor(),
                material.normal_texture().map(|n| n.scale()).unwrap_or(1.),
                1.,
                material.alpha_cutoff().unwrap_or(0.5),
            ],
            textures: [albedo, normal, mr, emissive],
            flags: [
                match material.alpha_mode() {
                    gltf::material::AlphaMode::Opaque => 0,
                    gltf::material::AlphaMode::Mask => 1,
                    gltf::material::AlphaMode::Blend => 2,
                },
                0,
                0,
                0,
            ],
        });
    }
    let root = document
        .default_scene()
        .or_else(|| document.scenes().next())
        .context("No glTF scene")?;
    let world = Mat4::from_translation(Vec3::new(0., 0., -3.0))
        * Mat4::from_rotation_y(0.4)
        * Mat4::from_scale(Vec3::splat(35.));
    for node in root.nodes() {
        node_mesh(node, world, scene, &buffers, base)?;
    }
    Ok(())
}
fn node_mesh(
    node: gltf::Node<'_>,
    parent: Mat4,
    scene: &mut Scene,
    buffers: &[gltf::buffer::Data],
    base: usize,
) -> Result<()> {
    let transform = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
    let normals = transform.inverse().transpose();
    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            anyhow::ensure!(
                primitive.mode() == gltf::mesh::Mode::Triangles,
                "Example supports triangle glTF primitives"
            );
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
            let positions = reader
                .read_positions()
                .context("No positions")?
                .collect::<Vec<_>>();
            let normal = reader
                .read_normals()
                .context("No normals")?
                .collect::<Vec<_>>();
            let uvs = reader
                .read_tex_coords(0)
                .context("No UV0")?
                .into_f32()
                .collect::<Vec<_>>();
            let indices = reader
                .read_indices()
                .context("No indices")?
                .into_u32()
                .collect::<Vec<_>>();
            let mut tangent = vec![Vec3::ZERO; positions.len()];
            let mut bitangent = vec![Vec3::ZERO; positions.len()];
            for tri in indices.as_chunks::<3>().0.iter() {
                let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
                let e1 = Vec3::from_array(positions[b]) - Vec3::from_array(positions[a]);
                let e2 = Vec3::from_array(positions[c]) - Vec3::from_array(positions[a]);
                let d1 = glam::Vec2::from_array(uvs[b]) - glam::Vec2::from_array(uvs[a]);
                let d2 = glam::Vec2::from_array(uvs[c]) - glam::Vec2::from_array(uvs[a]);
                let determinant = d1.x * d2.y - d1.y * d2.x;
                if determinant.abs() > 1e-8 {
                    let t = (e1 * d2.y - e2 * d1.y) / determinant;
                    let bvec = (e2 * d1.x - e1 * d2.x) / determinant;
                    for i in [a, b, c] {
                        tangent[i] += t;
                        bitangent[i] += bvec;
                    }
                }
            }
            let vertices = positions
                .iter()
                .enumerate()
                .map(|(i, &p)| {
                    let n = normals
                        .transform_vector3(Vec3::from_array(normal[i]))
                        .normalize();
                    let t = transform.transform_vector3(tangent[i]);
                    let t = (t - n * n.dot(t))
                        .try_normalize()
                        .unwrap_or_else(|| n.any_orthonormal_vector());
                    let handedness =
                        if n.cross(t).dot(transform.transform_vector3(bitangent[i])) < 0. {
                            -1.
                        } else {
                            1.
                        };
                    Vertex {
                        position: transform.transform_point3(Vec3::from_array(p)).to_array(),
                        normal: n.to_array(),
                        uv: uvs[i],
                        tangent: t.extend(handedness).to_array(),
                    }
                })
                .collect();
            let entity = 400 + scene.meshes.len() as u64;
            scene.meshes.push(Mesh {
                entity,
                name: format!("Avocado {}", node.name().unwrap_or("mesh")),
                vertices,
                indices,
                material: base + primitive.material().index().unwrap_or(0),
            });
        }
    }
    for child in node.children() {
        node_mesh(child, transform, scene, buffers, base)?;
    }
    Ok(())
}
