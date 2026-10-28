//! Distinguish measured Play authoring from its preceding native policy choices.
use super::*;

pub(super) fn scene(editor: &Editor) -> ara_ui::Result<Value> {
    let store = editor.host.store();
    let document = store
        .workspace()
        .doc(&editor.state.doc)
        .ok_or("Play fixture Scene disappeared")?;
    let text = ara_doc::write(document, store.schema());
    Ok(
        json!({"doc":document.id,"algorithm":"blake3-canonical-ara-doc-v1","bytes":text.len(),"digest":blake3::hash(text.as_bytes()).to_hex().to_string()}),
    )
}

fn policy(op: &ara_doc::Op) -> bool {
    use ara_host::project_policy::{PREFERENCES, PREFERENCES_ID};
    match op {
        ara_doc::Op::Set { doc, id, path, .. } => {
            matches!(doc.as_str(), "pj_project" | PREFERENCES_ID)
                && id.as_str() == "p_settings"
                && path.head() == "settings"
        }
        ara_doc::Op::Create { doc, id, record } => {
            doc.as_str() == PREFERENCES_ID
                && id.as_str() == "p_settings"
                && record.get("settings").is_some_and(Value::is_object)
                && record.len() == 1
        }
        ara_doc::Op::Document {
            doc,
            path,
            kind: Some(kind),
            ..
        } => doc.as_str() == PREFERENCES_ID && path == PREFERENCES && kind == "project",
        _ => false,
    }
}

impl PlayCheck {
    /// Native configuration may record policy edits, but it may never author the
    /// Scene or assets. Check that invariant before defining the Play baseline.
    pub(super) fn configured_baseline(&mut self, editor: &Editor) -> ara_ui::Result<()> {
        let after = scene(editor)?;
        let transactions = editor.host.store().since(self.initial);
        let policy_only = transactions
            .iter()
            .flat_map(|transaction| &transaction.ops)
            .all(policy);
        let scene_unchanged = after == self.scene_before;
        self.configuration = json!({"initialRevision":self.initial,"configuredRevision":editor.host.store().revision(),"policyOnly":policy_only,"sceneUnchanged":scene_unchanged,"sceneBefore":self.scene_before,"sceneConfigured":after,
            "transactions":transactions.iter().map(|transaction|json!({"txn":transaction.id,"author":transaction.author,"why":transaction.why,"ops":transaction.ops})).collect::<Vec<_>>()});
        if !policy_only || !scene_unchanged {
            return Err(
                "Native Play configuration changed the Scene or non-policy assets before Play"
                    .into(),
            );
        }
        self.before = editor.host.store().revision();
        Ok(())
    }
}
