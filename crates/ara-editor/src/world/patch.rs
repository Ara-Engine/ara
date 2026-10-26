//! In-place Scene updates for value edits. A document change that only moves meshes,
//! recolours or renames them, or edits lights patches the renderer (`update_mesh`,
//! `update_materials`, `rename`, the light list) instead of extracting and uploading
//! the whole scene again (11 s per commit on Outpost before the model cache, ~0.8 s
//! after it). Typed Inspector drafts and gizmo drags preview through the same path.
use super::*;
use ara_render::Renderer;

/// What the Scene renderer needs to go from one document view to another, short of a
/// full extraction. Build one with [`DocumentView::patch_to`] or
/// [`DocumentView::gesture_patch`]; upload it with [`DocumentView::apply`].
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Patch {
    /// Drawn mesh entities whose world transform changed.
    pub moved: BTreeSet<String>,
    /// A drawn light, or the sun, changed.
    pub lights: bool,
    /// A drawn mesh's material fields changed (the material block uploads again).
    pub materials: bool,
    /// Drawn meshes whose name (capture label) changed.
    pub renamed: BTreeSet<String>,
}

/// `MeshRenderer` fields that only feed the raster material (`raster_material`).
const MATERIAL_FIELDS: [&str; 4] = ["color", "emissive", "roughness", "metallic"];

/// Components the Scene extracts beyond meshes, lights and transforms (environment,
/// post-processing, terrain, effects): a change to any of them, even on a hidden
/// entity, needs a full extraction.
const SCENE_WIDE: [&str; 8] = [
    "Environment",
    "SkyAtmosphere",
    "HeightFog",
    "PostProcessVolume",
    "Terrain",
    "TerrainChunk",
    "TerrainView",
    "VisualEffect",
];

/// Whether every document changed after revision `since` reaches the Scene through
/// the open `scene`'s entities: the scene itself, the prefabs it instances (resolved
/// into its records) and import settings (compared as a view's `imports`). Anything
/// else (a material graph, the project's render settings, an environment asset) needs
/// a full extraction, since a [`DocumentView`] does not see it.
pub fn scene_only(store: &Store, since: u64, scene: &str) -> bool {
    let workspace = store.workspace();
    store
        .since(since)
        .iter()
        .flat_map(|transaction| &transaction.ops)
        .all(|op| {
            let doc = op.doc().as_str();
            doc == scene
                || workspace
                    .doc(doc)
                    .is_some_and(|d| matches!(d.kind.as_str(), "prefab" | "import"))
        })
}

impl Patch {
    /// Whether the renderer already shows the target.
    pub fn is_empty(&self) -> bool {
        self.moved.is_empty() && !self.lights && !self.materials && self.renamed.is_empty()
    }
    /// Everything either patch touches: applied to the newer view, it also undoes a
    /// preview the renderer still shows.
    pub fn union(mut self, other: &Patch) -> Patch {
        self.moved.extend(other.moved.iter().cloned());
        self.lights |= other.lights;
        self.materials |= other.materials;
        self.renamed.extend(other.renamed.iter().cloned());
        self
    }
}

