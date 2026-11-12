//! A small glTF 2.0 writer for the Unity comparison export (`unity_export`): one `.bin`
//! buffer, meshes, materials, textures, nodes and `KHR_lights_punctual` lights. Measurement
//! tooling only; nothing ships it.
use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

/// glTF component types used here.
const F32: u32 = 5126;
const U8: u32 = 5121;
const U32: u32 = 5125;

/// Accumulates a glTF document and its single binary buffer.
#[derive(Default)]
pub(crate) struct Writer {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    meshes: Vec<Value>,
    materials: Vec<Value>,
    images: Vec<Value>,
    textures: Vec<Value>,
    nodes: Vec<Value>,
    lights: Vec<Value>,
    roots: Vec<usize>,
    extensions: std::collections::BTreeSet<&'static str>,
}

impl Writer {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let mut view = json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(view);
        self.views.len() - 1
    }

    /// A float accessor of `width` components per element (3: positions and normals,
    /// 2: uvs, 4: tangents); positions carry the min and max glTF requires.
    pub fn floats(&mut self, data: &[f32], width: usize, bounds: bool) -> usize {
        let view = self.view(&bytemuck_f32(data), Some(34962));
        let count = data.len() / width;
        let ty = ["", "SCALAR", "VEC2", "VEC3", "VEC4"][width];
        let mut accessor = json!({
            "bufferView": view, "componentType": F32, "count": count,
            "type": ty,
        });
        if bounds && count > 0 {
            let mut min = vec![f32::MAX; width];
            let mut max = vec![f32::MIN; width];
            for chunk in data.chunks_exact(width) {
                for (i, v) in chunk.iter().enumerate() {
                    min[i] = min[i].min(*v);
                    max[i] = max[i].max(*v);
                }
            }
            accessor["min"] = json!(min);
            accessor["max"] = json!(max);
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    /// Normalised RGBA8 vertex colours.
    pub fn colors(&mut self, rgba: &[u8]) -> usize {
        let view = self.view(rgba, Some(34962));
        self.accessors.push(json!({
            "bufferView": view, "componentType": U8, "normalized": true,
            "count": rgba.len() / 4, "type": "VEC4",
        }));
        self.accessors.len() - 1
    }

    /// 32-bit triangle indices.
    pub fn indices(&mut self, data: &[u32]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34963));
        self.accessors.push(json!({
            "bufferView": view, "componentType": U32, "count": data.len(), "type": "SCALAR",
        }));
        self.accessors.len() - 1
    }

    /// Adds a mesh of one primitive, returning its index.
    pub fn mesh(&mut self, name: &str, attributes: Value, indices: usize, material: usize) -> usize {
        self.meshes.push(json!({"name": name, "primitives": [{
            "attributes": attributes, "indices": indices, "material": material,
        }]}));
        self.meshes.len() - 1
    }

    /// Adds a material from its JSON (the caller names the extensions it used).
    pub fn material(&mut self, material: Value, extensions: &[&'static str]) -> usize {
        self.extensions.extend(extensions);
        self.materials.push(material);
        self.materials.len() - 1
    }

    /// Adds an image file (relative to the glTF) and a texture over it.
    pub fn texture(&mut self, uri: &str, clamp: bool) -> usize {
        self.images.push(json!({"uri": uri}));
        self.textures.push(json!({"source": self.images.len() - 1, "sampler": usize::from(clamp)}));
        self.textures.len() - 1
    }

    /// Adds a punctual light, returning its index.
    pub fn light(&mut self, light: Value) -> usize {
        self.extensions.insert("KHR_lights_punctual");
        self.lights.push(light);
        self.lights.len() - 1
    }

    /// Adds a node under the scene root.
    pub fn node(&mut self, node: Value) -> usize {
        self.nodes.push(node);
        self.roots.push(self.nodes.len() - 1);
        self.nodes.len() - 1
    }

    /// Writes `<stem>.gltf` and `<stem>.bin`.
    pub fn write(mut self, dir: &Path, stem: &str) -> Result<()> {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        std::fs::write(dir.join(format!("{stem}.bin")), &self.bin)?;
        let mut doc = json!({
            "asset": {"version": "2.0", "generator": "ara-renderlab unity_export"},
            "scene": 0,
            "scenes": [{"nodes": self.roots}],
            "nodes": self.nodes,
            "meshes": self.meshes,
            "materials": self.materials,
            "accessors": self.accessors,
            "bufferViews": self.views,
            "buffers": [{"uri": format!("{stem}.bin"), "byteLength": self.bin.len()}],
        });
        if !self.textures.is_empty() {
            doc["textures"] = json!(self.textures);
            doc["images"] = json!(self.images);
            doc["samplers"] = json!([{"magFilter": 9729, "minFilter": 9987}, {"magFilter": 9729, "minFilter": 9987, "wrapS": 33071, "wrapT": 33071}]);
        }
        if !self.extensions.is_empty() {
            doc["extensionsUsed"] = json!(self.extensions);
        }
        if !self.lights.is_empty() {
            doc["extensions"] = json!({"KHR_lights_punctual": {"lights": self.lights}});
        }
        std::fs::write(dir.join(format!("{stem}.gltf")), serde_json::to_vec(&doc)?)?;
        Ok(())
    }
}

/// The bytes of a float slice, little-endian.
fn bytemuck_f32(data: &[f32]) -> Vec<u8> {
    data.iter().flat_map(|f| f.to_le_bytes()).collect()
}
