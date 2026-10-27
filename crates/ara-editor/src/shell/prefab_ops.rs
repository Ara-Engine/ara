//! Prefab workflows lower to one typed cross-document transaction per human action.
use super::commands::Target;
use super::*;

/// Root placement belongs to the instance rather than the source prefab.
const PLACEMENT: &[&str] = &[
    "Transform.position",
    "Transform.rotation",
    "parent",
    "order",
];

/// Source provenance and authored overrides of a selected prefab entity.
pub struct Instance {
    /// Entity path below the outer scene instance, absent for its root.
    pub inner: Option<String>,
    /// Nearest owning prefab or variant document identity.
    pub prefab_doc: String,
    /// Canonical project-relative owning source path.
    pub prefab_path: String,
    /// Entity address in that source, including arbitrary nested boundaries.
    pub prefab_record: Option<String>,
    /// Local override keys on this selected entity, excluding root placement.
    pub overrides: Vec<String>,
}

/// Every nested/variant source the user can choose as an Apply To destination.
pub fn source_choices(
    store: &ara_ops::Store,
    doc: &str,
    entity: &str,
) -> Vec<ara_cmd::builtin::PrefabSource> {
    ara_cmd::builtin::prefab_sources(store.workspace(), doc, entity).unwrap_or_default()
}

/// Find the nearest owning prefab without flattening a nested or variant source.
pub fn instance(store: &ara_ops::Store, doc: &str, entity: &str) -> Option<Instance> {
    let workspace = store.workspace();
    let scene = workspace.doc(workspace.find(doc)?.as_str())?;
    let (root, inner) = entity
        .split_once('/')
        .map_or((entity, None), |(root, inner)| {
            (root, Some(inner.to_owned()))
        });
    let record = scene.record(root)?;
    ara_doc::prefab_of(record)?;
    let source = source_choices(store, doc, entity).into_iter().next()?;
    let prefix = inner.as_ref().map(|inner| format!("{inner}/"));
    let overrides = record
        .get("overrides")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|map| map.keys())
        .filter_map(|key| match &prefix {
            Some(prefix) => key.strip_prefix(prefix).map(str::to_owned),
            None => (!key.contains('/') && !PLACEMENT.contains(&key.as_str())).then(|| key.clone()),
        })
        .collect();
    Some(Instance {
        inner,
        prefab_doc: source.doc.to_string(),
        prefab_path: source.path,
        prefab_record: Some(source.entity),
        overrides,
    })
}

impl Shell {
    /// Preserve context identity and field intent when a source submenu closes.
    pub(crate) fn apply_to_target(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Choice {
            entity: String,
            path: Option<String>,
            destination: String,
        }
        let Target::Text(text) = target else {
            return Err("Choose a prefab source destination".into());
        };
        let choice = if text.starts_with('@') {
            Choice {
                entity: editor
                    .state
                    .selection
                    .first()
                    .ok_or("Select a prefab instance")?
                    .clone(),
                path: None,
                destination: text.clone(),
            }
        } else {
            serde_json::from_str::<Choice>(text)?
        };
        self.apply_overrides_to(
            editor,
            &choice.entity,
            choice.path.as_deref(),
            Some(&choice.destination),
        )
    }

    /// Create/populate the asset and replace its source as one undoable transaction.
    /// Existing instances create inherited variants, preserving their base reference.
    pub(crate) fn prefab_from_selection(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let root = target
            .entity(&editor.state)
            .ok_or("Select an entity")?
            .to_owned();
        let resolved = self.resolved(editor)?;
        let name = resolved
            .get(&root)
            .and_then(|entity| entity.record.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Prefab");
        let slug: String = name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let slug = slug.trim_matches('_');
        let slug = if slug.is_empty() { "prefab" } else { slug };
        let workspace = editor.host.store().workspace();
        let path = (1..)
            .map(|index| {
                if index == 1 {
                    format!("prefabs/{slug}.prefab")
                } else {
                    format!("prefabs/{slug}_{index}.prefab")
                }
            })
            .find(|path| ops::authored::available(&self.browser.files, workspace, path))
            .ok_or("No unused prefab path")?;
        let variant = instance(editor.host.store(), &editor.state.doc, &root).is_some();
        let result = editor.apply(vec![json!({"cmd": if variant {"prefab.variant"} else {"prefab.create"}, "args": {"entity": root, "path": path}})], "Create prefab from selection", None)?;
        let entity = result["outputs"]
            .as_array()
            .and_then(|outputs| outputs.iter().find_map(|output| output["entity"].as_str()))
            .ok_or("Prefab creation returned no instance")?;
        editor.action("editor.select", json!({"entities": [entity]}))?;
        self.status_note = Some(format!("Created {path}"));
        Ok(())
    }

    /// Apply to the nearest owning prefab; source and instance change together.
    pub(crate) fn apply_overrides(
        &mut self,
        editor: &mut Editor,
        entity: &str,
        path: Option<&str>,
    ) -> ara_ui::Result<()> {
        self.apply_overrides_to(editor, entity, path, None)
    }

    /// Apply to an explicit nested/variant source chosen from source_choices.
    pub(crate) fn apply_overrides_to(
        &mut self,
        editor: &mut Editor,
        entity: &str,
        path: Option<&str>,
        destination: Option<&str>,
    ) -> ara_ui::Result<()> {
        let result = editor.apply(vec![json!({"cmd": "prefab.apply", "args": {"entity": entity, "path": path, "destination": destination}})], "Apply prefab overrides", None)?;
        let source = result["outputs"]
            .as_array()
            .and_then(|outputs| outputs.iter().find_map(|output| output["path"].as_str()))
            .unwrap_or("prefab");
        self.status_note = Some(format!("Applied to {source}"));
        Ok(())
    }
}
