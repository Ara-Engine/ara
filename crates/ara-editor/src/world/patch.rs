//! In-place Scene updates for value edits. A document change that only moves meshes
//! or edits lights patches the renderer (`Renderer::update_mesh`, the light list)
//! instead of extracting and uploading the whole scene again (11 s per commit on
//! Outpost before the model cache, 2 s of upload after it). Typed Inspector drafts
//! and gizmo drags preview through the same path.
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
}

impl Patch {
    /// Whether the renderer already shows the target.
    pub fn is_empty(&self) -> bool {
        self.moved.is_empty() && !self.lights
    }
    /// Everything either patch touches: applied to the newer view, it also undoes a
    /// preview the renderer still shows.
    pub fn union(mut self, other: &Patch) -> Patch {
        self.moved.extend(other.moved.iter().cloned());
        self.lights |= other.lights;
        self
    }
}

impl DocumentView {
    /// The patch that takes the Scene from this view to `next`, or None when only a
    /// full extraction will do: entities added, removed or re-parented, import
    /// settings or ghosts changed, or a drawn mesh's `MeshRenderer` or name changed
    /// (materials and capture labels have no in-place update yet). Fields the Scene
    /// does not draw never need one.
    pub(crate) fn patch_to(&self, next: &DocumentView, hidden: &BTreeSet<String>) -> Option<Patch> {
        if self.entities.len() != next.entities.len()
            || !self.ghosts.is_empty()
            || !next.ghosts.is_empty()
            || !same_imports(&self.imports, &next.imports)
        {
            return None;
        }
        let mut patch = Patch::default();
        let mut transforms = false;
        for ((id, a), (other, b)) in self.entities.iter().zip(&next.entities) {
            if id != other || a.parent != b.parent {
                return None;
            }
            if a.record == b.record {
                continue;
            }
            // A hidden entity's transform still moves its children.
            transforms |= a.record.get("Transform") != b.record.get("Transform");
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
                    "MeshRenderer" => return None,
                    "name" if mesh => return None,
                    // Transforms are compared as world matrices below; nothing else
                    // is drawn.
                    _ => {}
                }
            }
        }
        if transforms {
            let before = world_transforms(&self.entities);
            let after = world_transforms(&next.entities);
            moved(&before, &after, &next.entities, hidden, &mut patch);
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
        let mut entities = self.entities.clone();
        for (id, record) in records {
            if let Some(entity) = entities.get_mut(&id) {
                entity.record = record;
            }
        }
        DocumentView {
            entities,
            ghosts: self.ghosts.clone(),
            imports: self.imports.clone(),
        }
    }

    /// Bring the renderer to this view (with `preview` drawn on top) through `patch`:
    /// moved meshes are extracted alone and replace their vertices, and lights upload
    /// as one list. An error can leave the renderer part-way, so callers fall back to
    /// a full update.
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
        let scene = self.extract(root, state, preview, models, Some(&patch.moved))?;
        for mesh in &scene.meshes {
            renderer.update_mesh(mesh.entity, &mesh.vertices)?;
        }
        if patch.lights {
            renderer.set_lights(&scene.lights)?;
            renderer.set_sun(scene.sun_direction, scene.sun_lux)?;
        }
        Ok(())
    }
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

/// Import settings compare by value (they decide how FBX meshes cook).
fn same_imports(
    a: &BTreeMap<String, ara_asset::ImportSettings>,
    b: &BTreeMap<String, ara_asset::ImportSettings>,
) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((pa, sa), (pb, sb))| {
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
            entities: [
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
            ]
            .into(),
            ghosts: BTreeMap::new(),
            imports: BTreeMap::new(),
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
    fn mesh_settings_names_and_structure_need_a_full_extraction() {
        let before = view();
        let recoloured = edit(
            &before,
            "e_crate",
            &["MeshRenderer", "color"],
            json!([0., 1., 0., 1.]),
        );
        assert_eq!(before.patch_to(&recoloured, &BTreeSet::new()), None);
        let renamed = edit(&before, "e_crate", &["name"], json!("Box"));
        assert_eq!(before.patch_to(&renamed, &BTreeSet::new()), None);
        let mut fewer = view();
        fewer.entities.remove("e_pivot");
        assert_eq!(before.patch_to(&fewer, &BTreeSet::new()), None);
        let mut moved_out = view();
        moved_out.entities.get_mut("e_lid").expect("lid").parent = None;
        assert_eq!(before.patch_to(&moved_out, &BTreeSet::new()), None);
    }
}
