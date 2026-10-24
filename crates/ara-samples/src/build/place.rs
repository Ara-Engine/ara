//! Placement helpers: `entity.create` calls for props, colliders and lights, model
//! bounds read from glTF, and terrain heights from the authored terrain document.
use super::Ctx;
use anyhow::{Context, Result};
use glam::{Quat, Vec3};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// A model registered in the project: its `@path#id` reference and rest bounds.
#[derive(Debug, Clone)]
pub struct Model {
    /// Source model or prefab asset reference.
    pub reference: String,
    /// Native prefab instance instead of a direct model renderer.
    pub prefab: bool,
    /// Renderer defaults for a direct source model.
    pub renderer: Value,
    /// Renderer child IDs in a native prefab, for per-prop overrides.
    pub parts: Vec<String>,
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
        let (min, max) = super::bounds::file(&self.root.join(path), None)
            .with_context(|| format!("bounds of {path}"))?;
        models.0.insert(
            name.to_owned(),
            Model {
                reference: format!("@{path}#{id}"),
                prefab: false,
                renderer: json!({"model":format!("@{path}#{id}")}),
                parts: Vec::new(),
                min,
                max,
            },
        );
        Ok(())
    }

    /// Registers a fetched third-party model by lock ID (its first glTF/GLB).
    pub fn third_party(&self, models: &mut Models, name: &str, asset: &str) -> Result<()> {
        super::native::whole(self, models, name, asset, &[])
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

/// A direct model or native prefab, with an optional centered collider on its root.
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

    /// Native authoring calls for the prop and its centered collision.
    pub fn commands(&self, models: &Models) -> Vec<(&'static str, Value)> {
        let model = models.get(&self.model);
        let mut renderer = model.renderer.clone();
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
        let mut components = json!({"Transform":transform(origin,self.yaw,self.scale)});
        if !model.prefab {
            components["MeshRenderer"] = renderer;
        }
        if self.solid {
            let center = (model.min + model.max) * 0.5 * self.scale;
            let half = ((model.max - model.min) * 0.5 * self.scale).max(Vec3::splat(0.05));
            components["RigidBody"] = json!({});
            components["Collider"] =
                json!({"center":center.to_array(),"halfExtents":half.to_array()});
        }
        merge(&mut components, self.extra.clone());
        let mut out = Vec::new();
        if model.prefab {
            out.push(("prefab.instantiate",json!({"id":self.id,"name":self.name,"prefab":model.reference,"position":origin.to_array()})));
            for (component, value) in components.as_object().into_iter().flatten() {
                if component == "Transform" {
                    out.push((
                        "property.set",
                        json!({"entity":self.id,"path":"Transform","value":value}),
                    ));
                } else {
                    out.push((
                        "component.add",
                        json!({"entity":self.id,"component":component,"value":value}),
                    ));
                }
            }
            for child in &model.parts {
                for (field, value) in self.renderer.as_object().into_iter().flatten() {
                    out.push(("property.set",json!({"entity":format!("{}/{child}",self.id),"path":format!("MeshRenderer.{field}"),"value":value})));
                }
            }
        } else {
            out.push(create(&self.id, &self.name, components));
        }

        out
    }
}

/// Accumulates one scene's `entity.create` calls.
pub struct Scene<'a> {
    /// Registered models.
    pub models: &'a Models,
    /// Calls in creation order.
    pub commands: Vec<(&'static str, Value)>,
}

impl<'a> Scene<'a> {
    /// An empty scene over `models`.
    pub fn new(models: &'a Models) -> Self {
        Self {
            models,
            commands: Vec::new(),
        }
    }
    /// Adds a prop (and its collider).
    pub fn prop(&mut self, prop: Prop) {
        let calls = prop.commands(self.models);
        self.commands.extend(calls);
    }
    /// Adds an entity.
    pub fn entity(&mut self, id: &str, name: &str, components: Value) {
        self.commands.push(create(id, name, components));
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

/// Evaluated terrain heights in authored world coordinates.
pub struct Ground {
    data: ara_runtime::terrain::TerrainData,
}

impl Ground {
    /// Evaluates every tile of the project's terrain document at `path`.
    pub fn load(root: &Path, path: &str) -> Result<Self> {
        let schema = ara_runtime::schema(&[]).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let text = std::fs::read_to_string(root.join(path))?;
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
                foliage_rules: Vec::new(),
            },
        })
    }

    /// Ground height at layout X/Z (0 outside the terrain).
    pub fn height(&self, x: f32, z: f32) -> f32 {
        self.data.height(x, z).unwrap_or(0.0)
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
