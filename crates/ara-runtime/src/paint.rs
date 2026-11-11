//! Per-instance vertex paint (Unreal's mesh-paint override colours): the reflected
//! [`VertexPaint`] component, its compact text encoding, the typed `vertexPaint.*`
//! commands and the extraction cache that maps source-vertex colours onto a
//! primitive's cooked vertices.
//!
//! Colours are stored per *source* vertex (the glTF accessor element or FBX corner the
//! cook recorded in `VSRC`), so paint survives re-cooks, LOD and tangent-split changes:
//! every LOD indexes the same vertices, so one stream covers them all.
use crate::model::Primitive;
use ara_schema::Schema;
use base64::Engine;
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Write, sync::Arc};

pub mod commands;

/// Vertex colours painted onto this entity's `MeshRenderer` (its instance only; the
/// shared model stays untouched). Materials read them as `vertexColor()`: glTF
/// materials multiply base colour by them, graph materials blend layers by them.
#[derive(Debug, Clone, Default, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct VertexPaint {
    /// Painted primitives by subtree-local slot (as `MeshRenderer.materialOverrides`);
    /// unlisted primitives keep their imported colours.
    pub primitives: Vec<PaintedPrimitive>,
}

/// One primitive's painted colours.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaintedPrimitive {
    /// Zero-based primitive within the selected subtree, before `primitive` filtering.
    // The canonical writer omits the default slot 0, so reading must default it.
    #[serde(default)]
    pub primitive: u32,
    /// Source vertices the colours cover (the imported primitive's vertex count). A
    /// re-import that changes it leaves the primitive unpainted until repainted.
    pub vertices: u32,
    /// Linear RGBA8 per source vertex: zlib-compressed, then standard base64.
    pub colors: String,
}

/// Encodes linear RGBA8 colours for [`PaintedPrimitive::colors`] (deterministic).
pub fn encode(colors: &[[u8; 4]]) -> String {
    let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    zlib.write_all(colors.as_flattened())
        .and_then(|()| zlib.finish())
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
        .expect("compressing into memory cannot fail")
}

/// Decodes [`PaintedPrimitive::colors`].
pub fn decode(colors: &str) -> Result<Vec<[u8; 4]>, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(colors)
        .map_err(|e| format!("paint colours are not base64: {e}"))?;
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&bytes[..]), &mut raw)
        .map_err(|e| format!("paint colours are not zlib data: {e}"))?;
    let (colors, rest) = raw.as_chunks::<4>();
    if !rest.is_empty() {
        return Err("paint colours are not whole RGBA8 values".into());
    }
    Ok(colors.to_vec())
}

impl PaintedPrimitive {
    /// Paint for slot `primitive` covering `colors.len()` source vertices.
    pub fn new(primitive: u32, colors: &[[u8; 4]]) -> Self {
        Self {
            primitive,
            vertices: colors.len() as u32,
            colors: encode(colors),
        }
    }
    /// The decoded colours, checked against `vertices`.
    pub fn decode(&self) -> Result<Vec<[u8; 4]>, String> {
        let colors = decode(&self.colors)?;
        if colors.len() != self.vertices as usize {
            return Err(format!(
                "paint holds {} colours for {} vertices",
                colors.len(),
                self.vertices
            ));
        }
        Ok(colors)
    }
}

impl VertexPaint {
    /// The paint of subtree-local slot `primitive`.
    pub fn get(&self, primitive: u32) -> Option<&PaintedPrimitive> {
        self.primitives.iter().find(|p| p.primitive == primitive)
    }
    /// Replaces (or adds) slot `paint.primitive`, keeping slots sorted.
    pub fn set(&mut self, paint: PaintedPrimitive) {
        match self
            .primitives
            .binary_search_by_key(&paint.primitive, |p| p.primitive)
        {
            Ok(i) => self.primitives[i] = paint,
            Err(i) => self.primitives.insert(i, paint),
        }
    }
}

/// One `u32` colour (`ara_frame::instanced::Vertex::color`, red in the low byte) per
/// cooked vertex of `primitive`, from per-source-vertex `colors`; `None` when the
/// colours were painted for a different source vertex count.
pub fn cooked(primitive: &Primitive, colors: &[[u8; 4]]) -> Option<Arc<[u32]>> {
    (colors.len() == primitive.source_vertex_count()).then(|| {
        (0..primitive.vertices.len())
            .map(|i| u32::from_le_bytes(colors[primitive.source_vertex(i)]))
            .collect()
    })
}

/// Identifies a [`PaintedPrimitive::colors`] string without hashing it: its
/// allocation, length and both ends (the zlib trailer checksums the content).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    address: usize,
    length: usize,
    head: [u8; 16],
    tail: [u8; 16],
}

