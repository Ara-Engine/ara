//! Exact hierarchy shape, pose and single native Undo/Redo assertions for tree drops.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum Action {
    Before,
    Inside,
    After,
    Root,
}
impl Action {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Before => "hierarchy-before-selected-roots-one-undo",
            Self::Inside => "hierarchy-inside-parent-child-filter-one-undo",
            Self::After => "hierarchy-after-parent-child-filter-one-undo",
            Self::Root => "hierarchy-blank-root-unparent-one-undo",
        }
    }
}

pub(super) fn prepare(action: Action, editor: &mut Editor) -> ara_ui::Result<()> {
    let mut commands: Vec<_> = IDS.iter().enumerate().map(|(index,id)| {
        json!({"cmd":"transform.set","args":{"entities":[id],"position":[index as f32*2.-4.,0.5,0.],"rotation":[0.,0.,0.],"scale":[1.,1.,1.]}})
    }).collect();
    // A selected child is intentionally first in selection order. The payload
    // must remove covered descendants rather than detach or move it twice.
    if !matches!(action, Action::Before) {
        commands.push(
            json!({"cmd":"entity.reparent","args":{"entity":IDS[1],"parent":IDS[0],"index":0}}),
        );
    }
    if matches!(action, Action::Inside | Action::Root) {
        commands.push(json!({"cmd":"transform.set","args":{"entities":[IDS[3]],"position":[3.,2.,1.],"rotation":[0.,35.,0.],"scale":[1.25,1.25,1.25]}}));
    }
    if matches!(action, Action::Root) {
        commands.push(
            json!({"cmd":"entity.reparent","args":{"entity":IDS[0],"parent":IDS[3],"index":0}}),
        );
        commands.push(
            json!({"cmd":"entity.reparent","args":{"entity":IDS[2],"parent":IDS[3],"index":1}}),
        );
    }
    editor.apply(
        commands,
        "Set up hierarchy placement and covered-child native fixture",
        None,
    )?;
    Ok(())
}

/// Canonical hash covers all Scene records; compact detailed evidence covers this selected set.
pub(super) fn snapshot(editor: &Editor) -> ara_ui::Result<Value> {
    let entities = crate::world::resolve(editor.host.store(), &editor.state.doc)
        .map_err(|diagnostics| format!("{diagnostics:?}"))?;
    let records = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .ok_or("Hierarchy proof Scene document disappeared")?
        .records();
    let canonical = ara_json::to_canonical_string(&json!(records));
    let worlds = crate::world::world_transforms(&entities);
    let mut roots: Vec<_> = entities
        .iter()
        .filter(|(id, entity)| IDS.contains(&id.as_str()) && entity.parent.is_none())
        .collect();
    roots.sort_by(|(a, ea), (b, eb)| ea.order.cmp(&eb.order).then(a.cmp(b)));
    let fixture: std::collections::BTreeMap<_,_> = IDS.iter().filter_map(|id| {
        let entity = entities.get(*id)?;
        let mut children: Vec<_> = entities.iter().filter(|(child,value)| IDS.contains(&child.as_str()) && value.parent.as_deref()==Some(*id)).collect();
        children.sort_by(|(a,ea),(b,eb)|ea.order.cmp(&eb.order).then(a.cmp(b)));
        Some((*id,json!({"record":entity.record,"parent":entity.parent,"children":children.into_iter().map(|(id,_)|id).collect::<Vec<_>>(),"world":worlds.get(*id).map(|matrix|matrix.to_cols_array())})))
    }).collect();
    Ok(
        json!({"revision":editor.host.store().revision(),"recordCount":records.len(),"documents":{"algorithm":"blake3","schema":"canonical-scene-records-utf8-v1","bytes":canonical.len(),"hash":blake3::hash(canonical.as_bytes()).to_hex().to_string()},"roots":roots.into_iter().map(|(id,_)|id).collect::<Vec<_>>(),"fixture":fixture}),
    )
}