impl DocumentView {
    /// The patch that takes the Scene from this view to `next`, or None when only a
    /// full extraction will do: entities added, removed or re-parented, import
    /// settings or ghosts changed, a drawn `MeshRenderer` changed more than its
    /// material fields, or a scene-wide component changed (environment, post, terrain,
    /// effects). Fields the Scene does not draw never need one. Documents other than
    /// the scene are not compared here: see [`scene_only`].
    pub(crate) fn patch_to(&self, next: &DocumentView, hidden: &BTreeSet<String>) -> Option<Patch> {
        if self.entities.len() != next.entities.len()
            || !self.ghosts.is_empty()
            || !next.ghosts.is_empty()
            || !same_imports(&self.imports, &next.imports)
        {
            return None;
        }
        let mut patch = Patch::default();
        // Entities whose own Transform changed: only they and their descendants move.
        let mut transformed = BTreeSet::new();
        for ((id, a), (other, b)) in self.entities.iter().zip(next.entities.iter()) {
            if id != other || a.parent != b.parent {
                return None;
            }
            if a.record == b.record {
                continue;
            }
            if SCENE_WIDE
                .iter()
                .any(|key| a.record.get(*key) != b.record.get(*key))
            {
                return None;
            }
            // A hidden entity's transform still moves its children.
            if a.record.get("Transform") != b.record.get("Transform") {
                transformed.insert(id.as_str());
            }
            if hidden.contains(id) {
                continue;
            }
            let mesh =
                a.record.contains_key("MeshRenderer") || b.record.contains_key("MeshRenderer");
            for key in a.record.keys().chain(b.record.keys()) {
                if a.record.get(key) == b.record.get(key) {
                    continue;
                }
                match key.as_str() {
                    "Light" => patch.lights = true,
                    "MeshRenderer" if same_geometry(a.record.get(key), b.record.get(key)) => {
                        patch.materials = true;
                    }
                    "MeshRenderer" => return None,
                    "name" if mesh => {
                        patch.renamed.insert(id.clone());
                    }
                    // Transforms are compared as world matrices below; nothing else
                    // is drawn.
                    _ => {}
                }
            }
        }
        if !transformed.is_empty() {
            // Parents are unchanged here, so world matrices are composed only for the
            // moved subtrees, not for the whole scene twice.
            let world = |entities: &Resolved| -> BTreeMap<String, Mat4> {
                next.entities
                    .keys()
                    .filter(|id| under(&next.entities, id, &transformed))
                    .filter_map(|id| Some((id.clone(), world_transform(entities, id)?)))
                    .collect()
            };
            moved(
                &world(&self.entities),
                &world(&next.entities),
                &next.entities,
                hidden,
                &mut patch,
            );
        }
        Some(patch)
    }

    /// What a gizmo drag draws on top of this view: every drawn mesh and light it moves.
    pub(crate) fn gesture_patch(
        &self,
        gesture: &gesture::Gesture,
        hidden: &BTreeSet<String>,
    ) -> Patch {
        let before = world_transforms(&self.entities);
        let mut after = BTreeMap::new();
        gesture.preview_world(&self.entities, &mut after);
        let mut patch = Patch::default();
        moved(&before, &after, &self.entities, hidden, &mut patch);
        patch
    }

    /// This view with some entities' records replaced (an uncommitted Inspector
    /// draft); unknown entities are ignored.
    pub(crate) fn with_records(&self, records: BTreeMap<String, ara_doc::Record>) -> DocumentView {
        let mut entities = (*self.entities).clone();
        for (id, record) in records {
            if let Some(entity) = entities.get_mut(&id) {
                entity.record = record;
            }
        }
        DocumentView {
            entities: std::sync::Arc::new(entities),
            ghosts: self.ghosts.clone(),
            imports: self.imports.clone(),
        }
    }

    /// Bring the renderer to this view (with `preview` drawn on top) through `patch`:
    /// moved meshes are extracted alone and replace their vertices; lights and
    /// materials upload as whole (small) blocks; renamed meshes relabel. An error can
    /// leave the renderer part-way, so callers fall back to a full update.
    pub(crate) fn apply(
        &self,
        patch: &Patch,
        renderer: &mut Renderer,
        root: &Path,
        state: &EditorState,
        preview: Option<&gesture::Gesture>,
        models: &mut ModelCache,
    ) -> anyhow::Result<()> {
        if patch.is_empty() {
            return Ok(());
        }
        if !patch.moved.is_empty() || patch.lights {
            let scene = self.extract(root, state, preview, models, Some(&patch.moved))?;
            for mesh in &scene.meshes {
                renderer.update_mesh(mesh.entity, &mesh.vertices)?;
            }
            if patch.lights {
                renderer.set_lights(&scene.lights)?;
                renderer.set_sun(scene.sun_direction, scene.sun_lux)?;
            }
        }
        if patch.materials {
            renderer.update_materials(self.materials(state))?;
        }
        for id in &patch.renamed {
            let name = self.entities.get(id).and_then(|e| e.record.get("name"));
            let name = name.and_then(Value::as_str).unwrap_or(id);
            renderer.rename(render_id(id), name.to_owned())?;
        }
        Ok(())
    }

