//! Duplicate selected authored roots and apply final local poses in the same EditCtx transaction.
//! The viewport's provisional gesture never writes to the source documents.
use super::*;
use std::collections::BTreeSet;

/// Final local pose of one copied selection root.
#[derive(Schema, Deserialize)]
#[ara(params, name = "EditorDuplicateRootPose")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RootPose {
    /// Source root whose copy takes this pose.
    pub entity: EntityRef,
    /// Parent-local translation in metres.
    #[ara(unit=Meters)]
    pub position: [f32; 3],
    /// Parent-local XYZ Euler rotation in degrees.
    #[ara(unit=Degrees)]
    pub rotation: [f32; 3],
    /// Parent-local scale.
    pub scale: [f32; 3],
}

#[cfg(test)]
#[path = "duplicate_drag_tests.rs"]
mod tests;
/// Selected sources and optional final poses, used by Duplicate and Alt-drag alike.
#[derive(Schema, Deserialize)]
#[ara(params, name = "EditorDuplicateEntitiesParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Params {
    /// Authored entity or prefab-instance roots. Selected descendants follow their selected ancestor.
    pub entities: Vec<EntityRef>,
    /// Final poses of copied roots; omitted copies the sources without moving them.
    pub poses: Option<Vec<RootPose>>,
}
pub(super) struct DuplicateDrag;
impl Command for DuplicateDrag {
    type Params = Params;
    type Output = Value;
    const NAME: &'static str = "editor.entities.duplicate";
    const SUMMARY: &'static str = "Duplicate selected subtrees together, remap references between them, and optionally set copied root transforms in one undoable transaction.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"entities":["e_crate001"],"poses":[{"entity":"e_crate001","position":[2,0,3],"rotation":[0,0,0],"scale":[1,1,1]}]}),
        ]
    }
    fn run(cx: &mut EditCtx<'_, '_>, params: Params) -> Result<Value, Diagnostic> {
        let invalid = |message: &str| Diagnostic::error(ara_diag::codes::INVALID_PARAMS, message);
        if params.entities.is_empty() {
            return Err(invalid("Select an authored entity to duplicate"));
        }
        let selected: BTreeSet<_> = params
            .entities
            .iter()
            .map(|entity| entity.0.as_str())
            .collect();
        for entity in &selected {
            cx.read().target(entity)?;
            if entity.contains('/') {
                return Err(invalid(
                    "Duplicate the containing prefab instance to preserve its source link",
                ));
            }
        }
        let resolved = cx.read().resolved();
        let mut roots = Vec::new();
        for &entity in &selected {
            let mut parent = resolved
                .get(entity)
                .and_then(|entity| entity.parent.as_deref());
            let mut covered = false;
            let mut seen = BTreeSet::new();
            while let Some(id) = parent {
                if !seen.insert(id) {
                    return Err(invalid(
                        "Cannot duplicate a hierarchy containing a parent cycle",
                    ));
                }
                if selected.contains(id) {
                    covered = true;
                    break;
                }
                parent = resolved.get(id).and_then(|entity| entity.parent.as_deref());
            }
            if !covered {
                roots.push(entity);
            }
        }
        let mut poses = BTreeMap::new();
        for pose in params.poses.unwrap_or_default() {
            if !roots.contains(&pose.entity.0.as_str()) {
                return Err(invalid(
                    "Only a copied selection root can receive a final pose",
                ));
            }
            if !pose
                .position
                .into_iter()
                .chain(pose.rotation)
                .chain(pose.scale)
                .all(f32::is_finite)
            {
                return Err(invalid("Duplicate transforms must be finite"));
            }
            let entity = pose.entity.0.clone();
            if poses.insert(entity, pose).is_some() {
                return Err(invalid("Give each copied root at most one final pose"));
            }
        }
        let mut source = Vec::new();
        let mut included = BTreeSet::new();
        for root in &roots {
            for id in super::descendants(&resolved, root) {
                // Linked prefab content remains in its source document. Only outer
                // authored records, including attachments inside instances, are copied.
                if !id.contains('/')
                    && cx.read().doc().record(&id).is_some()
                    && included.insert(id.clone())
                {
                    source.push(id);
                }
            }
        }
        let mut ids = BTreeMap::new();
        for source in &source {
            ids.insert(source.clone(), cx.new_id(None, "e")?);
        }
        for old in &source {
            let mut record = cx
                .read()
                .doc()
                .record(old)
                .cloned()
                .ok_or_else(|| invalid("The source changed before duplication"))?;
            if let Some(parent) = record
                .get("parent")
                .and_then(Value::as_str)
                .and_then(|parent| super::remap_address(&ids, parent))
            {
                record.insert("parent".into(), parent.into());
            }
            super::remap(cx.read().schema(), &mut record, &ids);
            if roots.contains(&old.as_str()) {
                let name = record
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Entity");
                record.insert("name".into(), format!("{name} copy").into());
                let parent = ara_doc::parent_of(&record);
                let siblings = cx.read().doc().children(parent);
                let index = siblings
                    .iter()
                    .position(|id| id.as_str() == old)
                    .and_then(|index| u32::try_from(index + 1).ok());
                let order = cx.read().order_at(parent, index, None)?;
                record.insert("order".into(), order.to_string().into());
            }
            let id = ids
                .get(old)
                .ok_or_else(|| invalid("Missing allocated duplicate identity"))?;
            cx.create(id.clone(), record)?;
        }
        for (source, pose) in poses {
            let id = ids
                .get(&source)
                .ok_or_else(|| invalid("Missing copied root identity"))?;
            ara_cmd::builtin::TransformSet::run(
                cx,
                ara_cmd::builtin::TransformSetParams {
                    entities: vec![EntityRef(id.to_string())],
                    position: Some(pose.position),
                    rotation: Some(pose.rotation),
                    scale: Some(pose.scale),
                },
            )?;
        }
        let copied: Vec<_> = roots
            .iter()
            .filter_map(|root| ids.get(*root).map(ToString::to_string))
            .collect();
        Ok(json!({"roots":copied,"ids":ids,"count":source.len()}))
    }
}
