//! Placement helpers: `entity.create` calls for props, colliders and lights, model
//! bounds read from glTF, and terrain heights from the authored terrain document.
use super::Ctx;
use anyhow::{Context, Result, ensure};
use glam::{Mat4, Quat, Vec3};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// A model registered in the project: its `@path#id` reference and rest bounds.
#[derive(Debug, Clone)]
pub struct Model {
    /// `@assets/…#as_…`.
    pub reference: String,
    /// Model-space AABB minimum (metres).
    pub min: Vec3,
    /// Model-space AABB maximum (metres).
    pub max: Vec3,
}

/// The models of a project by short name.
#[derive(Default)]
pub struct Models(pub BTreeMap<String, Model>);

impl Models {
    /// The model registered as `name`.
    pub fn get(&self, name: &str) -> &Model {
        self.0
            .get(name)
            .unwrap_or_else(|| panic!("model {name} was not registered"))
    }
}

impl Ctx {
    /// Registers a model file: writes its glTF sidecar and records its bounds.
    pub fn model(&self, models: &mut Models, name: &str, path: &str, id: &str) -> Result<()> {
        self.sidecar(path, id, Value::Null)?;
        let (min, max) =
            gltf_bounds(&self.root.join(path)).with_context(|| format!("bounds of {path}"))?;
        models.0.insert(
            name.to_owned(),
            Model {
                reference: format!("@{path}#{id}"),
                min,
                max,
            },
        );
        Ok(())
    }

    /// Registers a fetched third-party model by lock ID (its first glTF/GLB).
    pub fn third_party(&self, models: &mut Models, name: &str, asset: &str) -> Result<()> {
        let entry = self
            .lock
            .get(asset)
            .with_context(|| format!("{asset} is not in the lock"))?;
        let file = entry
            .entry(&[".gltf", ".glb"])
            .with_context(|| format!("{asset} has no glTF"))?;
        let id = format!("as_{}", short(asset));
        self.model(models, name, &entry.relative(&file.path), &id)
    }
}

/// A short asset-ID body from a lock ID (`namaqualand_boulder_02` →
/// `namaqualandboulder02`); IDs allow at most 24 lowercase alphanumerics.
pub fn short(id: &str) -> String {
    let mut body: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    body.truncate(24);
    body
}

/// Model-space bounds of a glTF/GLB's default scene, from accessor min/max.
pub fn gltf_bounds(path: &Path) -> Result<(Vec3, Vec3)> {
    let bytes = std::fs::read(path)?;
    let doc: Value = if bytes.starts_with(b"glTF") {
        ensure!(bytes.len() > 20, "short GLB");
        let len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
        serde_json::from_slice(&bytes[20..20 + len])?
    } else {
        serde_json::from_slice(&bytes)?
    };
    let nodes = doc["nodes"].as_array().cloned().unwrap_or_default();
    let local = |n: &Value| -> Mat4 {
        if let Some(m) = n["matrix"].as_array().filter(|m| m.len() == 16) {
            let cols: Vec<f32> = m.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect();
            return Mat4::from_cols_slice(&cols);
        }
        let f = |v: &Value, d: &[f32]| -> Vec<f32> {
            (0..d.len())
                .map(|i| v[i].as_f64().map_or(d[i], |x| x as f32))
                .collect()
        };
        let t = f(&n["translation"], &[0.0; 3]);
        let r = f(&n["rotation"], &[0.0, 0.0, 0.0, 1.0]);
        let s = f(&n["scale"], &[1.0; 3]);
        Mat4::from_scale_rotation_translation(
            Vec3::new(s[0], s[1], s[2]),
            Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize(),
            Vec3::new(t[0], t[1], t[2]),
        )
    };
    let scene = doc["scene"].as_u64().unwrap_or(0) as usize;
    let mut stack: Vec<(usize, Mat4)> = doc["scenes"][scene]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .map(|n| (n as usize, Mat4::IDENTITY))
        .collect();
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    while let Some((index, parent)) = stack.pop() {
        let node = &nodes[index];
        let world = parent * local(node);
        for child in node["children"].as_array().into_iter().flatten() {
            if let Some(c) = child.as_u64() {
                stack.push((c as usize, world));
            }
        }
        let Some(mesh) = node["mesh"].as_u64() else {
            continue;
        };
        for primitive in doc["meshes"][mesh as usize]["primitives"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let accessor = &doc["accessors"][primitive["attributes"]["POSITION"]
                .as_u64()
                .context("primitive without positions")?
                as usize];
            let corner = |v: &Value| {
                Vec3::new(
                    v[0].as_f64().unwrap_or(0.0) as f32,
                    v[1].as_f64().unwrap_or(0.0) as f32,
                    v[2].as_f64().unwrap_or(0.0) as f32,
                )
            };
            let (lo, hi) = (corner(&accessor["min"]), corner(&accessor["max"]));
            for i in 0..8 {
                let p = Vec3::new(
                    if i & 1 == 0 { lo.x } else { hi.x },
                    if i & 2 == 0 { lo.y } else { hi.y },
                    if i & 4 == 0 { lo.z } else { hi.z },
                );
                let w = world.transform_point3(p);
                min = min.min(w);
                max = max.max(w);
            }
        }
    }
    ensure!(min.x <= max.x, "no geometry");
    Ok((min, max))
}

