//! Strict fixture-only glTF import for pinned CC0 PBR content, not the engine importer.
use anyhow::{Context, Result, ensure};
use ara_render::scene::{Material, Mesh, Scene, Texture, Vertex};
use glam::{Mat4, Quat, Vec2, Vec3};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

fn index(v: &Value) -> Result<usize> {
    Ok(v.as_u64().context("Missing glTF index")? as usize)
}
fn vec<const N: usize>(v: &Value, default: [f32; N]) -> [f32; N] {
    std::array::from_fn(|i| v[i].as_f64().map_or(default[i], |x| x as f32))
}
fn accessor<const N: usize>(doc: &Value, bin: &[u8], id: usize) -> Result<Vec<[f32; N]>> {
    let a = &doc["accessors"][id];
    ensure!(
        a["componentType"] == 5126 && a["sparse"].is_null(),
        "Fixture needs dense float attributes"
    );
    let view = &doc["bufferViews"][index(&a["bufferView"])?];
    let offset = index(&view["byteOffset"]).unwrap_or(0) + index(&a["byteOffset"]).unwrap_or(0);
    let stride = index(&view["byteStride"]).unwrap_or(N * 4);
    let count = index(&a["count"])?;
    ensure!(
        count > 0 && offset + (count - 1) * stride + N * 4 <= bin.len(),
        "glTF attribute outside buffer"
    );
    Ok((0..count)
        .map(|i| {
            std::array::from_fn(|j| {
                let b = offset + i * stride + j * 4;
                f32::from_le_bytes([bin[b], bin[b + 1], bin[b + 2], bin[b + 3]])
            })
        })
        .collect())
}
fn texture(
    scene: &mut Scene,
    doc: &Value,
    path: &Path,
    id: usize,
    srgb: bool,
    cache: &mut BTreeMap<(usize, bool), u32>,
) -> Result<u32> {
    let source = index(&doc["textures"][id]["source"])?;
    if let Some(t) = cache.get(&(source, srgb)) {
        return Ok(*t);
    }
    let uri = doc["images"][source]["uri"]
        .as_str()
        .context("Fixture requires external images")?;
    ensure!(
        !uri.contains("..") && !Path::new(uri).is_absolute(),
        "Invalid fixture image URI"
    );
    let image = image::open(path.join(uri))?.to_rgba8();
    let id = scene.textures.len() as u32;
    scene.textures.push(Texture {
        size: [image.width(), image.height()],
        rgba: image.into_raw(),
        srgb,
    });
    cache.insert((source, srgb), id);
    Ok(id)
}
/// Append extracted world-space triangles and authored PBR channels from pinned glTF.
pub(crate) fn append(scene: &mut Scene, path: &Path, placement: Mat4) -> Result<()> {
    let doc: Value = serde_json::from_slice(&std::fs::read(path.join("model.gltf"))?)?;
    ensure!(
        doc["buffers"].as_array().is_some_and(|a| a.len() == 1),
        "Fixture expects one external buffer"
    );
    let uri = doc["buffers"][0]["uri"]
        .as_str()
        .context("Missing glTF buffer")?;
    ensure!(
        !uri.contains("..") && !Path::new(uri).is_absolute(),
        "Invalid fixture buffer URI"
    );
    let bin = std::fs::read(path.join(uri))?;
    let material_base = scene.materials.len();
    let mut cache = BTreeMap::new();
    for source in doc["materials"].as_array().context("Missing materials")? {
        let pbr = &source["pbrMetallicRoughness"];
        let mut material = Material {
            base_color: vec(&pbr["baseColorFactor"], [1.; 4]),
            emissive_roughness: [
                0.,
                0.,
                0.,
                pbr["roughnessFactor"].as_f64().unwrap_or(1.) as f32,
            ],
            surface: [
                pbr["metallicFactor"].as_f64().unwrap_or(1.) as f32,
                source["normalTexture"]["scale"].as_f64().unwrap_or(1.) as f32,
                0.,
                source["alphaCutoff"].as_f64().unwrap_or(0.5) as f32,
            ],
            ..Default::default()
        };
        material.emissive_roughness[..3].copy_from_slice(&vec(&source["emissiveFactor"], [0.; 3]));
        for (slot, entry, srgb) in [
            (0, &pbr["baseColorTexture"], true),
            (1, &source["normalTexture"], false),
            (2, &pbr["metallicRoughnessTexture"], false),
            (3, &source["emissiveTexture"], true),
        ] {
            if let Some(id) = entry["index"].as_u64() {
                material.textures[slot] =
                    texture(scene, &doc, path, id as usize, srgb, &mut cache)?;
            }
        }
        material.flags[0] = match source["alphaMode"].as_str() {
            Some("MASK") => 1,
            Some("BLEND") => 2,
            _ => 0,
        };
        scene.materials.push(material);
    }
    let nodes = doc["nodes"].as_array().context("Missing nodes")?;
    let roots = doc["scenes"][index(&doc["scene"]).unwrap_or(0)]["nodes"]
        .as_array()
        .context("Missing glTF scene")?;
    let mut pending = roots
        .iter()
        .map(|id| Ok((index(id)?, placement)))
        .collect::<Result<Vec<_>>>()?;
    while let Some((id, parent)) = pending.pop() {
        let node = nodes.get(id).context("Invalid node")?;
        let local = if node["matrix"].is_array() {
            Mat4::from_cols_array(&vec(&node["matrix"], [0.; 16]))
        } else {
            Mat4::from_scale_rotation_translation(
                Vec3::from_array(vec(&node["scale"], [1.; 3])),
                Quat::from_array(vec(&node["rotation"], [0., 0., 0., 1.])),
                Vec3::from_array(vec(&node["translation"], [0.; 3])),
            )
        };
        let world = parent * local;
        if let Some(children) = node["children"].as_array() {
            for c in children {
                pending.push((index(c)?, world));
            }
        }
        let Some(id) = node["mesh"].as_u64() else {
            continue;
        };
        for primitive in doc["meshes"][id as usize]["primitives"]
            .as_array()
            .context("Missing primitives")?
        {
            ensure!(
                primitive["mode"].as_u64().unwrap_or(4) == 4,
                "Fixture requires triangles"
            );
            let a = &primitive["attributes"];
            let positions = accessor::<3>(&doc, &bin, index(&a["POSITION"])?)?;
            let normals = accessor::<3>(&doc, &bin, index(&a["NORMAL"])?)?;
            let uvs = accessor::<2>(&doc, &bin, index(&a["TEXCOORD_0"])?)?;
            ensure!(
                positions.len() == normals.len() && positions.len() == uvs.len(),
                "Attribute counts differ"
            );
            let normal_matrix = world.inverse().transpose();
            let mut mesh = Mesh {
                entity: scene.meshes.len() as u64 + 1,
                name: path
                    .file_name()
                    .context("Asset name")?
                    .to_string_lossy()
                    .into_owned(),
                material: material_base + index(&primitive["material"]).unwrap_or(0),
                indices: Vec::new(),
                vertices: Vec::new(),
            };
            for i in 0..positions.len() {
                mesh.vertices.push(Vertex {
                    position: world
                        .transform_point3(Vec3::from_array(positions[i]))
                        .to_array(),
                    normal: normal_matrix
                        .transform_vector3(Vec3::from_array(normals[i]))
                        .normalize()
                        .to_array(),
                    uv: uvs[i],
                    tangent: [1., 0., 0., 1.],
                });
            }
            let a = &doc["accessors"][index(&primitive["indices"])?];
            let view = &doc["bufferViews"][index(&a["bufferView"])?];
            let offset =
                index(&view["byteOffset"]).unwrap_or(0) + index(&a["byteOffset"]).unwrap_or(0);
            let bytes = if a["componentType"] == 5123 {
                2
            } else {
                ensure!(a["componentType"] == 5125, "Unsupported indices");
                4
            };
            let count = index(&a["count"])?;
            ensure!(
                offset + count * bytes <= bin.len(),
                "Index buffer outside file"
            );
            for b in bin[offset..offset + count * bytes].chunks_exact(bytes) {
                mesh.indices.push(if bytes == 2 {
                    u16::from_le_bytes([b[0], b[1]]) as u32
                } else {
                    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
                });
            }
            ensure!(
                mesh.indices.iter().all(|i| (*i as usize) < positions.len()),
                "Invalid vertex index"
            );
            if world.determinant() < 0. {
                for t in mesh.indices.as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
            }
            tangents(&mut mesh);
            scene.meshes.push(mesh);
        }
    }
    Ok(())
}
fn tangents(mesh: &mut Mesh) {
    let mut tangent = vec![Vec3::ZERO; mesh.vertices.len()];
    let mut bitangent = vec![Vec3::ZERO; mesh.vertices.len()];
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        let e1 = Vec3::from_array(mesh.vertices[b].position)
            - Vec3::from_array(mesh.vertices[a].position);
        let e2 = Vec3::from_array(mesh.vertices[c].position)
            - Vec3::from_array(mesh.vertices[a].position);
        let u1 = Vec2::from_array(mesh.vertices[b].uv) - Vec2::from_array(mesh.vertices[a].uv);
        let u2 = Vec2::from_array(mesh.vertices[c].uv) - Vec2::from_array(mesh.vertices[a].uv);
        let det = u1.x * u2.y - u1.y * u2.x;
        if det.abs() < 1e-10 {
            continue;
        }
        let t = (e1 * u2.y - e2 * u1.y) / det;
        let bt = (e2 * u1.x - e1 * u2.x) / det;
        for i in [a, b, c] {
            tangent[i] += t;
            bitangent[i] += bt;
        }
    }
    for (i, vertex) in mesh.vertices.iter_mut().enumerate() {
        let n = Vec3::from_array(vertex.normal);
        let t = (tangent[i] - n * n.dot(tangent[i]))
            .try_normalize()
            .unwrap_or_else(|| n.any_orthonormal_vector());
        vertex.tangent = t
            .extend(if n.cross(t).dot(bitangent[i]) < 0. {
                -1.
            } else {
                1.
            })
            .to_array();
    }
}
