//! Decodes one cooked glTF import generation (`scene.json` manifest plus mesh,
//! texture, skeleton and clip artifacts, FORMAT.md v1) into renderable primitives,
//! materials and animation-ready skins.
use crate::{Result, RuntimeError, skin::Skin};
use ara_asset::{CachedImport, Digest};
use ara_asset_format::{Lod, Mesh, QuantizedVertex, SkinExtra};
use glam::{Mat4, Quat, Vec3};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

/// A decoded model-space vertex with up to eight skin influences.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ModelVertex {
    /// Position in model space (metres before entity scale).
    pub position: [f32; 3],
    /// Unit normal.
    pub normal: [f32; 3],
    /// First texture coordinate.
    pub uv: [f32; 2],
    /// Tangent xyz and bitangent handedness.
    pub tangent: [f32; 4],
    /// Skin-local joint indices (the owning skin's joint order).
    pub joints: [u16; 8],
    /// Normalized influence weights; all zero for rigid vertices.
    pub weights: [f32; 8],
}

/// One drawable triangle list.
#[derive(Debug, Clone)]
pub struct Primitive {
    /// Vertices in model space; rigid primitives already include their node transform.
    pub vertices: Vec<ModelVertex>,
    /// Finest-LOD triangle indices.
    pub indices: Vec<u32>,
    /// Index into [`Model::materials`].
    pub material: Option<usize>,
    /// Index into [`Model::skins`] for skinned primitives.
    pub skin: Option<usize>,
}

/// Metallic-roughness material factors and texture slots.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelMaterial {
    /// Linear base color factor.
    pub base_color: [f32; 4],
    /// Metalness factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
    /// Emissive factor (multiplied by the entity's emissive radiance scale).
    pub emissive: [f32; 3],
    /// Base color texture, an index into [`Model::textures`].
    pub base_color_texture: Option<usize>,
    /// Alpha mode: 0 opaque, 1 mask, 2 blend.
    pub alpha_mode: u32,
    /// Mask cutoff.
    pub alpha_cutoff: f32,
}

/// A cooked KTX2 texture, decoded by the presentation layer.
#[derive(Debug, Clone)]
pub struct ModelTexture {
    /// Artifact name, stable within the generation.
    pub name: String,
    /// Complete KTX2 bytes.
    pub ktx2: Arc<[u8]>,
}

/// An immutable decoded import generation.
#[derive(Debug)]
pub struct Model {
    /// Stable asset ID.
    pub asset: String,
    /// Import action digest identifying this generation.
    pub digest: Digest,
    /// Drawable primitives in manifest order.
    pub primitives: Vec<Primitive>,
    /// Materials in source order.
    pub materials: Vec<ModelMaterial>,
    /// Cooked textures referenced by materials.
    pub textures: Vec<ModelTexture>,
    /// Skins with cooked skeletons and source clips.
    pub skins: Vec<Arc<Skin>>,
    /// Model-space bounds (min, max) of every primitive at rest.
    pub bounds: [[f32; 3]; 2],
}

impl Model {
    /// Decodes a glTF import generation. Other source formats report a clear error.
    pub fn decode(asset: &str, digest: Digest, cached: &CachedImport) -> Result<Self> {
        let artifact = |name: &str| -> Result<&[u8]> {
            cached
                .output
                .artifacts
                .iter()
                .find(|a| a.name == name)
                .map(|a| a.bytes.as_slice())
                .ok_or_else(|| RuntimeError(format!("{asset}: missing artifact {name}")))
        };
        let manifest: Value = serde_json::from_slice(artifact("scene.json")?)?;
        if manifest["sourceFormat"] != "gltf2" {
            return Err(RuntimeError(format!(
                "{asset}: runtime models support glTF imports; got {}",
                manifest["sourceFormat"]
            )));
        }
        let document = &manifest["document"];
        let nodes = document["nodes"].as_array().cloned().unwrap_or_default();
        let parents = node_parents(&nodes);
        let locals: Vec<Mat4> = nodes.iter().map(node_matrix).collect();
        let world = |node: usize| -> Mat4 {
            let mut m = locals[node];
            let mut cursor = parents.get(&node).copied();
            while let Some(p) = cursor {
                m = locals[p] * m;
                cursor = parents.get(&p).copied();
            }
            m
        };

        let mut textures = Vec::new();
        let mut texture_by_image = BTreeMap::new();
        for entry in manifest["textures"].as_array().into_iter().flatten() {
            if entry["usage"] == "color" {
                let name = string(&entry["artifact"])?;
                texture_by_image.insert(index(&entry["image"])?, textures.len());
                textures.push(ModelTexture {
                    name: name.to_owned(),
                    ktx2: Arc::from(artifact(name)?),
                });
            }
        }
        let gltf_textures = document["textures"].as_array().cloned().unwrap_or_default();
        let materials = document["materials"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| material(m, &gltf_textures, &texture_by_image))
            .collect::<Vec<_>>();

        let mut skins = Vec::new();
        for entry in manifest["skeletons"].as_array().into_iter().flatten() {
            let skin_index = index(&entry["skin"])?;
            let clips = manifest["clips"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    let animation = index(&c["animation"])?;
                    let name = document["animations"][animation]["name"]
                        .as_str()
                        .map_or_else(|| format!("animation{animation}"), str::to_owned);
                    Ok((name, artifact(string(&c["artifact"])?)?))
                })
                .collect::<Result<Vec<_>>>()?;
            if skins.len() != skin_index {
                return Err(RuntimeError(format!("{asset}: skins out of order")));
            }
            skins.push(Arc::new(Skin::cook(
                artifact(string(&entry["artifact"])?)?,
                &clips,
                &locals,
                &parents,
            )?));
        }