impl Fingerprint {
    fn of(colors: &str) -> Self {
        let bytes = colors.as_bytes();
        let mut head = [0; 16];
        let mut tail = [0; 16];
        let n = bytes.len().min(16);
        head[..n].copy_from_slice(&bytes[..n]);
        tail[..n].copy_from_slice(&bytes[bytes.len() - n..]);
        Self {
            address: bytes.as_ptr() as usize,
            length: bytes.len(),
            head,
            tail,
        }
    }
}

/// One draw's decoded paint and what it was decoded from.
#[derive(Debug)]
struct Cached {
    fingerprint: Fingerprint,
    digest: ara_asset::Digest,
    primitive: usize,
    colors: Option<Arc<[u32]>>,
}

/// Decoded paint by draw, reused across extractions while the component's colours,
/// the model generation and the primitive stay the same (painted frames cost one
/// lookup per draw, not a decode).
#[derive(Debug, Default)]
pub struct PaintCache {
    entries: HashMap<u64, Cached>,
    seen: Vec<u64>,
}

impl PaintCache {
    /// Colours of draw `id` (model `digest`, primitive `index`; procedural shapes pass
    /// a default digest), decoding on change and mapping through `cook` ([`cooked`],
    /// or identity for shapes). Undecodable or mismatched paint draws unpainted.
    pub(crate) fn get(
        &mut self,
        id: u64,
        paint: &PaintedPrimitive,
        digest: ara_asset::Digest,
        index: usize,
        cook: impl FnOnce(&[[u8; 4]]) -> Option<Arc<[u32]>>,
    ) -> Option<Arc<[u32]>> {
        self.seen.push(id);
        let fingerprint = Fingerprint::of(&paint.colors);
        if let Some(cached) = self.entries.get(&id)
            && (cached.fingerprint, cached.digest, cached.primitive) == (fingerprint, digest, index)
        {
            return cached.colors.clone();
        }
        let colors = paint.decode().ok().and_then(|colors| cook(&colors));
        self.entries.insert(
            id,
            Cached {
                fingerprint,
                digest,
                primitive: index,
                colors: colors.clone(),
            },
        );
        colors
    }
    /// Drops entries no draw used since the last call (removed paint or entities).
    pub(crate) fn retain_seen(&mut self) {
        let seen = std::mem::take(&mut self.seen);
        if seen.len() != self.entries.len() {
            let seen: std::collections::HashSet<_> = seen.iter().copied().collect();
            self.entries.retain(|id, _| seen.contains(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_round_trip_deterministically_and_compress() {
        let colors: Vec<_> = (0..4096u32)
            .map(|i| {
                if i < 3000 {
                    [255; 4]
                } else {
                    [i as u8, 0, 0, 255]
                }
            })
            .collect();
        let encoded = encode(&colors);
        assert_eq!(encode(&colors), encoded, "deterministic");
        assert!(
            encoded.len() < colors.len() * 4 / 4,
            "mostly-white paint compresses"
        );
        assert_eq!(decode(&encoded).expect("decodes"), colors);
        assert!(decode("not base64!").is_err());
        assert!(decode(&base64::engine::general_purpose::STANDARD.encode([1, 2, 3])).is_err());
    }

    #[test]
    fn cooked_vertices_read_their_source_vertex() {
        let primitive = Primitive {
            vertices: vec![Default::default(); 4],
            indices: vec![0, 1, 2],
            lods: Vec::new(),
            material: None,
            skin: None,
            casts_shadows: true,
            // A tangent split duplicated source vertex 1.
            source_vertices: vec![0, 1, 1, 2],
            vertex_colors: false,
        };
        let colors = [[10, 0, 0, 255], [20, 0, 0, 255], [30, 0, 0, 255]];
        let cooked = cooked(&primitive, &colors).expect("matching counts");
        assert_eq!(
            cooked
                .iter()
                .map(|c| c.to_le_bytes()[0])
                .collect::<Vec<_>>(),
            [10, 20, 20, 30]
        );
        assert!(super::cooked(&primitive, &colors[..2]).is_none());
    }

    #[test]
    fn slots_stay_sorted_and_replace() {
        let mut paint = VertexPaint::default();
        paint.set(PaintedPrimitive::new(2, &[[0; 4]]));
        paint.set(PaintedPrimitive::new(0, &[[0; 4]]));
        paint.set(PaintedPrimitive::new(2, &[[1; 4], [1; 4]]));
        assert_eq!(
            paint
                .primitives
                .iter()
                .map(|p| (p.primitive, p.vertices))
                .collect::<Vec<_>>(),
            [(0, 1), (2, 2)]
        );
        assert_eq!(
            paint.get(2).map(|p| p.decode().expect("valid")),
            Some(vec![[1; 4]; 2])
        );
    }
}