pub(super) fn same_documents(a: &Value, b: &Value) -> bool {
    a["documents"].is_object() && a["documents"] == b["documents"]
}
fn poses_preserved(before: &Value, after: &Value) -> bool {
    IDS.into_iter().all(|id| {
        let Some(a) = before["fixture"][id]["world"].as_array() else {
            return false;
        };
        let Some(b) = after["fixture"][id]["world"].as_array() else {
            return false;
        };
        a.len() == 16
            && b.len() == 16
            && a.iter().zip(b).all(|(a, b)| {
                a.as_f64()
                    .zip(b.as_f64())
                    .is_some_and(|(a, b)| a.is_finite() && b.is_finite() && (a - b).abs() < 0.0005)
            })
    })
}
pub(super) fn observe(action: Action, proof: &Value, preview: Option<&Value>) -> (bool, Value) {
    let before = &proof["before"];
    let after = &proof["after"];
    let undo = &proof["undo"];
    let redo = &proof["redo"];
    let expected_roots = match action {
        Action::Before => vec![IDS[1], IDS[2], IDS[0], IDS[3], IDS[4]],
        Action::Inside => vec![IDS[3], IDS[4]],
        Action::After | Action::Root => vec![IDS[3], IDS[4], IDS[0], IDS[2]],
    };
    // "After D" precedes E, while root-background drop appends to all roots.
    let expected_roots = if matches!(action, Action::After) {
        vec![IDS[3], IDS[0], IDS[2], IDS[4]]
    } else {
        expected_roots
    };
    let placement = match action {
        Action::Before => "Before",
        Action::After => "After",
        _ => "Inside",
    };
    let valid_preview = preview.is_some_and(|value| {
        value["feedback"]["valid"] == true
            && value["active"] == true
            && value["feedback"]["target"]["kind"] == "hierarchy"
            && value["feedback"]["target"]["placement"] == placement
            && value["feedback"]["target"]["entity"]
                == match action {
                    Action::Before => json!(IDS[0]),
                    Action::Root => Value::Null,
                    _ => json!(IDS[3]),
                }
    });
    let hierarchy = after["roots"] == json!(expected_roots)
        && (matches!(action, Action::Before) || after["fixture"][IDS[1]]["parent"] == IDS[0])
        && match action {
            Action::Inside => {
                after["fixture"][IDS[3]]["children"] == json!([IDS[0], IDS[2]])
                    && after["fixture"][IDS[0]]["parent"] == IDS[3]
                    && after["fixture"][IDS[2]]["parent"] == IDS[3]
            }
            _ => [IDS[0], IDS[2]]
                .into_iter()
                .all(|id| after["fixture"][id]["parent"].is_null()),
        };
    let revision = before["revision"].as_u64();
    let checks = json!({"exactHierarchyAndOrder":hierarchy,"worldPosesPreserved":poses_preserved(before,after),"validPlacementPreview":valid_preview,
        "previewHasNoOps":same_documents(before,&proof["preview"])&&before["revision"]==proof["preview"]["revision"],
        "noDuplicateDescendants":before["recordCount"]==after["recordCount"],
        "coveredChildRecordUntouched":matches!(action,Action::Before)||before["fixture"][IDS[1]]["record"]==after["fixture"][IDS[1]]["record"],
        "oneReleaseTransaction":revision.is_some_and(|revision|after["revision"]==revision+1),
        "oneNativeUndoRestoresAllRecords":same_documents(before,undo) && revision.is_some_and(|revision|undo["revision"]==revision+2),
        "oneNativeRedoRestoresAllRecords":same_documents(after,redo) && revision.is_some_and(|revision|redo["revision"]==revision+3)});
    let ok = checks
        .as_object()
        .is_some_and(|checks| checks.values().all(|value| value == true));
    (
        ok,
        json!({"checks":checks,"expectedRoots":expected_roots,"expectedPlacement":placement,"snapshots":proof,"preview":preview,"ingress":"SDL pointer press/motion/release then Ctrl-Z/Ctrl-Y through production router"}),
    )
}
