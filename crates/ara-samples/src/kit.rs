//! Generated kit geometry: containers, sheds, the hangar shell, the radio mast, glass,
//! light panels and material balls, written as glTF that references the fetched CC0
//! texture sets (`../third_party/<set>/…`). UVs are in texture repeats, derived from
//! the real-world size each texture covers, so texel density matches across pieces.
//! Kits are deterministic and written by `build` into the ignored `assets/kit/`.
use crate::lock::{Asset, Lock};
use anyhow::{Context, Result, ensure};
use glam::{Mat3, Mat4, Vec2, Vec3};
use serde_json::{Value, json};
use std::path::Path;

/// A material of a kit: factors plus an optional texture set from the lock.
#[derive(Debug, Clone)]
pub struct Material {
    /// Material name in the glTF.
    pub name: String,
    /// Linear base colour factor and opacity.
    pub color: [f32; 4],
    /// Roughness factor (multiplies the ARM green channel when textured).
    pub roughness: f32,
    /// Metallic factor (multiplies the ARM blue channel when textured).
    pub metallic: f32,
    /// Emissive factor; the entity's `MeshRenderer.emissive` scales it to cd/m².
    pub emissive: [f32; 3],
    /// Lock ID of a texture set (colour, normal, ARM).
    pub textures: Option<String>,
    /// Alpha-blended (glass).
    pub blend: bool,
}

impl Material {
    /// A textured material: factors stay neutral so the scan's values come through.
    pub fn textured(name: &str, set: &str) -> Self {
        Self {
            name: name.into(),
            color: [1.0; 4],
            roughness: 1.0,
            metallic: 1.0,
            emissive: [0.0; 3],
            textures: Some(set.into()),
            blend: false,
        }
    }
    /// An untextured material.
    pub fn flat(name: &str, color: [f32; 3], roughness: f32, metallic: f32) -> Self {
        Self {
            name: name.into(),
            color: [color[0], color[1], color[2], 1.0],
            roughness,
            metallic,
            emissive: [0.0; 3],
            textures: None,
            blend: false,
        }
    }
    /// A pure emitter (light panels, bulbs); colour is the emission tint.
    pub fn emitter(name: &str, tint: [f32; 3]) -> Self {
        Self {
            emissive: tint,
            ..Self::flat(name, [0.05, 0.05, 0.05], 0.4, 0.0)
        }
    }
    /// Clear glass: low roughness, blended.
    pub fn glass(name: &str, tint: [f32; 3], opacity: f32) -> Self {
        Self {
            color: [tint[0], tint[1], tint[2], opacity],
            blend: true,
            ..Self::flat(name, tint, 0.04, 0.0)
        }
    }
    /// The same material with a colour multiplier.
    pub fn tinted(mut self, color: [f32; 3]) -> Self {
        self.color = [color[0], color[1], color[2], self.color[3]];
        self
    }
    /// The same material with other roughness/metallic factors.
    pub fn with(mut self, roughness: f32, metallic: f32) -> Self {
        self.roughness = roughness;
        self.metallic = metallic;
        self
    }
}

#[derive(Default)]
struct Mesh {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<Vec2>,
    indices: Vec<u32>,
}

