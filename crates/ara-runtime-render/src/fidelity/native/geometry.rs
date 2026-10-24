use super::*;
use ara_runtime::{extract::Geometry, model::ModelVertex};

impl Builder {
    pub(super) fn geometry(&mut self, draw: &Draw, frame: &RenderFrame) -> Result<usize> {
        let mut hash = Sha256::new();
        match &draw.geometry {
            Geometry::Shape { shape, size } => {
                hash.update(format!("{shape:?}"));
                hash.update(bytemuck::cast_slice(size));
            }
            Geometry::Model { model, primitive } => {
                hash.update(model.digest.0);
                hash.update(primitive.to_le_bytes());
            }
        }
        // Distinct animators may pose the same source independently. Rigid source
        // selections retain their existing Draw.world pivot and share geometry.
        if draw.skin.is_some() {
            hash.update(draw.entity.to_le_bytes());
        }
        let key: [u8; 32] = hash.finalize().into();
        if let Some(&index) = self.geometry_keys.get(&key) {
            return Ok(index);
        }
        let shape;
        let (vertices, indices, lods, extent) = match &draw.geometry {
            Geometry::Shape { shape: kind, size } => {
                shape = crate::geometry::shape(*kind, *size);
                (
                    shape.vertices.as_slice(),
                    shape.indices.as_slice(),
                    &[][..],
                    1.0,
                )
            }
            Geometry::Model { model, primitive } => {
                let p = &model.primitives[*primitive];
                (
                    p.vertices.as_slice(),
                    p.indices.as_slice(),
                    p.lods.as_slice(),
                    super::super::extent(model),
                )
            }
        };
        let mut native: Vec<_> = vertices.iter().map(vertex).collect();
        if let Some(range) = &draw.skin {
            let palette = frame
                .palettes
                .get(range.clone())
                .context("invalid skin palette range")?;
            let mut posed = Vec::new();
            crate::geometry::transform(vertices, glam::Mat4::IDENTITY, Some(palette), &mut posed);
            for (native, posed) in native.iter_mut().zip(posed) {
                native.position = posed.position;
                native.normal = posed.normal;
                native.tangent = posed.tangent;
            }
        }
        let mut bounds = [
            glam30::Vec3::splat(f32::INFINITY),
            glam30::Vec3::splat(f32::NEG_INFINITY),
        ];
        for vertex in &native {
            let position = glam30::Vec3::from_array(vertex.position);
            bounds[0] = bounds[0].min(position);
            bounds[1] = bounds[1].max(position);
        }
        let mut native_lods = vec![render::Lod {
            indices: indices.to_vec(),
            error: 0.0,
        }];
        for lod in lods {
            if lod.indices != indices && !lod.indices.is_empty() {
                native_lods.push(render::Lod {
                    indices: lod.indices.to_vec(),
                    error: lod.error * extent,
                });
            }
        }
        let index = self.scene.geometry.len();
        self.scene.geometry.push(render::Geometry {
            key,
            vertices: native,
            lods: native_lods,
            bounds,
            deforming: draw.skin.is_some(),
            revision: 0,
        });
        self.geometry_keys.insert(key, index);
        Ok(index)
    }
}

fn vertex(v: &ModelVertex) -> render::Vertex {
    render::Vertex {
        position: v.position,
        normal: v.normal,
        uv: v.uv,
        tangent: v.tangent,
        uv1: v.uv1,
        terrain_weights: 0,
        reserved: 0,
    }
}