/// A placed prop: the visual entity plus an optional static box collider entity
/// (colliders have no offset, so the box is its own entity at the bounds centre).
#[derive(Debug, Clone)]
pub struct Prop {
    /// Record ID (`e_…`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Model short name.
    pub model: String,
    /// Where the prop stands: its footprint centre on the ground (`grounded`), or the
    /// model origin.
    pub position: Vec3,
    /// Anchor at the bounds' footprint centre and bottom instead of the model origin
    /// (sources differ in where their origin sits).
    pub grounded: bool,
    /// Yaw in degrees.
    pub yaw: f32,
    /// Uniform scale.
    pub scale: f32,
    /// Static collision.
    pub solid: bool,
    /// Extra components merged into the visual entity.
    pub extra: Value,
    /// Optional `MeshRenderer` overrides (`color`, `emissive`).
    pub renderer: Value,
}

impl Prop {
    /// A static, solid prop.
    pub fn new(id: &str, name: &str, model: &str, position: Vec3, yaw: f32) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            model: model.into(),
            position,
            grounded: true,
            yaw,
            scale: 1.0,
            solid: true,
            extra: json!({}),
            renderer: json!({}),
        }
    }
    /// Places the model origin at `position` (hanging lamps, wall fixtures).
    pub fn at_origin(mut self) -> Self {
        self.grounded = false;
        self
    }
    /// Decoration without collision.
    pub fn decor(mut self) -> Self {
        self.solid = false;
        self
    }
    /// A uniform scale.
    pub fn scaled(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }
    /// Extra `MeshRenderer` fields.
    pub fn renderer(mut self, fields: Value) -> Self {
        self.renderer = fields;
        self
    }

    /// The `entity.create` calls for this prop; colliders take the next physics ID.
    pub fn commands(&self, models: &Models, physics: &mut u64) -> Vec<(&'static str, Value)> {
        let model = models.get(&self.model);
        let mut renderer = json!({"model": model.reference});
        merge(&mut renderer, self.renderer.clone());
        let yaw = Quat::from_rotation_y(self.yaw.to_radians());
        let origin = if self.grounded {
            let center = (model.min + model.max) * 0.5 * self.scale;
            self.position
                - yaw * Vec3::new(center.x, 0.0, center.z)
                - Vec3::Y * model.min.y * self.scale
        } else {
            self.position
        };
        let mut components = json!({
            "Transform": transform(origin, self.yaw, self.scale),
            "MeshRenderer": renderer,
        });
        merge(&mut components, self.extra.clone());
        let mut out = vec![create(&self.id, &self.name, components)];
        if self.solid {
            let center = (model.min + model.max) * 0.5 * self.scale;
            let half = ((model.max - model.min) * 0.5 * self.scale).max(Vec3::splat(0.05));
            out.push(create(
                &format!("{}c", self.id),
                &format!("{} collider", self.name),
                json!({
                    "Transform": transform(origin + yaw * center, self.yaw, 1.0),
                    "RigidBody": {},
                    "PhysicsId": {"value": next(physics)},
                    "Collider": {"halfExtents": half.to_array()},
                }),
            ));
        }
        out
    }
}

/// The next unique physics ID (`PhysicsId` must be unique within a world).
pub fn next(counter: &mut u64) -> u64 {
    *counter += 1;
    *counter
}

/// Accumulates one scene's `entity.create` calls.
pub struct Scene<'a> {
    /// Registered models.
    pub models: &'a Models,
    /// Calls in creation order.
    pub commands: Vec<(&'static str, Value)>,
    /// Last physics ID handed out.
    pub physics: u64,
}

impl<'a> Scene<'a> {
    /// An empty scene over `models`.
    pub fn new(models: &'a Models) -> Self {
        Self {
            models,
            commands: Vec::new(),
            physics: 0,
        }
    }
    /// Adds a prop (and its collider).
    pub fn prop(&mut self, prop: Prop) {
        let calls = prop.commands(self.models, &mut self.physics);
        self.commands.extend(calls);
    }
    /// Adds an entity.
    pub fn entity(&mut self, id: &str, name: &str, components: Value) {
        self.commands.push(create(id, name, components));
    }
    /// The next physics ID.
    pub fn physics_id(&mut self) -> u64 {
        next(&mut self.physics)
    }