    /// The raster materials in extraction order: the default, then one per drawn
    /// `MeshRenderer` (as `extract` pushes them).
    fn materials(&self, state: &EditorState) -> Vec<Material> {
        let mut materials = vec![Material::default()];
        for (id, entity) in self.entities.iter() {
            if let Some(record) = entity.record.get("MeshRenderer")
                && !state.hidden.contains(id)
            {
                let renderer: MeshRenderer =
                    serde_json::from_value(record.clone()).unwrap_or_default();
                materials.push(raster_material(&renderer));
            }
        }
        materials
    }
}

/// Whether `id` or one of its ancestors is in `set` (a parent cycle stops the walk).
fn under(entities: &Resolved, id: &str, set: &BTreeSet<&str>) -> bool {
    let mut current = Some(id);
    for _ in 0..=entities.len() {
        let Some(at) = current else {
            return false;
        };
        if set.contains(at) {
            return true;
        }
        current = entities.get(at).and_then(|e| e.parent.as_deref());
    }
    false
}

/// Record the drawn meshes and lights whose world matrix differs.
fn moved(
    before: &BTreeMap<String, Mat4>,
    after: &BTreeMap<String, Mat4>,
    entities: &Resolved,
    hidden: &BTreeSet<String>,
    patch: &mut Patch,
) {
    for (id, entity) in entities {
        if hidden.contains(id) || before.get(id) == after.get(id) {
            continue;
        }
        if entity.record.contains_key("MeshRenderer") {
            patch.moved.insert(id.clone());
        }
        if entity.record.contains_key("Light") {
            patch.lights = true;
        }
    }
}

/// Whether two `MeshRenderer` records draw the same geometry: they differ at most in
/// material fields.
fn same_geometry(a: Option<&Value>, b: Option<&Value>) -> bool {
    fn geometry(v: Option<&Value>) -> Option<BTreeMap<&String, &Value>> {
        let fields = v?.as_object()?.iter();
        Some(
            fields
                .filter(|(k, _)| !MATERIAL_FIELDS.contains(&k.as_str()))
                .collect(),
        )
    }
    matches!((geometry(a), geometry(b)), (Some(a), Some(b)) if a == b)
}

