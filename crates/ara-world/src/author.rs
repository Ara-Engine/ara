use crate::{Manifest, Result, TextIndex, invalid};
use ara_doc::{AssetId, Edit, RecordId};
use ara_ops::{Commit, Store, TxnMeta};
use ara_schema::{FieldPath, Schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// Atomically relocates an entity subtree, retaining every stable entity ID.
#[derive(Debug, Schema, Serialize, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveToCell {
    /// Stable entity ID; a stale path in an `@path#id` address is harmless.
    pub entity: String,
    /// Destination manifest cell ID.
    pub cell: String,
}

/// Changes data-layer loading and presentation visibility through undoable ops.
#[derive(Debug, Schema, Serialize, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetLayer {
    /// Stable layer record ID.
    pub layer: String,
    /// Desired runtime/editor loading state.
    pub loaded: bool,
    /// Desired presentation state.
    pub visible: bool,
}

/// World authoring intents. Execute through `apply`; no document side channels.
#[derive(Debug)]
pub enum WorldCommand {
    /// `world.entity.move_to_cell`.
    MoveToCell(MoveToCell),
    /// `world.layer.set`.
    SetLayer(SetLayer),
}

impl WorldCommand {
    /// Plans and commits one attributed, revision-checked multi-document transaction.
    /// The caller journals/persists the resulting kernel transaction like any edit.
    pub fn apply<'a>(
        &self,
        store: &'a mut Store,
        world: &AssetId,
        meta: TxnMeta,
    ) -> Result<Commit<'a>> {
        if let Some(key) = &meta.idempotency_key
            && store.find_by_key(key).is_some()
        {
            return store
                .transact(meta, Vec::new())
                .map_err(crate::Error::Transaction);
        }
        let doc = store
            .workspace()
            .doc(world.as_str())
            .ok_or_else(|| invalid("world is not loaded"))?;
        let manifest = Manifest::read(doc)?;
        let edits = match self {
            Self::SetLayer(params) => {
                if !manifest.layers.contains_key(&params.layer) {
                    return Err(invalid("unknown data layer"));
                }
                let id = RecordId::parse(&params.layer).map_err(invalid)?;
                vec![
                    Edit::Set {
                        doc: world.clone(),
                        id: id.clone(),
                        path: FieldPath::field("loaded"),
                        value: Some(params.loaded.into()),
                    },
                    Edit::Set {
                        doc: world.clone(),
                        id,
                        path: FieldPath::field("visible"),
                        value: Some(params.visible.into()),
                    },
                ]
            }
            Self::MoveToCell(params) => move_edits(store, &manifest, params)?,
        };
        store
            .transact(meta, edits)
            .map_err(crate::Error::Transaction)
    }
}

fn move_edits(store: &Store, manifest: &Manifest, params: &MoveToCell) -> Result<Vec<Edit>> {
    let index = TextIndex::build(manifest, store.workspace(), store.schema())?;
    let entity_id = if params.entity.starts_with('@') {
        ara_schema::grammar::parse_ref(&params.entity)
            .map_err(invalid)?
            .id
            .ok_or_else(|| invalid("entity reference needs an ID"))?
    } else {
        params.entity.as_str()
    };
    let entity = index
        .entity(entity_id)
        .ok_or_else(|| invalid("unknown world entity"))?;
    if entity.cell == params.cell {
        return Ok(Vec::new());
    }
    let destination = manifest
        .cells
        .get(&params.cell)
        .ok_or_else(|| invalid("unknown destination cell"))?;
    let source = store
        .workspace()
        .doc(entity.document.as_str())
        .ok_or_else(|| invalid("source file is not loaded"))?;
    let record = source
        .record(entity_id)
        .ok_or_else(|| invalid("source entity missing"))?;
    if ara_doc::parent_of(record).is_some() {
        return Err(invalid(
            "move a subtree root, or reparent it to a root first",
        ));
    }
    let id = RecordId::parse(entity_id).map_err(invalid)?;
    let subtree = source.subtree(&id);
    // A standalone actor file keeps its readable path and moves by manifest membership.
    let actor_reference = manifest.cells[&entity.cell]
        .actors
        .iter()
        .find(|r| crate::model::document_ref(r).is_ok_and(|(_, id)| id == source.id.as_str()));
    if let Some(reference) = actor_reference
        && subtree.len() == source.records().len()
    {
        let mut from = manifest.cells[&entity.cell].actors.clone();
        from.retain(|r| r != reference);
        let mut to = destination.actors.clone();
        to.push(reference.clone());
        to.sort();
        return Ok(vec![
            Edit::Set {
                doc: manifest.id.clone(),
                id: RecordId::parse(&entity.cell).map_err(invalid)?,
                path: FieldPath::field("actors"),
                value: Some(ara_json::to_value(&from)?),
            },
            Edit::Set {
                doc: manifest.id.clone(),
                id: RecordId::parse(&params.cell).map_err(invalid)?,
                path: FieldPath::field("actors"),
                value: Some(ara_json::to_value(&to)?),
            },
        ]);
    }
    let reference = destination
        .documents
        .first()
        .ok_or_else(|| invalid("destination needs a scene file for inline entities"))?;
    let (target_path, target_id) = crate::model::document_ref(reference)?;
    let target = store
        .workspace()
        .doc(target_id)
        .ok_or_else(|| invalid("destination scene is not loaded"))?;
    if target.kind != "scene" {
        return Err(invalid("destination is not a scene"));
    }
    let moved: BTreeSet<_> = subtree.iter().map(|id| id.as_str()).collect();
    let mut edits = Vec::new();
    for id in &subtree {
        let record = source
            .record(id.as_str())
            .ok_or_else(|| invalid("subtree record missing"))?;
        edits.push(Edit::Create {
            doc: target.id.clone(),
            id: id.clone(),
            record: record.clone(),
        });
        edits.push(Edit::Delete {
            doc: source.id.clone(),
            id: id.clone(),
        });
    }
    // Repair all cross-file references inside the same atomic transaction, including
    // references in unloaded runtime cells. Strings without @path#id are untouched.
    for (_, doc) in store.workspace().docs() {
        if doc.kind != "scene" {
            continue;
        }
        for (id, record) in doc.records() {
            for (field, value) in record {
                let mut next = value.clone();
                repair(&mut next, &moved, target_path);
                if next != *value {
                    edits.push(Edit::Set {
                        doc: if doc.id == source.id && moved.contains(id.as_str()) {
                            target.id.clone()
                        } else {
                            doc.id.clone()
                        },
                        id: id.clone(),
                        path: FieldPath::field(field),
                        value: Some(next),
                    });
                }
            }
        }
    }
    Ok(edits)
}

fn repair(value: &mut Value, moved: &BTreeSet<&str>, path: &str) {
    match value {
        Value::String(text) if text.starts_with('@') => {
            if let Ok(parts) = ara_schema::grammar::parse_ref(text)
                && let Some(id) = parts.id
                && moved.contains(id)
            {
                *text = format!("@{path}#{id}");
            }
        }
        Value::Array(values) => {
            for value in values {
                repair(value, moved, path);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                repair(value, moved, path);
            }
        }
        _ => {}
    }
}
