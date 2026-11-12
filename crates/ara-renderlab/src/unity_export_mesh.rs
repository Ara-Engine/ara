//! Mesh half of the Unity comparison export (`unity_export`): a draw's geometry as one glTF
//! mesh, shared by content and posed per draw when skinned.
use crate::{
    unity_export::{Export, hex},
    unity_gltf::Writer,
};
use anyhow::{Context, Result};
use ara_runtime::{
    extract::{Draw, Geometry, RenderFrame},
    model::ModelVertex,
};
use serde_json::json;


pub(crate) fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l.is_finite() && l > 1e-12 { v.map(|c| c / l) } else { [0., 1., 0.] }
}

/// `m` (column-major) applied to a point (`w` 1) or a direction (`w` 0).
fn transform(m: &[f32; 16], v: [f32; 3], w: f32) -> [f32; 3] {
    std::array::from_fn(|r| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r] * w)
}

/// A skinned primitive's vertices at the palette's pose: each position and normal moves by
/// its weighted joints (up to eight), unposed where the vertex has none.
fn pose(vertices: &[ModelVertex], palette: &[glam34::Mat4]) -> Vec<ModelVertex> {
    let m: Vec<[f32; 16]> = palette.iter().map(|m| m.to_cols_array()).collect();
    vertices
        .iter()
        .map(|v| {
            let mut out = *v;
            let (mut p, mut n, mut total) = ([0f32; 3], [0f32; 3], 0f32);
            for (joint, weight) in v.joints.iter().zip(&v.weights) {
                let Some(jm) = m.get(*joint as usize).filter(|_| *weight > 0.) else {
                    continue;
                };
                let (tp, tn) = (transform(jm, v.position, 1.), transform(jm, v.normal, 0.));
                for k in 0..3 {
                    p[k] += weight * tp[k];
                    n[k] += weight * tn[k];
                }
                total += weight;
            }
            if total > 0. {
                out.position = p.map(|c| c / total);
                out.normal = normalize(n);
                // Skinned meshes recompute tangents on import.
                out.tangent = [0.; 4];
            }
            out
        })
        .collect()
}

impl Export {
    /// The glTF mesh for a draw: shared by content and material, posed per draw when skinned.
    pub(crate) fn mesh(
        &mut self,
        w: &mut Writer,
        frame: &RenderFrame,
        draw: &Draw,
        material: usize,
        with_tangents: bool,
    ) -> Result<usize> {
        let skinned = draw.skin.is_some();
        let key = match &draw.geometry {
            Geometry::Shape { shape, size } => {
                format!("shape:{}:{:?}:{material}", *shape as u8, size.map(f32::to_bits))
            }
            Geometry::Model { model, primitive } if skinned => {
                format!("skin:{:x}:{material}", draw.entity.wrapping_add(*primitive as u64))
            }
            Geometry::Model { model, primitive } => {
                format!("model:{}:{primitive}:{material}", hex(&model.digest.0))
            }
        };
        if let Some(index) = self.meshes.get(&key) {
            return Ok(*index);
        }
        let (name, vertices, indices, colored) = match &draw.geometry {
            Geometry::Shape { shape, size } => {
                let s = ara_runtime_render::geometry::shape(*shape, *size);
                ("shape".to_owned(), s.vertices, s.indices, false)
            }
            Geometry::Model { model, primitive } => {
                let p = model
                    .primitives
                    .get(*primitive)
                    .context("primitive out of range")?;
                let vertices = match &draw.skin {
                    Some(range) => pose(&p.vertices, &frame.palettes[range.clone()]),
                    None => p.vertices.clone(),
                };
                (model.asset.clone(), vertices, p.indices.clone(), p.vertex_colors)
            }
        };
        let mut positions = Vec::with_capacity(vertices.len() * 3);
        let mut normals = Vec::with_capacity(vertices.len() * 3);
        let mut uvs = Vec::with_capacity(vertices.len() * 2);
        for v in &vertices {
            positions.extend_from_slice(&v.position);
            normals.extend_from_slice(&normalize(v.normal));
            uvs.extend_from_slice(&v.uv);
        }
        let mut attributes = json!({
            "POSITION": w.floats(&positions, 3, true),
            "NORMAL": w.floats(&normals, 3, false),
            "TEXCOORD_0": w.floats(&uvs, 2, false),
        });
        // Tangents only where every one is usable; glTFast derives the rest.
        if with_tangents
            && vertices.iter().all(|v| {
                let t = v.tangent;
                t.iter().all(|c| c.is_finite())
                    && ((t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt() - 1.).abs() < 0.05
                    && t[3].abs() > 0.5
            })
        {
            let tangents: Vec<f32> = vertices.iter().flat_map(|v| v.tangent).collect();
            attributes["TANGENT"] = json!(w.floats(&tangents, 4, false));
        }
        if colored {
            let colors: Vec<u8> = vertices.iter().flat_map(|v| v.color).collect();
            attributes["COLOR_0"] = json!(w.colors(&colors));
        }
        let index = w.indices(&indices);
        let mesh = w.mesh(&name, attributes, index, material);
        self.meshes.insert(key, mesh);
        Ok(mesh)
    }
}