/// Import settings compare by value (they decide how FBX meshes cook).
fn same_imports(a: &std::sync::Arc<super::Imports>, b: &std::sync::Arc<super::Imports>) -> bool {
    std::sync::Arc::ptr_eq(a, b)
        || a.len() == b.len()
            && a.iter().zip(b.iter()).all(|((pa, sa), (pb, sb))| {
                pa == pb && serde_json::to_value(sa).ok() == serde_json::to_value(sb).ok()
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_doc::{AssetId, RecordId, prefab::ResolvedEntity};
    use serde_json::json;

    fn entity(parent: Option<&str>, record: Value) -> ResolvedEntity {
        ResolvedEntity {
            record: record.as_object().cloned().unwrap_or_default(),
            parent: parent.map(str::to_owned),
            order: None,
            source: (
                AssetId::parse("sc_test1234").expect("asset ID"),
                RecordId::parse("e_test").expect("record ID"),
            ),
        }
    }
    /// A crate with a mesh, a mesh child, an empty pivot and a lamp.
    fn view() -> DocumentView {
        let mesh = |x: f64| json!({"name": "Crate", "Transform": {"position": [x, 0., 0.]}, "MeshRenderer": {"color": [1., 0., 0., 1.]}});
        DocumentView {
            entities: std::sync::Arc::new(Resolved::from([
                ("e_crate".to_owned(), entity(None, mesh(0.))),
                ("e_lid".to_owned(), entity(Some("e_crate"), mesh(1.))),
                (
                    "e_pivot".to_owned(),
                    entity(None, json!({"name": "Pivot", "Transform": {}})),
                ),
                (
                    "e_lamp".to_owned(),
                    entity(None, json!({"Light": {"intensity": 800.}})),
                ),
            ])),
            ghosts: BTreeMap::new(),
            imports: Default::default(),
        }
    }
    fn edit(view: &DocumentView, id: &str, path: &[&str], value: Value) -> DocumentView {
        let mut record = Value::Object(view.entities[id].record.clone());
        let mut at = &mut record;
        for key in path {
            at = &mut at[*key];
        }
        *at = value;
        view.with_records(
            [(
                id.to_owned(),
                record.as_object().cloned().unwrap_or_default(),
            )]
            .into(),
        )
    }
    fn moved(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn moving_a_mesh_moves_it_and_its_children_only() {
        let before = view();
        let after = edit(
            &before,
            "e_crate",
            &["Transform", "position"],
            json!([2., 0., 0.]),
        );
        let patch = before.patch_to(&after, &BTreeSet::new()).expect("a patch");
        assert_eq!(patch.moved, moved(&["e_crate", "e_lid"]));
        assert!(!patch.lights);
        let hidden = moved(&["e_lid"]);
        let patch = before.patch_to(&after, &hidden).expect("a patch");
        assert_eq!(
            patch.moved,
            moved(&["e_crate"]),
            "hidden meshes are not drawn"
        );
    }

    #[test]
    fn scene_wide_components_need_a_full_extraction() {
        let before = view();
        let fog = edit(&before, "e_pivot", &["HeightFog", "density"], json!(0.2));
        assert_eq!(before.patch_to(&fog, &BTreeSet::new()), None);
        // Hidden or not: the environment applies to the whole Scene.
        let hidden = moved(&["e_pivot"]);
        assert_eq!(before.patch_to(&fog, &hidden), None);
        let script = edit(&before, "e_pivot", &["Spinner", "speed"], json!(2.));
        assert_eq!(
            before.patch_to(&script, &BTreeSet::new()),
            Some(Patch::default()),
            "a component the Scene does not draw needs nothing"
        );
    }

    #[test]
    fn lights_patch_and_undrawn_fields_need_nothing() {
        let before = view();
        let lamp = edit(&before, "e_lamp", &["Light", "intensity"], json!(1600.));
        let patch = before.patch_to(&lamp, &BTreeSet::new()).expect("a patch");
        assert!(patch.lights && patch.moved.is_empty());
        let renamed = edit(&before, "e_pivot", &["name"], json!("Hinge"));
        assert_eq!(
            before.patch_to(&renamed, &BTreeSet::new()),
            Some(Patch::default())
        );
        let pivot = edit(
            &before,
            "e_pivot",
            &["Transform", "position"],
            json!([0., 5., 0.]),
        );
        assert_eq!(
            before.patch_to(&pivot, &BTreeSet::new()),
            Some(Patch::default())
        );
    }

    #[test]
    fn recolours_and_renames_patch_in_place() {
        let before = view();
        let recoloured = edit(
            &before,
            "e_crate",
            &["MeshRenderer", "color"],
            json!([0., 1., 0., 1.]),
        );
        let patch = before
            .patch_to(&recoloured, &BTreeSet::new())
            .expect("a patch");
        assert!(patch.materials && patch.moved.is_empty() && patch.renamed.is_empty());
        let renamed = edit(&before, "e_crate", &["name"], json!("Box"));
        let patch = before
            .patch_to(&renamed, &BTreeSet::new())
            .expect("a patch");
        assert_eq!(patch.renamed, moved(&["e_crate"]));
        assert!(!patch.materials);
    }

    #[test]
    fn geometry_and_structure_need_a_full_extraction() {
        let before = view();
        let reshaped = edit(
            &before,
            "e_crate",
            &["MeshRenderer", "shape"],
            json!("sphere"),
        );
        assert_eq!(before.patch_to(&reshaped, &BTreeSet::new()), None);
        let mut fewer = view();
        std::sync::Arc::make_mut(&mut fewer.entities).remove("e_pivot");
        assert_eq!(before.patch_to(&fewer, &BTreeSet::new()), None);
        let mut moved_out = view();
        std::sync::Arc::make_mut(&mut moved_out.entities)
            .get_mut("e_lid")
            .expect("lid")
            .parent = None;
        assert_eq!(before.patch_to(&moved_out, &BTreeSet::new()), None);
    }
}