impl Mesh {
    fn quad(&mut self, corners: [Vec3; 4], normal: Vec3, uvs: [Vec2; 4]) {
        let base = self.positions.len() as u32;
        for (p, uv) in corners.into_iter().zip(uvs) {
            self.positions.push(p);
            self.normals.push(normal);
            self.uvs.push(uv);
        }
        self.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// A model under construction: one merged mesh per material.
pub struct Kit {
    materials: Vec<Material>,
    meshes: Vec<Mesh>,
}

impl Kit {
    /// An empty kit with these materials (parts address them by index).
    pub fn new(materials: Vec<Material>) -> Self {
        let meshes = materials.iter().map(|_| Mesh::default()).collect();
        Self { materials, meshes }
    }

    /// A box of `size` placed by `transform` (box centred at its local origin). UVs
    /// are local metres divided by `meters` (the texture's real-world repeat size).
    pub fn cuboid(&mut self, material: usize, size: Vec3, transform: Mat4, meters: Vec2) {
        let h = size * 0.5;
        let normal = Mat3::from_mat4(transform).inverse().transpose();
        let mesh = &mut self.meshes[material];
        // (normal, u axis, v axis) per face; u × v = normal keeps CCW winding outward.
        for (n, u, v) in [
            (Vec3::X, -Vec3::Z, Vec3::Y),
            (-Vec3::X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::X, -Vec3::Z),
            (-Vec3::Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (-Vec3::Z, -Vec3::X, Vec3::Y),
        ] {
            let (eu, ev, en) = (h.dot(u.abs()), h.dot(v.abs()), h.dot(n.abs()));
            let center = n * en;
            let corners = [
                center - u * eu - v * ev,
                center + u * eu - v * ev,
                center + u * eu + v * ev,
                center - u * eu + v * ev,
            ];
            let uv = |p: Vec3| Vec2::new(p.dot(u), -p.dot(v)) / meters;
            mesh.quad(
                corners.map(|p| transform.transform_point3(p)),
                (normal * n).normalize(),
                corners.map(uv),
            );
        }
    }

    /// A vertical cylinder (axis +Y from its base) with `segments` sides and caps.
    pub fn cylinder(
        &mut self,
        material: usize,
        radius: f32,
        height: f32,
        segments: u32,
        transform: Mat4,
        meters: Vec2,
    ) {
        let normal = Mat3::from_mat4(transform).inverse().transpose();
        let mesh = &mut self.meshes[material];
        let circumference = std::f32::consts::TAU * radius;
        for i in 0..segments {
            let a0 = i as f32 / segments as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / segments as f32 * std::f32::consts::TAU;
            let dir = |a: f32| Vec3::new(a.cos(), 0.0, -a.sin());
            let (d0, d1) = (dir(a0), dir(a1));
            let u0 = i as f32 / segments as f32 * circumference / meters.x;
            let u1 = (i + 1) as f32 / segments as f32 * circumference / meters.x;
            let vh = height / meters.y;
            let corners = [
                d0 * radius,
                d1 * radius,
                d1 * radius + Vec3::Y * height,
                d0 * radius + Vec3::Y * height,
            ];
            let base = mesh.positions.len() as u32;
            for (p, (n, uv)) in corners.into_iter().zip([
                (d0, Vec2::new(u0, vh)),
                (d1, Vec2::new(u1, vh)),
                (d1, Vec2::new(u1, 0.0)),
                (d0, Vec2::new(u0, 0.0)),
            ]) {
                mesh.positions.push(transform.transform_point3(p));
                mesh.normals.push((normal * n).normalize());
                mesh.uvs.push(uv);
            }
            mesh.indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            for (y, n) in [(height, Vec3::Y), (0.0, -Vec3::Y)] {
                let base = mesh.positions.len() as u32;
                for p in [
                    Vec3::Y * y,
                    d0 * radius + Vec3::Y * y,
                    d1 * radius + Vec3::Y * y,
                ] {
                    mesh.positions.push(transform.transform_point3(p));
                    mesh.normals.push((normal * n).normalize());
                    mesh.uvs.push(Vec2::new(p.x, p.z) / meters);
                }
                if n.y > 0.0 {
                    mesh.indices.extend([base, base + 1, base + 2]);
                } else {
                    mesh.indices.extend([base, base + 2, base + 1]);
                }
            }
        }
    }

    /// A UV sphere of `radius` centred at `center`; UVs wrap `repeats` times around.
    pub fn sphere(
        &mut self,
        material: usize,
        center: Vec3,
        radius: f32,
        rings: u32,
        repeats: Vec2,
    ) {
        let mesh = &mut self.meshes[material];
        let segments = rings * 2;
        let base = mesh.positions.len() as u32;
        for r in 0..=rings {
            let theta = r as f32 / rings as f32 * std::f32::consts::PI;
            for s in 0..=segments {
                let phi = s as f32 / segments as f32 * std::f32::consts::TAU;
                // Poles are exact, so their fan triangles are exactly degenerate.
                let n = match r {
                    0 => Vec3::Y,
                    r if r == rings => Vec3::NEG_Y,
                    _ => Vec3::new(
                        theta.sin() * phi.cos(),
                        theta.cos(),
                        -theta.sin() * phi.sin(),
                    ),
                };
                mesh.positions.push(center + n * radius);
                mesh.normals.push(n);
                mesh.uvs.push(Vec2::new(
                    s as f32 / segments as f32 * repeats.x,
                    r as f32 / rings as f32 * repeats.y,
                ));
            }
        }
        let row = segments + 1;
        for r in 0..rings {
            for s in 0..segments {
                let a = base + r * row + s;
                let b = a + row;
                mesh.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            }
        }
    }

    /// A flat, irregular disc (puddles) at height `y`, facing +Y; `seed` shapes it.
    pub fn disc(&mut self, material: usize, radius: f32, seed: u32, y: f32) {
        let mesh = &mut self.meshes[material];
        let base = mesh.positions.len() as u32;
        mesh.positions.push(Vec3::new(0.0, y, 0.0));
        mesh.normals.push(Vec3::Y);
        mesh.uvs.push(Vec2::splat(0.5));
        let n = 40;
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let s = seed as f32;
            let r = radius
                * (0.75
                    + 0.18 * (a * 2.0 + s).sin()
                    + 0.09 * (a * 5.0 + s * 1.7).sin()
                    + 0.05 * (a * 9.0 + s * 0.3).cos());
            let p = Vec3::new(a.cos() * r, y, -a.sin() * r);
            mesh.positions.push(p);
            mesh.normals.push(Vec3::Y);
            mesh.uvs.push(Vec2::new(p.x, p.z) / (2.0 * radius) + 0.5);
        }
        for i in 0..n {
            mesh.indices
                .extend([base, base + 1 + i, base + 1 + (i + 1) % n]);
        }
    }

    /// A ground-hugging strip along `path` (world X/Z points), `width` wide, at
    /// `height(x, z) + lift`, UVs in texture repeats (`meters`).
    pub fn ribbon(
        &mut self,
        material: usize,
        path: &[Vec2],
        width: f32,
        lift: f32,
        meters: Vec2,
        height: &dyn Fn(f32, f32) -> f32,
    ) {
        // Resample to ~1 m steps so the strip follows the terrain.
        let mut points = Vec::new();
        for pair in path.windows(2) {
            let steps = (pair[0].distance(pair[1]) / 1.0).ceil().max(1.0) as usize;
            for i in 0..steps {
                points.push(pair[0].lerp(pair[1], i as f32 / steps as f32));
            }
        }
        points.extend(path.last());
        let across = 6;
        let mesh = &mut self.meshes[material];
        let base = mesh.positions.len() as u32;
        let mut along = 0.0;
        for (i, p) in points.iter().enumerate() {
            let prev = points[i.saturating_sub(1)];
            let next = points[(i + 1).min(points.len() - 1)];
            let dir = (next - prev).normalize_or_zero();
            let side = Vec2::new(-dir.y, dir.x);
            if i > 0 {
                along += p.distance(points[i - 1]);
            }
            // Ragged edges: width varies slowly along the path.
            let w = width * (1.0 + 0.12 * (along * 0.37).sin() + 0.06 * (along * 1.3).cos());
            for j in 0..=across {
                let t = j as f32 / across as f32 - 0.5;
                let q = *p + side * t * w;
                mesh.positions
                    .push(Vec3::new(q.x, height(q.x, q.y) + lift, q.y));
                mesh.normals.push(Vec3::Y);
                mesh.uvs.push(Vec2::new(t * w, along) / meters);
            }
        }
        let row = across as u32 + 1;
        for i in 0..points.len().saturating_sub(1) as u32 {
            for j in 0..across as u32 {
                let a = base + i * row + j;
                let b = a + row;
                mesh.indices.extend([a, a + 1, b, a + 1, b + 1, b]);
            }
        }
        // Normals from the surface.
        let start = base as usize;
        let count = mesh.positions.len() - start;
        let mut normals = vec![Vec3::ZERO; count];
        let first = self_tri_start(mesh, base);
        for t in mesh.indices[first..].as_chunks::<3>().0 {
            let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
            let n = (mesh.positions[b] - mesh.positions[a])
                .cross(mesh.positions[c] - mesh.positions[a]);
            for v in [a, b, c] {
                normals[v - start] += n;
            }
        }
        for (i, n) in normals.into_iter().enumerate() {
            mesh.normals[start + i] = n.normalize_or(Vec3::Y);
        }
    }

    /// Writes `<project>/assets/kit/<name>.gltf` (+ `.bin`), resolving texture sets
    /// against the lock. Returns the project-relative glTF path.
    pub fn write(&self, project: &Path, lock: &Lock, name: &str) -> Result<String> {
        let dir = project.join("assets/kit");
        std::fs::create_dir_all(&dir)?;
        let mut bin: Vec<u8> = Vec::new();
        let mut views = Vec::new();
        let mut accessors = Vec::new();
        let mut primitives = Vec::new();
        let mut push = |bytes: &[u8], target: u32| -> usize {
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len(), "target": target}));
            bin.extend_from_slice(bytes);
            views.len() - 1
        };
        for (material, mesh) in self.meshes.iter().enumerate() {
            if mesh.indices.is_empty() {
                continue;
            }
            let (min, max) = mesh.positions.iter().fold(
                (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                |(lo, hi), p| (lo.min(*p), hi.max(*p)),
            );
            let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
            let p = push(
                &floats(
                    &mesh
                        .positions
                        .iter()
                        .flat_map(|v| v.to_array())
                        .collect::<Vec<_>>(),
                ),
                34962,
            );
            let n = push(
                &floats(
                    &mesh
                        .normals
                        .iter()
                        .flat_map(|v| v.to_array())
                        .collect::<Vec<_>>(),
                ),
                34962,
            );
            let t = push(
                &floats(
                    &mesh
                        .uvs
                        .iter()
                        .flat_map(|v| v.to_array())
                        .collect::<Vec<_>>(),
                ),
                34962,
            );
            let i = push(
                &mesh
                    .indices
                    .iter()
                    .flat_map(|i| i.to_le_bytes())
                    .collect::<Vec<u8>>(),
                34963,
            );
            let count = mesh.positions.len();
            let first = accessors.len();
            accessors.push(json!({"bufferView": p, "componentType": 5126, "count": count, "type": "VEC3", "min": min.to_array(), "max": max.to_array()}));
            accessors.push(
                json!({"bufferView": n, "componentType": 5126, "count": count, "type": "VEC3"}),
            );
            accessors.push(
                json!({"bufferView": t, "componentType": 5126, "count": count, "type": "VEC2"}),
            );
            accessors.push(json!({"bufferView": i, "componentType": 5125, "count": mesh.indices.len(), "type": "SCALAR"}));
            primitives.push(json!({
                "attributes": {"POSITION": first, "NORMAL": first + 1, "TEXCOORD_0": first + 2},
                "indices": first + 3,
                "material": material,
            }));
        }
        ensure!(!primitives.is_empty(), "kit {name} is empty");
        let mut images: Vec<Value> = Vec::new();
        let mut image_index = |uri: String| -> usize {
            if let Some(i) = images.iter().position(|v| v["uri"] == uri.as_str()) {
                return i;
            }
            images.push(json!({"uri": uri}));
            images.len() - 1
        };
        let mut materials = Vec::new();
        for m in &self.materials {
            let mut pbr = json!({"baseColorFactor": m.color, "roughnessFactor": m.roughness, "metallicFactor": m.metallic});
            let mut out = json!({"name": m.name, "pbrMetallicRoughness": {}});
            if let Some(set) = &m.textures {
                let asset = lock
                    .get(set)
                    .with_context(|| format!("kit {name}: no texture set {set}"))?;
                let file = |key: &str| texture_file(asset, key);
                let uri = |f: &str| format!("../third_party/{}/{f}", asset.id);
                pbr["baseColorTexture"] = json!({"index": image_index(uri(&file("_diff_")?))});
                out["normalTexture"] = json!({"index": image_index(uri(&file("_nor_gl_")?))});
                if let Ok(arm) = file("_arm_") {
                    let arm = image_index(uri(&arm));
                    pbr["metallicRoughnessTexture"] = json!({"index": arm});
                    out["occlusionTexture"] = json!({"index": arm});
                } else if let Ok(rough) = file("_rough_") {
                    pbr["metallicRoughnessTexture"] = json!({"index": image_index(uri(&rough))});
                    pbr["metallicFactor"] = json!(0.0);
                }
            }
            out["pbrMetallicRoughness"] = pbr;
            if m.emissive.iter().any(|v| *v > 0.0) {
                out["emissiveFactor"] = json!(m.emissive);
            }
            if m.blend {
                out["alphaMode"] = json!("BLEND");
                out["doubleSided"] = json!(true);
            }
            materials.push(out);
        }
        // Textures are one per image (glTF needs a texture object per sampled image).
        let textures: Vec<Value> = (0..images.len()).map(|i| json!({"source": i})).collect();
        let gltf = json!({
            "asset": {"version": "2.0", "generator": "ara-samples kit"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"name": name, "mesh": 0}],
            "meshes": [{"name": name, "primitives": primitives}],
            "materials": materials,
            "textures": textures,
            "images": images,
            "buffers": [{"uri": format!("{name}.bin"), "byteLength": bin.len()}],
            "bufferViews": views,
            "accessors": accessors,
        });
        std::fs::write(dir.join(format!("{name}.bin")), &bin)?;
        std::fs::write(
            dir.join(format!("{name}.gltf")),
            ara_json::to_canonical_string(&gltf),
        )?;
        Ok(format!("assets/kit/{name}.gltf"))
    }
}

