//! Temporary document-to-render extraction, isolated for ara-sim's edit-world projection.
pub(crate) mod bounds;
pub(crate) mod channels;
pub mod gesture;
mod ghosts;
pub(crate) mod gltf;
mod imports;
mod patch;
mod resolver;
use crate::state::EditorState;
use ara_diag::Diagnostic;
use ara_doc::prefab::Resolved;
use ara_ops::Store;
use ara_render::scene::{Light, Material, Mesh, Scene, Texture, Vertex};
use ara_runtime::components::MeshRenderer;
use glam::{EulerRot, Mat4, Quat, Vec3};
pub use gltf::ModelCache;
pub use imports::{Imports, ImportsCache};
pub(crate) use patch::Patch;
pub use patch::scene_only;
pub use resolver::SceneResolver;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

/// Read-only extraction boundary; ara-sim can replace the document adapter without changing panels.
pub trait EditWorldView {
    /// Current scene entities, with prefab inheritance resolved.
    fn entities(&self) -> &Resolved;
    /// Extract geometry using the project-owned published model cache across edits.
    fn scene(
        &self,
        root: &Path,
        state: &EditorState,
        preview: Option<&gesture::Gesture>,
        models: &mut ModelCache,
    ) -> anyhow::Result<Scene>;
}
/// Document-backed edit-world view used until ara-sim is integrated.
pub struct DocumentView {
    /// Shared with the Shell's resolve of the same revision (`Shell::document_view`).
    entities: Arc<Resolved>,
    ghosts: BTreeMap<String, ara_doc::Record>,
    /// Shared with other views of the same settings (`ImportsCache`).
    imports: Arc<Imports>,
}
impl DocumentView {
    /// Resolve a scene once per committed document revision.
    pub fn new(store: &Store, doc: &str) -> Result<Self, Vec<Diagnostic>> {
        Ok(Self::with_entities(store, Arc::new(resolve(store, doc)?)))
    }
    /// The view of `entities`, the open scene already resolved at the store's current
    /// revision (no second resolve), with the import settings parsed.
    pub fn with_entities(store: &Store, entities: Arc<Resolved>) -> Self {
        Self::with_imports(entities, Arc::new(imports::parse(store)))
    }
    /// The view of `entities` and the import settings at the same revision, both
    /// shared (`Shell::document_view` keeps an [`ImportsCache`]).
    pub fn with_imports(entities: Arc<Resolved>, imports: Arc<Imports>) -> Self {
        Self {
            entities,
            ghosts: BTreeMap::new(),
            imports,
        }
    }
    /// Model sources the scene references, other than the `known` paths, each checked
    /// on disk once: `Some(entity)` (one that uses it; it draws a placeholder) when
    /// the file is missing. In path order.
    pub(crate) fn missing_sources(
        &self,
        root: &Path,
        known: &BTreeSet<String>,
    ) -> Vec<(String, Option<String>)> {
        // One disk check per path, not per entity: scenes reuse a few models widely.
        let mut checked: BTreeMap<String, Option<String>> = BTreeMap::new();
        for (id, entity) in self.entities.iter() {
            let Some(reference) = entity
                .record
                .get("MeshRenderer")
                .and_then(|r| r.get("model"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(path) = ara_schema::grammar::parse_ref(reference)
                .ok()
                .and_then(|r| r.path.map(str::to_owned))
            else {
                continue;
            };
            if !known.contains(&path) && !checked.contains_key(&path) {
                let missing = !root.join(&path).is_file();
                checked.insert(path, missing.then(|| id.clone()));
            }
        }
        checked.into_iter().collect()
    }
    /// Deleted-entity ghosts. Empty since D24 (agent deletions apply like any other
    /// edit); kept as the render seam for future soft-delete previews.
    pub(crate) fn ghosts(&self) -> &BTreeMap<String, ara_doc::Record> {
        &self.ghosts
    }
}
/// Resolve one loaded scene and its prefab graph, preserving diagnostics.
pub fn resolve(store: &Store, doc: &str) -> Result<Resolved, Vec<Diagnostic>> {
    let document = store
        .workspace()
        .find(doc)
        .and_then(|id| store.workspace().doc(id.as_str()))
        .ok_or_else(|| vec![crate::invalid(format!("document `{doc}` not found"))])?;
    let mut diags = Vec::new();
    let entities = ara_doc::prefab::resolve(
        document,
        store.schema(),
        store.workspace(),
        &Default::default(),
        &mut diags,
    );
    if ara_diag::has_errors(&diags) {
        Err(diags)
    } else {
        Ok(entities)
    }
}
/// Effective local transform, materializing the core schema defaults.
pub fn local_transform(record: &ara_doc::Record) -> Mat4 {
    let t = record.get("Transform").unwrap_or(&Value::Null);
    let position = vector(t.get("position"), Vec3::ZERO);
    let rotation = vector(t.get("rotation"), Vec3::ZERO).map(f32::to_radians);
    let scale = vector(t.get("scale"), Vec3::ONE);
    Mat4::from_scale_rotation_translation(
        scale,
        Quat::from_euler(EulerRot::XYZ, rotation.x, rotation.y, rotation.z),
        position,
    )
}
/// Parent-ordered transforms, computed without recursive calls even for deep hierarchies.
pub fn world_transforms(entities: &Resolved) -> BTreeMap<String, Mat4> {
    transforms_with(entities, None)
}
/// One entity's world transform: its local transform composed up its parent chain
/// (a cycle stops at the repeat, as in [`world_transforms`]). For the few selected
/// entities a frame needs, without composing the whole scene.
pub fn world_transform(entities: &Resolved, id: &str) -> Option<Mat4> {
    let entity = entities.get(id)?;
    let mut matrix = local_transform(&entity.record);
    let mut seen = BTreeSet::from([id]);
    let mut current = entity.parent.as_deref();
    while let Some(addr) = current {
        let Some(parent) = entities.get(addr).filter(|_| seen.insert(addr)) else {
            break;
        };
        matrix = local_transform(&parent.record) * matrix;
        current = parent.parent.as_deref();
    }
    Some(matrix)
}
pub(super) fn transforms_with(
    entities: &Resolved,
    overrides: Option<&BTreeMap<String, Mat4>>,
) -> BTreeMap<String, Mat4> {
    let mut out = BTreeMap::new();
    for id in entities.keys() {
        if out.contains_key(id) {
            continue;
        }
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut current = Some(id.as_str());
        while let Some(addr) = current {
            if out.contains_key(addr) || !seen.insert(addr) {
                break;
            }
            let Some(entity) = entities.get(addr) else {
                break;
            };
            chain.push(addr);
            current = entity.parent.as_deref();
        }
        while let Some(addr) = chain.pop() {
            let entity = &entities[addr];
            let parent = entity
                .parent
                .as_ref()
                .and_then(|p| out.get(p))
                .copied()
                .unwrap_or(Mat4::IDENTITY);
            let local = overrides
                .and_then(|m| m.get(addr))
                .copied()
                .unwrap_or_else(|| local_transform(&entity.record));
            out.insert(addr.to_owned(), parent * local);
        }
    }
    out
}
/// Stable entity identity for the renderer's compact picking channel.
pub fn render_id(address: &str) -> u64 {
    ara_ui::source::stable_id(address)
}
pub(crate) fn vector(value: Option<&Value>, default: Vec3) -> Vec3 {
    value
        .and_then(Value::as_array)
        // RGBA colors read their first three channels.
        .filter(|a| a.len() == 3 || a.len() == 4)
        .and_then(|a| {
            Some(Vec3::new(
                a[0].as_f64()? as f32,
                a[1].as_f64()? as f32,
                a[2].as_f64()? as f32,
            ))
        })
        .unwrap_or(default)
}
impl EditWorldView for DocumentView {
    fn entities(&self) -> &Resolved {
        &self.entities
    }
    fn scene(
        &self,
        root: &Path,
        state: &EditorState,
        preview: Option<&gesture::Gesture>,
        models: &mut ModelCache,
    ) -> anyhow::Result<Scene> {
        self.extract(root, state, preview, models, None)
    }
}
impl DocumentView {
    /// Extract render geometry. With `only`, meshes come just from those entities (a
    /// [`Patch`] re-uploads them alone) while lights still cover the whole scene;
    /// ghosts and the empty-scene fallback are left out. Every record field read here
    /// must also be classified by `patch.rs`.
    pub(crate) fn extract(
        &self,
        root: &Path,
        state: &EditorState,
        preview: Option<&gesture::Gesture>,
        models: &mut ModelCache,
        only: Option<&BTreeSet<String>>,
    ) -> anyhow::Result<Scene> {
        let mut scene = Scene {
            meshes: Vec::new(),
            materials: vec![Material::default()],
            textures: vec![
                Texture {
                    size: [1, 1],
                    rgba: vec![255; 4],
                    srgb: false,
                },
                Texture {
                    size: [1, 1],
                    rgba: vec![128, 128, 255, 255],
                    srgb: false,
                },
            ],
            lights: Vec::new(),
            sun_direction: Vec3::new(-0.6, 1., 0.4).normalize(),
            sun_lux: Vec3::splat(3500.),
            environment: vec![[160., 180., 220., 1.]; 8],
            environment_size: [4, 2],
        };
        let mut transforms = world_transforms(&self.entities);
        if let Some(gesture) = preview {
            gesture.preview_world(&self.entities, &mut transforms);
        }
        for (id, entity) in self.entities.iter() {
            if state.hidden.contains(id) {
                continue;
            }
            let matrix = transforms.get(id).copied().unwrap_or(Mat4::IDENTITY);
            let name = entity
                .record
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(id);
            if let Some(record) = entity.record.get("MeshRenderer")
                && only.is_none_or(|only| only.contains(id))
            {
                // Decoded as the engine component, defaults and all: the viewport
                // draws what Play draws (same material and procedural shapes).
                let renderer: MeshRenderer =
                    serde_json::from_value(record.clone()).unwrap_or_default();
                let material = scene.materials.len();
                scene.materials.push(raster_material(&renderer));
                let mut mesh = if let Some(reference) = &renderer.model {
                    let reference =
                        ara_schema::grammar::parse_ref(&reference.0).map_err(anyhow::Error::msg)?;
                    let relative = reference
                        .path
                        .ok_or_else(|| anyhow::anyhow!("mesh reference needs a path"))?;
                    let path = crate::commands::project_path(root, relative)?;
                    if path
                        .extension()
                        .is_some_and(|s| s.eq_ignore_ascii_case("fbx"))
                    {
                        let meshes = self.imports.get(relative).and_then(|settings| {
                            crate::asset_tools::cooked_mesh::cached(root, relative, settings)
                                .ok()
                                .flatten()
                        });
                        if let Some(meshes) = meshes {
                            let mut combined = Mesh {
                                entity: render_id(id),
                                name: name.to_owned(),
                                vertices: Vec::new(),
                                indices: Vec::new(),
                                material,
                            };
                            for mesh in meshes {
                                let offset = combined.vertices.len() as u32;
                                combined.vertices.extend(mesh.vertices);
                                combined
                                    .indices
                                    .extend(mesh.indices.iter().map(|i| i + offset));
                            }
                            combined
                        } else {
                            cube(render_id(id), name.to_owned(), material)
                        }
                    } else {
                        reference
                            .id
                            .and_then(|asset| {
                                models
                                    .load(
                                        asset,
                                        &renderer,
                                        render_id(id),
                                        name.to_owned(),
                                        material,
                                    )
                                    .ok()
                            })
                            .unwrap_or_else(|| cube(render_id(id), name.to_owned(), material))
                    }
                } else {
                    shape(&renderer, render_id(id), name.to_owned(), material)
                };
                let normal = matrix.inverse().transpose();
                for v in &mut mesh.vertices {
                    v.position = matrix.transform_point3(v.position.into()).into();
                    v.normal = normal
                        .transform_vector3(v.normal.into())
                        .normalize_or_zero()
                        .into();
                    let tangent = matrix
                        .transform_vector3(Vec3::from_slice(&v.tangent[..3]))
                        .normalize_or_zero();
                    v.tangent[..3].copy_from_slice(&tangent.to_array());
                }
                scene.meshes.push(mesh);
            }
            if let Some(light) = entity.record.get("Light") {
                let position = matrix.transform_point3(Vec3::ZERO);
                let color = vector(light.get("color"), Vec3::ONE);
                let kind = light.get("kind").and_then(Value::as_str).unwrap_or("point");
                let intensity = light
                    .get("intensity")
                    .and_then(Value::as_f64)
                    .unwrap_or(1000.) as f32;
                if kind == "directional" {
                    scene.sun_direction = -matrix.transform_vector3(-Vec3::Z).normalize_or_zero();
                    scene.sun_lux = color * intensity;
                } else {
                    anyhow::ensure!(
                        scene.lights.len() < 64,
                        "baseline viewport supports 64 point/spot lights"
                    );
                    let direction = matrix.transform_vector3(-Vec3::Z).normalize_or_zero();
                    scene.lights.push(Light {
                        position_range:
                            position
                                .extend(light.get("range").and_then(Value::as_f64).unwrap_or(10.)
                                    as f32)
                                .into(),
                        color_lumens: color.extend(intensity).into(),
                        direction_outer: direction.extend(0.8).into(),
                        parameters: [if kind == "spot" { 1. } else { 0. }, 0.94, 0., 0.],
                    });
                }
            }
        }
        if only.is_some() {
            return Ok(scene);
        }
        self.append_ghosts(root, state, &transforms, &mut scene, models)?;
        // A tiny non-pickable fallback keeps the baseline renderer valid for an empty scene.
        if scene.meshes.is_empty() {
            let mut mesh = cube(0, "Empty scene".into(), 0);
            for vertex in &mut mesh.vertices {
                vertex.position = [100_000., 100_000., 100_000.];
            }
            scene.meshes.push(mesh);
        }
        Ok(scene)
    }
}
/// The raster material a `MeshRenderer` draws with (`MATERIAL_FIELDS` in `patch.rs`
/// lists what it reads).
fn raster_material(renderer: &MeshRenderer) -> Material {
    let [r, g, b] = renderer.emissive;
    Material {
        base_color: renderer.color.0,
        emissive_roughness: [r, g, b, renderer.roughness],
        surface: [renderer.metallic, 1.0, 1.0, 0.5],
        textures: [0, 1, 0, 0],
        flags: [u32::from(renderer.color.0[3] < 1.0) * 2, 0, 0, 0],
    }
}
/// The engine's procedural shape for a `MeshRenderer` without a model (model space).
fn shape(renderer: &MeshRenderer, entity: u64, name: String, material: usize) -> Mesh {
    let source = ara_runtime_render::geometry::shape(renderer.shape, renderer.size);
    Mesh {
        entity,
        name,
        vertices: source
            .vertices
            .iter()
            .map(|v| Vertex {
                position: v.position,
                normal: v.normal,
                uv: v.uv,
                tangent: v.tangent,
            })
            .collect(),
        indices: source.indices,
        material,
    }
}
fn cube(entity: u64, name: String, material: usize) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (normal, u, v) in [
        (Vec3::X, -Vec3::Z, Vec3::Y),
        (-Vec3::X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, -Vec3::Z),
        (-Vec3::Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (-Vec3::Z, -Vec3::X, Vec3::Y),
    ] {
        let base = vertices.len() as u32;
        for [x, y] in [[0., 0.], [1., 0.], [1., 1.], [0., 1.]] {
            vertices.push(Vertex {
                position: (normal * 0.5 + u * (x - 0.5) + v * (y - 0.5)).into(),
                normal: normal.into(),
                uv: [x, y],
                tangent: u.extend(1.).into(),
            });
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh {
        entity,
        name,
        vertices,
        indices,
        material,
    }
}
