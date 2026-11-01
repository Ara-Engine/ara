//! Source glTF A/B loader. This deliberately reports source-fixture provenance;
//! it does not pretend to reproduce Polar's separately authored cooked project.
use super::fixtures::Fixture;
use anyhow::{Context, Result, ensure};
use ara_render::{instanced as n, scene};
use glam::{Mat4, Vec3};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

pub fn load(path: &Path) -> Result<Fixture> {
    let (doc, buffers, images) = gltf::import(path)?;
    let textures = images
        .into_iter()
        .map(|image| {
            use gltf::image::Format;
            let channels = match image.format {
                Format::R8 => 1,
                Format::R8G8 => 2,
                Format::R8G8B8 => 3,
                Format::R8G8B8A8 => 4,
                _ => anyhow::bail!("unsupported glTF image precision; do not silently downsample"),
            };
            let rgba = image
                .pixels
                .chunks_exact(channels)
                .flat_map(|p| {
                    [
                        p[0],
                        *p.get(1).unwrap_or(&p[0]),
                        *p.get(2).unwrap_or(&p[0]),
                        if channels == 4 { p[3] } else { 255 },
                    ]
                })
                .collect();
            Ok(scene::Texture {
                size: [image.width, image.height],
                rgba,
                srgb: true,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut materials = Vec::new();
    for m in doc.materials() {
        let p = m.pbr_metallic_roughness();
        let mut material = n::Material::default();
        material.pbr.base_color = p.base_color_factor();
        let e = m.emissive_factor();
        material.pbr.emissive_roughness = [e[0], e[1], e[2], p.roughness_factor()];
        material.pbr.surface[0] = p.metallic_factor();
        material.pbr.surface[3] = m.alpha_cutoff().unwrap_or(0.5);
        material.pbr.flags[0] = match m.alpha_mode() {
            gltf::material::AlphaMode::Opaque => 0,
            gltf::material::AlphaMode::Mask => 1,
            gltf::material::AlphaMode::Blend => 2,
        };
        if m.double_sided() {
            material.pbr.flags[1] |= scene::DOUBLE_SIDED;
        }
        for (slot, texture) in [
            (0, p.base_color_texture().map(|t| t.texture())),
            (1, m.normal_texture().map(|t| t.texture())),
            (2, p.metallic_roughness_texture().map(|t| t.texture())),
            (3, m.emissive_texture().map(|t| t.texture())),
        ] {
            if let Some(t) = texture {
                material.pbr.textures[slot] = t.source().index() as u32;
            }
        }
        materials.push(material);
    }
    let default_material = materials.len();
    materials.push(n::Material::default());
    let mut result = n::Scene {
        geometry: Vec::new(),
        instances: Vec::new(),
        materials,
        textures: textures.len() as u32,
        decals: Vec::new(),
        lights: Vec::new(),
        sun_direction: Vec3::new(0.4, 0.8, 0.3).normalize(),
        sun_lux: Vec3::splat(200.),
        environment: vec![[0.1, 0.12, 0.16, 1.]],
        environment_size: [1, 1],
    };
    let mut keys = BTreeMap::new();
    let roots = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .context("glTF has no scene")?;
    for node in roots.nodes() {
        visit(
            node,
            Mat4::IDENTITY,
            &buffers,
            &mut result,
            &mut keys,
            default_material,
        )?;
    }
    result.validate()?;
    let bounds = result.instances.iter().fold(
        [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)],
        |b, i| {
            let a = i.bounds(&result.geometry[i.geometry]);
            [b[0].min(a[0]), b[1].max(a[1])]
        },
    );
    Ok(Fixture {
        scene: result,
        textures,
        bodies: BTreeMap::new(),
        camera: scene::CameraSpec::FrameBounds {
            min: bounds[0],
            max: bounds[1],
            direction: Vec3::new(1., 0.5, 1.),
            padding: 0.1,
            vertical_fov: 55f32.to_radians(),
        },
        exposure: 1. / 38.4,
    })
}
fn visit(
    node: gltf::Node<'_>,
    parent: Mat4,
    buffers: &[gltf::buffer::Data],
    scene: &mut n::Scene,
    keys: &mut BTreeMap<(usize, usize), usize>,
    default_material: usize,
) -> Result<()> {
    let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            ensure!(
                primitive.mode() == gltf::mesh::Mode::Triangles,
                "nontriangle source primitive"
            );
            let key = (mesh.index(), primitive.index());
            let geometry = if let Some(&slot) = keys.get(&key) {
                slot
            } else {
                let read = primitive.reader(|b| Some(&buffers[b.index()]));
                let positions: Vec<_> = read
                    .read_positions()
                    .context("missing positions")?
                    .collect();
                let normals: Vec<_> = read
                    .read_normals()
                    .context("missing normals; source must be imported first")?
                    .collect();
                let uv: Vec<_> = read
                    .read_tex_coords(0)
                    .map(|v| v.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.; 2]; positions.len()]);
                let uv1: Vec<_> = read
                    .read_tex_coords(1)
                    .map(|v| v.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.; 2]; positions.len()]);
                let tangents: Vec<_> = read
                    .read_tangents()
                    .map(Iterator::collect)
                    .unwrap_or_else(|| vec![[1., 0., 0., 1.]; positions.len()]);
                let bounds = positions.iter().fold(
                    [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)],
                    |b, &p| [b[0].min(Vec3::from(p)), b[1].max(Vec3::from(p))],
                );
                let vertices: Vec<_> = positions
                    .iter()
                    .enumerate()
                    .map(|(i, &position)| n::Vertex {
                        position,
                        normal: normals[i],
                        uv: uv[i],
                        tangent: tangents[i],
                        uv1: uv1[i],
                        terrain_weights: 0,
                        color: n::Vertex::WHITE,
                    })
                    .collect();
                let indices = read
                    .read_indices()
                    .map(|v| v.into_u32().collect())
                    .unwrap_or_else(|| (0..positions.len() as u32).collect());
                let key = Sha256::digest(bytemuck::cast_slice(&vertices)).into();
                let slot = scene.geometry.len();
                scene.geometry.push(n::Geometry {
                    key,
                    vertices,
                    lods: vec![n::Lod { indices, error: 0. }],
                    bounds,
                    deforming: false,
                    vertex_colors: false,
                    revision: 0,
                });
                keys.insert((mesh.index(), primitive.index()), slot);
                slot
            };
            scene.instances.push(n::Instance {
                entity: scene.instances.len() as u64 + 1,
                name: node.name().unwrap_or("glTF").into(),
                geometry,
                material: primitive.material().index().unwrap_or(default_material),
                world,
                previous_world: world,
                casts_shadows: true,
                paint: None,
                receives_decals: true,
            });
        }
    }
    for child in node.children() {
        visit(child, world, buffers, scene, keys, default_material)?;
    }
    Ok(())
}