        let mut by_mesh: BTreeMap<(usize, usize), (&str, Option<usize>)> = BTreeMap::new();
        for entry in manifest["meshes"].as_array().into_iter().flatten() {
            by_mesh.insert(
                (index(&entry["mesh"])?, index(&entry["primitive"])?),
                (
                    string(&entry["artifact"])?,
                    entry["material"].as_u64().map(|m| m as usize),
                ),
            );
        }
        let mut primitives = Vec::new();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for node in scene_nodes(document, &nodes) {
            let Some(mesh) = nodes[node]["mesh"].as_u64().map(|m| m as usize) else {
                continue;
            };
            let skin = nodes[node]["skin"].as_u64().map(|s| s as usize);
            let transform = if skin.is_some() {
                Mat4::IDENTITY
            } else {
                world(node)
            };
            for (&(_, primitive), &(name, material)) in by_mesh.range((mesh, 0)..(mesh + 1, 0)) {
                let _ = primitive;
                let mut decoded = decode_mesh(artifact(name)?, transform)?;
                if skin.is_some_and(|s| s >= skins.len()) {
                    return Err(RuntimeError(format!("{asset}: unknown skin")));
                }
                decoded.material = material.filter(|&m| m < materials.len());
                decoded.skin = skin;
                for v in &decoded.vertices {
                    min = min.min(Vec3::from_array(v.position));
                    max = max.max(Vec3::from_array(v.position));
                }
                primitives.push(decoded);
            }
        }
        if primitives.is_empty() {
            return Err(RuntimeError(format!(
                "{asset}: model has no drawable meshes"
            )));
        }
        Ok(Self {
            asset: asset.to_owned(),
            digest,
            primitives,
            materials,
            textures,
            skins,
            bounds: [min.to_array(), max.to_array()],
        })
    }
}

fn index(value: &Value) -> Result<usize> {
    value
        .as_u64()
        .map(|v| v as usize)
        .ok_or_else(|| RuntimeError(format!("expected an index, got {value}")))
}

fn string(value: &Value) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| RuntimeError(format!("expected a string, got {value}")))
}

fn floats<const N: usize>(value: &Value, default: [f32; N]) -> [f32; N] {
    let mut out = default;
    if let Some(items) = value.as_array() {
        for (slot, item) in out.iter_mut().zip(items) {
            *slot = item.as_f64().map_or(*slot, |v| v as f32);
        }
    }
    out
}

fn material(m: &Value, textures: &[Value], by_image: &BTreeMap<usize, usize>) -> ModelMaterial {
    let pbr = &m["pbrMetallicRoughness"];
    let base_color_texture = pbr["baseColorTexture"]["index"]
        .as_u64()
        .and_then(|t| textures.get(t as usize))
        .and_then(|t| t["source"].as_u64())
        .and_then(|image| by_image.get(&(image as usize)).copied());
    ModelMaterial {
        base_color: floats(&pbr["baseColorFactor"], [1.0; 4]),
        metallic: pbr["metallicFactor"].as_f64().map_or(1.0, |v| v as f32),
        roughness: pbr["roughnessFactor"].as_f64().map_or(1.0, |v| v as f32),
        emissive: floats(&m["emissiveFactor"], [0.0; 3]),
        base_color_texture,
        alpha_mode: match m["alphaMode"].as_str() {
            Some("MASK") => 1,
            Some("BLEND") => 2,
            _ => 0,
        },
        alpha_cutoff: m["alphaCutoff"].as_f64().map_or(0.5, |v| v as f32),
    }
}

/// Node → parent node.
pub(crate) fn node_parents(nodes: &[Value]) -> BTreeMap<usize, usize> {
    let mut parents = BTreeMap::new();
    for (parent, node) in nodes.iter().enumerate() {
        for child in node["children"].as_array().into_iter().flatten() {
            if let Some(child) = child.as_u64() {
                parents.insert(child as usize, parent);
            }
        }
    }
    parents
}

