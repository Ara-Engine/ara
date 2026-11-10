//! Prefab instances as the Inspector and commands see them: where an entity comes from
//! ([`instance`], [`source_choices`]) and the human actions on it, each one typed
//! cross-document transaction ([`apply`] overrides to a source, [`from_selection`]).
use serde_json::{Value, json};

use crate::Editor;

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

impl Instance {
    /// Whether field `path` (or a field inside it, or the struct holding it) is overridden.
    pub fn overrides_field(&self, path: &str) -> bool {
        self.overrides.iter().any(|k| {
            k == path || k.starts_with(&format!("{path}.")) || path.starts_with(&format!("{k}."))
        })
    }
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

/// Writes `entity`'s overrides (only field `path` when given) into its nearest prefab, or
/// into `destination` (a nested prefab, variant or base from [`source_choices`]); the
/// source and the instance change in one undo step. Gives back the source written to.
pub fn apply(
    editor: &mut Editor,
    entity: &str,
    path: Option<&str>,
    destination: Option<&str>,
) -> ara_ui::Result<String> {
    let result = editor.apply(
        vec![json!({"cmd": "prefab.apply", "args": {"entity": entity, "path": path, "destination": destination}})],
        "Apply prefab overrides",
        None,
    )?;
    Ok(result["outputs"]
        .as_array()
        .and_then(|outputs| outputs.iter().find_map(|output| output["path"].as_str()))
        .unwrap_or("prefab")
        .to_owned())
}

/// Turns `root` and its children into a prefab under `prefabs/` (a variant when `root`
/// is already an instance), replacing them with a linked instance in one undo step, and
/// selects the instance. `files` is the project index, so the name picked is unused.
/// Gives back the prefab's path.
pub fn from_selection(
    editor: &mut Editor,
    files: &[ara_host::project::AuthoredFile],
    root: &str,
) -> ara_ui::Result<String> {
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
        .map_err(|_| "The open scene does not resolve; see Problems")?;
    let name = resolved
        .get(root)
        .and_then(|entity| entity.record.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("Prefab");
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let slug = match slug.trim_matches('_') {
        "" => "prefab",
        slug => slug,
    };
    let workspace = editor.host.store().workspace();
    let path = (1..)
        .map(|index| match index {
            1 => format!("prefabs/{slug}.prefab"),
            _ => format!("prefabs/{slug}_{index}.prefab"),
        })
        .find(|path| crate::authored::available(files, workspace, path))
        .ok_or("No unused prefab path")?;
    let variant = instance(editor.host.store(), &editor.state.doc, root).is_some();
    let cmd = if variant {
        "prefab.variant"
    } else {
        "prefab.create"
    };
    let result = editor.apply(
        vec![json!({"cmd": cmd, "args": {"entity": root, "path": path}})],
        "Create prefab from selection",
        None,
    )?;
    let entity = result["outputs"]
        .as_array()
        .and_then(|outputs| outputs.iter().find_map(|output| output["entity"].as_str()))
        .ok_or("Prefab creation returned no instance")?;
    editor.action("editor.select", json!({"entities": [entity]}))?;
    Ok(path)
}