pub fn world(f: &Fixture) -> scene::Scene {
    let mut textures = vec![
        scene::Texture {
            size: [1, 1],
            rgba: vec![255; 4],
            srgb: false,
        },
        scene::Texture {
            size: [1, 1],
            rgba: vec![128, 128, 255, 255],
            srgb: false,
        },
    ];
    textures.extend(f.textures.iter().map(|t| scene::Texture {
        size: t.size,
        rgba: t.rgba.to_vec(),
        srgb: t.srgb,
    }));
    let materials = f
        .scene
        .materials
        .iter()
        .map(|m| {
            let mut p = m.pbr;
            for (i, t) in p.textures.iter_mut().enumerate() {
                *t = if *t == n::NO_TEXTURE {
                    u32::from(i == 1)
                } else {
                    *t + 2
                };
            }
            p
        })
        .collect();
    let meshes = f
        .scene
        .instances
        .iter()
        .enumerate()
        .map(|(row, i)| {
            let g = &f.scene.geometry[i.geometry];
            let normal = i.world.inverse().transpose();
            let vertices = g
                .vertices
                .iter()
                .map(|v| scene::Vertex {
                    position: i.world.transform_point3(Vec3::from(v.position)).to_array(),
                    normal: normal
                        .transform_vector3(Vec3::from(v.normal))
                        .normalize()
                        .to_array(),
                    uv: v.uv,
                    tangent: v.tangent,
                })
                .collect();
            scene::Mesh {
                entity: row as u64 + 1,
                name: i.name.to_owned(),
                vertices,
                indices: g.lods[0].indices.to_vec(),
                material: i.material,
            }
        })
        .collect();
    scene::Scene {
        meshes,
        materials,
        textures,
        lights: f.scene.lights.to_vec(),
        sun_direction: f.scene.sun_direction,
        sun_lux: f.scene.sun_lux,
        environment: f.scene.environment.to_vec(),
        environment_size: f.scene.environment_size,
    }
}