/// Index of the first triangle index referring to vertices at or after `base`.
fn self_tri_start(mesh: &Mesh, base: u32) -> usize {
    mesh.indices
        .iter()
        .position(|&i| i >= base)
        .map_or(mesh.indices.len(), |p| p - p % 3)
}

/// The file of a texture set whose name contains `key` (`_diff_`, `_nor_gl_`, `_arm_`).
fn texture_file(asset: &Asset, key: &str) -> Result<String> {
    asset
        .files
        .iter()
        .find(|f| f.path.contains(key))
        .map(|f| f.path.clone())
        .with_context(|| format!("texture set {} has no {key} map", asset.id))
}

/// The real-world repeat size of a texture set (1 m when the source gives none).
pub fn meters(lock: &Lock, set: &str) -> Vec2 {
    lock.get(set)
        .and_then(|a| a.meters)
        .map_or(Vec2::ONE, Vec2::from_array)
}

/// One source glTF node placed as a native prefab child.
pub struct Part<'a> {
    /// Source node name.
    pub node: &'a str,
    /// Translation of the selected node's pivot in the prefab.
    pub at: [f32; 3],
    /// Extra yaw in degrees, applied after the source node's rotation.
    pub yaw: f32,
}

/// Shorthand for a [`Part`].
pub fn part(node: &str, at: [f32; 3], yaw: f32) -> Part<'_> {
    Part { node, at, yaw }
}

