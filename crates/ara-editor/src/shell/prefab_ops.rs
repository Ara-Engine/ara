//! Prefab workflows through typed commands: Prefab from Selection (copy a subtree
//! into a new `.prefab`, then replace it with a linked instance in place), and
//! applying or reverting an instance's overrides.
use super::commands::Target;
use super::*;

/// Instance-root fields that place the instance rather than change the prefab.
const PLACEMENT: &[&str] = &[
    "Transform.position",
    "Transform.rotation",
    "parent",
    "order",
];

/// What an entity inside a prefab instance points at.
pub struct Instance {
    /// Path inside the instance (`e_m4tz`), `None` for the root itself.
    pub inner: Option<String>,
    /// The prefab document ID.
    pub prefab_doc: String,
    /// The prefab's project path (`prefabs/crate.prefab`).
    pub prefab_path: String,
    /// The prefab record the entity comes from (its root for the instance root).
    pub prefab_record: Option<String>,
    /// Override keys stored on the instance root, for this entity.
    pub overrides: Vec<String>,
}

/// The prefab behind `entity` in `doc`, if it is (inside) an instance.
pub fn instance(store: &ara_ops::Store, doc: &str, entity: &str) -> Option<Instance> {
    let workspace = store.workspace();
    let scene = workspace.doc(workspace.find(doc)?.as_str())?;
    let (root, inner) = match entity.split_once('/') {
        Some((root, inner)) => (root, Some(inner.to_owned())),
        None => (entity, None),
    };
    let record = scene.record(root)?;
    let reference = ara_doc::prefab_of(record)?;
    let parts = ara_schema::grammar::parse_ref(reference).ok()?;
    let prefab_doc = parts.id.map(str::to_owned).or_else(|| {
        parts
            .path
            .and_then(|p| workspace.find(p))
            .map(ToString::to_string)
    })?;
    let prefab_path = workspace.path_of(&prefab_doc)?.to_owned();
    let prefab = workspace.doc(&prefab_doc)?;
    let prefab_record = match &inner {
        // One level deep: nested prefab instances keep their own overrides.
        Some(inner) if !inner.contains('/') => Some(inner.clone()),
        Some(_) => None,
        None => prefab
            .records()
            .iter()
            .find(|(_, r)| r.get(ara_doc::field::PARENT).is_none())
            .map(|(id, _)| id.to_string()),
    };
    let prefix = inner.as_ref().map(|i| format!("{i}/"));
    let overrides = record
        .get("overrides")
        .and_then(Value::as_object)
        .map(|o| {
            o.keys()
                .filter_map(|k| match &prefix {
                    Some(p) => k.strip_prefix(p.as_str()).map(str::to_owned),
                    // Where the instance sits is its own business, not the prefab's
                    // (Unity's "default overrides"): never counted or applied.
                    None => {
                        (!k.contains('/') && !PLACEMENT.contains(&k.as_str())).then(|| k.clone())
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Instance {
        inner,
        prefab_doc,
        prefab_path,
        prefab_record,
        overrides,
    })
}

impl Shell {
    /// Prefab from Selection: the selected entity and its children become
    /// `prefabs/<name>.prefab`, and the scene keeps a linked instance where they were.
    pub(crate) fn prefab_from_selection(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let root = target
            .entity(&editor.state)
            .ok_or("select an entity")?
            .to_owned();
        if root.contains('/') || instance(editor.host.store(), &editor.state.doc, &root).is_some() {
            return Err("this is already a prefab instance; unpack it first".into());
        }
        let workspace = editor.host.store().workspace();
        let scene = workspace
            .doc(&editor.state.doc)
            .ok_or("open a scene first")?;
        // The subtree in parent-first order, from the scene's own records.
        let mut subtree = vec![root.clone()];
        let mut index = 0;
        while index < subtree.len() {
            let parent = subtree[index].clone();
            let mut children: Vec<(String, String)> = scene
                .records()
                .iter()
                .filter(|(_, r)| {
                    r.get(ara_doc::field::PARENT).and_then(Value::as_str) == Some(parent.as_str())
                })
                .map(|(id, r)| {
                    (
                        r.get(ara_doc::field::ORDER)
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        id.to_string(),
                    )
                })
                .collect();
            children.sort();
            subtree.extend(children.into_iter().map(|(_, id)| id));
            index += 1;
        }
        let records: Vec<(String, ara_doc::Record)> = subtree
            .iter()
            .filter_map(|id| scene.record(id).map(|r| (id.clone(), r.clone())))
            .collect();
        let (_, root_record) = records.first().ok_or("entity not found")?;
        let name = root_record
            .get(ara_doc::field::NAME)
            .and_then(Value::as_str)
            .unwrap_or("Prefab")
            .to_owned();
        let parent = root_record
            .get(ara_doc::field::PARENT)
            .and_then(Value::as_str)
            .map(str::to_owned);
        let position = root_record
            .get("Transform")
            .and_then(|t| t.get("position"))
            .cloned()
            .unwrap_or_else(|| json!([0, 0, 0]));
        let slug: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .trim_matches('_')
            .to_owned();
        let slug = if slug.is_empty() {
            "prefab".to_owned()
        } else {
            slug
        };
        let root_dir = editor.host.root().to_owned();
        let path = (1..)
            .map(|n| {
                if n == 1 {
                    format!("prefabs/{slug}.prefab")
                } else {
                    format!("prefabs/{slug}_{n}.prefab")
                }
            })
            .find(|p| !root_dir.join(p).exists() && workspace.find(p).is_none())
            .unwrap_or_else(|| format!("prefabs/{slug}.prefab"));
        // Build the prefab: the root sits at its origin; children keep local transforms.
        let symbol = |id: &str| format!("${}", id.replace(['/', '-'], "_"));
        let mut commands = Vec::new();
        for (n, (id, record)) in records.iter().enumerate() {
            let mut components = serde_json::Map::new();
            for (key, value) in record {
                if ara_doc::is_component_key(key) {
                    components.insert(key.clone(), value.clone());
                }
            }
            if n == 0
                && let Some(transform) = components.get_mut("Transform")
                && let Some(t) = transform.as_object_mut()
            {
                t.remove("position");
            }
            let mut args = json!({"id": symbol(id), "name": record.get(ara_doc::field::NAME).cloned().unwrap_or_else(|| json!("Entity")), "components": components});
            if n > 0
                && let Some(parent) = record.get(ara_doc::field::PARENT).and_then(Value::as_str)
            {
                args["parent"] = symbol(parent).into();
            }
            commands.push(json!({"cmd": "entity.create", "args": args}));
        }
        let created = editor.action(
            "editor.resource.create",
            json!({"path": path, "domain": "prefab"}),
        )?;
        let doc = created["doc"]
            .as_str()
            .ok_or("prefab creation returned no document")?
            .to_owned();
        editor.apply_to(&doc, commands, &format!("Create prefab {name}"))?;
        // Replace the subtree with a linked instance at the same place.
        let mut instantiate = json!({"prefab": format!("@{path}"), "id": "$instance", "name": name, "position": position});
        if let Some(parent) = parent {
            instantiate["parent"] = parent.into();
        }
        let result = editor.apply(
            vec![
                json!({"cmd": "entity.delete", "args": {"entities": [root]}}),
                json!({"cmd": "prefab.instantiate", "args": instantiate}),
            ],
            &format!("Replace {name} with a prefab instance"),
            None,
        )?;
        let id = result["outputs"]
            .as_array()
            .and_then(|o| o.iter().find_map(|v| v.get("id").cloned()))
            .ok_or("instantiate returned no entity")?;
        editor.action("editor.select", json!({"entities": [id]}))?;
        self.status_note = Some(format!("Created {path}"));
        Ok(())
    }

    /// Writes an instance override into the prefab (every instance gets it), then
    /// drops the override so this instance follows the prefab again.
    pub(crate) fn apply_overrides(
        &mut self,
        editor: &mut Editor,
        entity: &str,
        path: Option<&str>,
    ) -> ara_ui::Result<()> {
        let store = editor.host.store();
        let found = instance(store, &editor.state.doc, entity).ok_or("not a prefab instance")?;
        let record = found
            .prefab_record
            .clone()
            .ok_or("overrides on nested prefab instances apply to their own prefab")?;
        let resolved =
            crate::world::resolve(store, &editor.state.doc).map_err(|d| format!("{d:?}"))?;
        let values = &resolved.get(entity).ok_or("entity not found")?.record;
        let keys: Vec<String> = found
            .overrides
            .iter()
            .filter(|k| path.is_none_or(|p| k.as_str() == p || k.starts_with(&format!("{p}."))))
            .cloned()
            .collect();
        if keys.is_empty() {
            return Err("nothing is overridden here".into());
        }
        let mut commands = Vec::new();
        for key in &keys {
            let mut current: Option<&Value> = None;
            for (n, part) in key.split('.').enumerate() {
                current = if n == 0 {
                    values.get(part)
                } else {
                    current.and_then(|v| v.get(part))
                };
            }
            let Some(value) = current.cloned() else {
                continue;
            };
            commands.push(json!({"cmd": "property.set", "args": {"entity": record, "path": key, "value": value}}));
        }
        let prefab_doc = found.prefab_doc.clone();
        editor.apply_to(&prefab_doc, commands, "Apply overrides to prefab")?;
        let reverts = keys
            .iter()
            .map(
                |k| json!({"cmd": "prefab.override.revert", "args": {"entity": entity, "path": k}}),
            )
            .collect();
        editor.apply(reverts, "Instance follows the prefab again", None)?;
        self.status_note = Some(format!("Applied to {}", found.prefab_path));
        Ok(())
    }
}
