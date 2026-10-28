use super::*;

fn component(editor: &Editor, doc: &str, entity: &str, name: &str, property: &str) -> Value {
    editor
        .host
        .store()
        .workspace()
        .doc(doc)
        .and_then(|doc| doc.record(entity))
        .and_then(|record| record.get(name))
        .and_then(|value| value.get(property))
        .cloned()
        .unwrap_or(Value::Null)
}
fn scale(value: &Value) -> Value {
    value
        .get("Transform")
        .and_then(|value| value.get("scale"))
        .cloned()
        .unwrap_or_else(|| json!([1, 1, 1]))
}
fn source_scale(editor: &Editor, doc: &str, entity: &str) -> Value {
    let value = component(editor, doc, entity, "Transform", "scale");
    if value.is_null() {
        json!([1, 1, 1])
    } else {
        value
    }
}
impl PrefabCheck {
    pub(super) fn observe_commit(&self, act: Act, editor: &Editor) -> ara_ui::Result<Value> {
        let store = editor.host.store();
        if act == Act::FailedApply {
            let unchanged = fixture::snapshot(editor) == self.before;
            return Ok(
                json!({"ok":unchanged&&store.revision()==self.before_revision&&store.history().len()==self.before_history,
                "allDocumentsUnchanged":unchanged,"historyUnchanged":store.history().len()==self.before_history}),
            );
        }
        let transactions = &store.history()[self.before_history.min(store.history().len())..];
        let one = transactions.len() == 1 && store.revision() == self.before_revision + 1;
        let touched: std::collections::BTreeSet<_> = transactions
            .iter()
            .flat_map(|txn| txn.ops.iter())
            .map(|op| op.doc().as_str())
            .collect();
        let rows = crate::world::resolve(store, SCENE)
            .map_err(|diagnostics| format!("J5 resolve: {diagnostics:?}"))?;
        let scene = store.workspace().doc(SCENE).ok_or("J5 scene missing")?;
        let (expected_source, effect) = match act {
            Act::ApplyUndo => {
                let source = source_scale(editor, BASE, &self.base_root);
                let second = rows.get(SECOND).map(|entity| scale(&json!(entity.record)));
                let cleared = scene
                    .record(FIRST)
                    .and_then(|record| record.get("overrides"))
                    .is_none_or(|overrides| overrides.get("Transform.scale").is_none());
                (
                    BASE.to_owned(),
                    json!({"ok":source==json!([2,2,2])&&second==Some(json!([2,2,2]))&&cleared,
                    "sourceScale":source,"secondInstanceScale":second,"instanceOverrideCleared":cleared}),
                )
            }
            Act::NestedNearest | Act::NestedExplicit => {
                let nearest = act == Act::NestedNearest;
                let source = if nearest { OUTER } else { BASE };
                let base = source_scale(editor, BASE, &self.base_child);
                let parts: Vec<_> = self.nested.split('/').collect();
                let outer_value = parts
                    .get(1)
                    .and_then(|id| store.workspace().doc(OUTER)?.record(id))
                    .and_then(|record| record.get("overrides"))
                    .and_then(|overrides| {
                        overrides.get(format!("{}/Transform.scale", self.base_child).as_str())
                    })
                    .cloned()
                    .unwrap_or(Value::Null);
                let selected = rows
                    .get(&self.nested)
                    .map(|entity| scale(&json!(entity.record)));
                let source_ok = if nearest {
                    base == json!([1, 1, 1]) && outer_value == json!([3, 3, 3])
                } else {
                    base == json!([3, 3, 3]) && outer_value.is_null()
                };
                (
                    source.into(),
                    json!({"ok":source_ok&&selected==Some(json!([3,3,3])),"baseScale":base,"outerOverride":outer_value,"selectedScale":selected,"nearestBoundary":nearest}),
                )
            }
            Act::VariantCreate => {
                let instance = scene
                    .record(VARIANT_ENTITY)
                    .ok_or("J5 variant instance missing")?;
                let reference = instance
                    .get("prefab")
                    .and_then(Value::as_str)
                    .ok_or("J5 created variant reference missing")?;
                let asset = store
                    .workspace()
                    .find(reference)
                    .ok_or("J5 variant document missing")?;
                let source = store
                    .workspace()
                    .doc(asset.as_str())
                    .ok_or("J5 variant source missing")?;
                let root = source
                    .children(None)
                    .first()
                    .and_then(|id| source.record(id.as_str()));
                let linked = root
                    .and_then(|record| record.get("prefab"))
                    .and_then(Value::as_str)
                    == Some("@prefabs/j5-base.prefab#pf_j5base");
                let instance_scale = rows
                    .get(VARIANT_ENTITY)
                    .map(|entity| scale(&json!(entity.record)));
                let source_added = transactions.iter().flat_map(|txn| txn.ops.iter()).any(
                    |op| matches!(op,ara_doc::Op::Document{doc,kind:Some(_),..} if doc==asset),
                );
                (
                    asset.to_string(),
                    json!({"ok":linked&&source_added&&instance_scale==Some(json!([4,4,4])),"baseLinkPreserved":linked,"newDocumentLifecycleInTransaction":source_added,"instanceScale":instance_scale,"reference":reference}),
                )
            }
            Act::VariantApply => {
                let source = store
                    .workspace()
                    .doc(VARIANT)
                    .ok_or("J5 variant source missing")?;
                let root = source
                    .children(None)
                    .first()
                    .and_then(|id| source.record(id.as_str()));
                let linked = root
                    .and_then(|record| record.get("prefab"))
                    .and_then(Value::as_str)
                    == Some("@prefabs/j5-base.prefab#pf_j5base");
                let authored = root
                    .and_then(|record| record.get("overrides"))
                    .and_then(|overrides| overrides.get("Transform.scale"))
                    .cloned();
                let base = source_scale(editor, BASE, &self.base_root);
                let inherited = rows
                    .get(VARIANT_ENTITY)
                    .and_then(|entity| entity.record.get("MeshRenderer"))
                    .and_then(|value| value.get("size"))
                    .cloned();
                (
                    VARIANT.into(),
                    json!({"ok":linked&&authored==Some(json!([6,6,6]))&&base==json!([1,1,1])&&inherited==Some(json!([2,3,4])),
                    "baseLinkPreserved":linked,"variantScale":authored,"baseScale":base,"laterBaseSizeInherited":inherited}),
                )
            }
            Act::FailedApply => return Err("J5 failure observation reached a commit branch".into()),
        };
        let source_and_scene = touched.contains(SCENE)
            && touched.contains(expected_source.as_str())
            && (expected_source == BASE || !touched.contains(BASE));
        Ok(
            json!({"ok":one&&source_and_scene&&effect["ok"]==true,"oneTransaction":one,"sourceAndSceneInSameTransaction":source_and_scene,
            "expectedSource":expected_source,"touchedDocuments":touched,"effect":effect,
            "transactions":transactions.iter().map(|txn|json!({"id":txn.id,"author":txn.author,"why":txn.why,"ops":txn.ops})).collect::<Vec<_>>()}),
        )
    }
}