/// A glTF's JSON and, for GLB, its binary chunk.
fn split_glb(bytes: &[u8]) -> Result<(Value, Option<Vec<u8>>)> {
    if !bytes.starts_with(b"glTF") {
        return Ok((serde_json::from_slice(bytes)?, None));
    }
    let u32_at = |o: usize| -> Result<usize> {
        let b = bytes.get(o..o + 4).context("short GLB")?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let json_len = u32_at(12)?;
    let doc = serde_json::from_slice(bytes.get(20..20 + json_len).context("short GLB")?)?;
    let at = 20 + json_len;
    let bin = if at + 8 <= bytes.len() {
        let len = u32_at(at)?;
        Some(
            bytes
                .get(at + 8..at + 8 + len)
                .context("short GLB")?
                .to_vec(),
        )
    } else {
        None
    };
    Ok((doc, bin))
}

/// Writes `assets/kit/<name>.png`: an image embedded in a fetched GLB (by image
/// name), its colours replaced by luminance x `tint`. Recolours a palette-textured
/// model without touching its UVs.
pub fn recolor(
    project: &Path,
    lock: &Lock,
    asset: &str,
    image: &str,
    tint: [f32; 3],
    name: &str,
) -> Result<String> {
    let entry = lock
        .get(asset)
        .with_context(|| format!("{asset} is not in the lock"))?;
    let file = entry
        .entry(&[".glb"])
        .with_context(|| format!("{asset} has no GLB"))?;
    let (doc, bin) = split_glb(&std::fs::read(entry.dir(project).join(&file.path))?)?;
    let bin = bin.context("GLB without a binary chunk")?;
    let found = doc["images"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["name"].as_str().is_some_and(|n| n.contains(image)))
        .with_context(|| format!("{asset} has no image {image}"))?;
    let view = &doc["bufferViews"][found["bufferView"]
        .as_u64()
        .context("image is not embedded")? as usize];
    let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let length = view["byteLength"].as_u64().context("view length")? as usize;
    let mut rgba = image::load_from_memory(&bin[offset..offset + length])?.to_rgba8();
    for p in rgba.pixels_mut() {
        let luma = (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
            / 255.0;
        // Compress the palette's range so suits stay mid-grey, joints a little lighter.
        let v = 0.35 + 0.65 * luma;
        for c in 0..3 {
            p[c] = (v * tint[c] * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
    let out = format!("{name}.png");
    let dir = project.join("assets/kit");
    std::fs::create_dir_all(&dir)?;
    rgba.save(dir.join(&out))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every triangle's winding agrees with its vertices' stored normals.
    fn wound_outward(mesh: &Mesh) -> bool {
        mesh.indices.as_chunks::<3>().0.iter().all(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.positions[i as usize]);
            let face = (b - a).cross(c - a);
            face.length() < 1e-9 || face.dot(mesh.normals[t[0] as usize]) > 0.0
        })
    }

    #[test]
    fn primitives_wind_counter_clockwise_outward() {
        let m = || Kit::new(vec![Material::flat("m", [0.5; 3], 0.5, 0.0)]);
        let mut cuboid = m();
        cuboid.cuboid(
            0,
            Vec3::new(2.0, 1.0, 3.0),
            Mat4::from_rotation_y(0.7),
            Vec2::ONE,
        );
        assert!(wound_outward(&cuboid.meshes[0]), "cuboid");
        let mut cylinder = m();
        cylinder.cylinder(0, 0.5, 2.0, 12, Mat4::IDENTITY, Vec2::ONE);
        assert!(wound_outward(&cylinder.meshes[0]), "cylinder");
        let mut sphere = m();
        sphere.sphere(0, Vec3::ZERO, 1.0, 8, Vec2::ONE);
        assert!(wound_outward(&sphere.meshes[0]), "sphere");
        let mut disc = m();
        disc.disc(0, 1.0, 3, 0.0);
        assert!(wound_outward(&disc.meshes[0]), "disc");
        let mut ribbon = m();
        ribbon.ribbon(
            0,
            &[Vec2::ZERO, Vec2::new(0.0, 10.0), Vec2::new(5.0, 20.0)],
            4.0,
            0.0,
            Vec2::ONE,
            &|x, _| x * 0.1,
        );
        assert!(wound_outward(&ribbon.meshes[0]), "ribbon");
    }
}