/// A glTF node's local matrix (explicit matrix or TRS).
pub(crate) fn node_matrix(node: &Value) -> Mat4 {
    if let Some(m) = node["matrix"].as_array().filter(|m| m.len() == 16) {
        let cols: Vec<f32> = m.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect();
        return Mat4::from_cols_slice(&cols);
    }
    let t = floats(&node["translation"], [0.0; 3]);
    let r = floats(&node["rotation"], [0.0, 0.0, 0.0, 1.0]);
    let s = floats(&node["scale"], [1.0; 3]);
    Mat4::from_scale_rotation_translation(
        Vec3::from_array(s),
        Quat::from_array(r).normalize(),
        Vec3::from_array(t),
    )
}

/// Nodes of the default scene, depth first in child order.
fn scene_nodes(document: &Value, nodes: &[Value]) -> Vec<usize> {
    let scene = document["scene"].as_u64().unwrap_or(0) as usize;
    let mut stack: Vec<usize> = document["scenes"][scene]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .map(|n| n as usize)
        .rev()
        .collect();
    let mut out = Vec::new();
    while let Some(node) = stack.pop() {
        if node >= nodes.len() || out.contains(&node) {
            continue;
        }
        out.push(node);
        let children: Vec<usize> = nodes[node]["children"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .map(|n| n as usize)
            .collect();
        stack.extend(children.into_iter().rev());
    }
    out
}

/// IEEE half to f32.
fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        e => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Decodes the finest LOD of a cooked mesh, applying `transform` to rigid geometry.
fn decode_mesh(bytes: &[u8], transform: Mat4) -> Result<Primitive> {
    let mesh = Mesh::read(bytes)?;
    let bounds: [f32; 6] = bytemuck::pod_read_unaligned(mesh.bounds);
    let min = Vec3::new(bounds[0], bounds[1], bounds[2]);
    let extent = Vec3::new(bounds[3], bounds[4], bounds[5]) - min;
    let normals = transform.inverse().transpose();
    let vertices = mesh
        .vertices
        .as_chunks::<52>()
        .0
        .iter()
        .zip(mesh.skin_extra.as_chunks::<16>().0)
        .map(|(raw, extra)| {
            let q: QuantizedVertex = bytemuck::pod_read_unaligned(raw);
            let x: SkinExtra = bytemuck::pod_read_unaligned(extra);
            let unorm = |v: u16| f32::from(v) / 65535.0;
            let snorm = |v: i16| (f32::from(v) / 32767.0).max(-1.0);
            let p = min
                + Vec3::new(
                    unorm(q.position[0]),
                    unorm(q.position[1]),
                    unorm(q.position[2]),
                ) * extent;
            let n = Vec3::new(snorm(q.normal[0]), snorm(q.normal[1]), snorm(q.normal[2]));
            let t = Vec3::new(
                snorm(q.tangent[0]),
                snorm(q.tangent[1]),
                snorm(q.tangent[2]),
            );
            let n = normals.transform_vector3(n).normalize_or(Vec3::Y);
            let t = transform
                .transform_vector3(t)
                .normalize_or(n.any_orthonormal_vector());
            let mut weights = [0.0; 8];
            let mut joints = [0u16; 8];
            for i in 0..4 {
                joints[i] = q.joints[i];
                weights[i] = unorm(q.weights[i]);
                joints[i + 4] = x.joints[i];
                weights[i + 4] = unorm(x.weights[i]);
            }
            let total: f32 = weights.iter().sum();
            if total > 0.0 {
                weights.iter_mut().for_each(|w| *w /= total);
            }
            ModelVertex {
                position: transform.transform_point3(p).to_array(),
                normal: n.to_array(),
                uv: [half(q.uv[0]), half(q.uv[1])],
                tangent: [t.x, t.y, t.z, if q.tangent[3] < 0 { -1.0 } else { 1.0 }],
                joints,
                weights,
            }
        })
        .collect();
    let lod: Lod = bytemuck::pod_read_unaligned(
        mesh.lods
            .get(..16)
            .ok_or_else(|| RuntimeError("mesh has no LOD".into()))?,
    );
    let first = lod.first_index as usize * 4;
    let indices = mesh.indices[first..first + lod.index_count as usize * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect();
    Ok(Primitive {
        vertices,
        indices,
        material: None,
        skin: None,
    })
}

#[cfg(test)]
mod tests {
    use super::half;

    #[test]
    fn half_floats_decode() {
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0xc000), -2.0);
        assert_eq!(half(0x3800), 0.5);
        assert_eq!(half(0), 0.0);
    }
}