    /// Moves every created entity by `by` (layout → world): `Transform.position` and
    /// world-space `NavMeshSurface` bounds. Terrain entities stay put (their grid
    /// places them).
    pub fn shift(&mut self, by: Vec3) {
        let add = |v: &mut Value| {
            if let Some(a) = v.as_array_mut().filter(|a| a.len() == 3) {
                for (c, d) in a.iter_mut().zip(by.to_array()) {
                    *c = json!(c.as_f64().unwrap_or(0.0) + f64::from(d));
                }
            }
        };
        for (cmd, args) in &mut self.commands {
            if *cmd != "entity.create" {
                continue;
            }
            let components = &mut args["components"];
            if components.get("Terrain").is_some() {
                continue;
            }
            if let Some(t) = components.get_mut("Transform") {
                if t.get("position").is_none() {
                    t["position"] = json!([0.0, 0.0, 0.0]);
                }
                add(&mut t["position"]);
            }
            if let Some(nav) = components.get_mut("NavMeshSurface") {
                add(&mut nav["min"]);
                add(&mut nav["max"]);
            }
        }
    }
}

/// An `entity.create` call with a concrete record ID.
pub fn create(id: &str, name: &str, components: Value) -> (&'static str, Value) {
    (
        "entity.create",
        json!({"id": id, "name": name, "components": components}),
    )
}

/// A `Transform` with yaw (degrees) and uniform scale; defaults are omitted.
pub fn transform(position: Vec3, yaw: f32, scale: f32) -> Value {
    let mut t = json!({"position": round3(position)});
    if yaw.abs() > 1e-4 {
        t["rotation"] = json!([0, round(yaw), 0]);
    }
    if (scale - 1.0).abs() > 1e-4 {
        t["scale"] = json!([round(scale), round(scale), round(scale)]);
    }
    t
}

/// A `Transform` with a full XYZ Euler rotation (degrees).
pub fn transform_euler(position: Vec3, rotation: Vec3) -> Value {
    json!({"position": round3(position), "rotation": round3(rotation)})
}

/// Millimetre rounding keeps documents readable and stable.
pub fn round(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

/// [`round`] per component.
pub fn round3(v: Vec3) -> [f64; 3] {
    [round(v.x), round(v.y), round(v.z)]
}

/// Shallow-merges object `b` into `a`.
pub fn merge(a: &mut Value, b: Value) {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        for (k, v) in b {
            a.insert(k, v);
        }
    }
}

/// Evaluated terrain heights for placing things on the ground, sampled in layout
/// coordinates (world = layout + `shift`).
pub struct Ground {
    data: ara_runtime::terrain::TerrainData,
    shift: Vec3,
}

impl Ground {
    /// Evaluates every tile of the project's terrain document at `path`.
    pub fn load(ctx: &Ctx, path: &str) -> Result<Self> {
        let schema = ara_runtime::schema(&[]).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let text = std::fs::read_to_string(ctx.root.join(path))?;
        let read = ara_doc::read(&text, path, &schema, None);
        let doc = read.doc.context("terrain document did not parse")?;
        let terrain =
            ara_terrain::Terrain::from_document(&doc).map_err(|d| anyhow::anyhow!("{d}"))?;
        let mut tiles = BTreeMap::new();
        for z in terrain.header.min.z..terrain.header.max.z {
            for x in terrain.header.min.x..terrain.header.max.x {
                let coord = ara_terrain::TileCoord { x, z };
                tiles.insert(
                    coord,
                    terrain
                        .evaluate(coord, &NoBases)
                        .map_err(|d| anyhow::anyhow!("{d}"))?,
                );
            }
        }
        Ok(Self {
            data: ara_runtime::terrain::TerrainData {
                header: terrain.header.clone(),
                tiles,
                foliage: Vec::new(),
            },
            shift: Vec3::ZERO,
        })
    }

    /// Samples layout coordinates `shift` away from the terrain's world coordinates.
    pub fn shifted(mut self, shift: Vec3) -> Self {
        self.shift = shift;
        self
    }

    /// Ground height at layout X/Z (0 outside the terrain).
    pub fn height(&self, x: f32, z: f32) -> f32 {
        self.data
            .height(x + self.shift.x, z + self.shift.z)
            .map_or(0.0, |h| h - self.shift.y)
    }

    /// A point on the ground.
    pub fn at(&self, x: f32, z: f32) -> Vec3 {
        Vec3::new(x, self.height(x, z), z)
    }

    /// The lowest ground height under a footprint (so props never float).
    pub fn lowest(&self, x: f32, z: f32, radius: f32) -> f32 {
        let mut h = self.height(x, z);
        for (dx, dz) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            h = h.min(self.height(x + dx * radius, z + dz * radius));
        }
        h
    }
}

/// Terrains here are sculpted by strokes only.
struct NoBases;
impl ara_terrain::cache::TileSource for NoBases {
    fn load(&self, hash: &str) -> std::result::Result<ara_terrain::Tile, ara_diag::Diagnostic> {
        Err(ara_terrain::invalid(format!("no base {hash}")))
    }
}
