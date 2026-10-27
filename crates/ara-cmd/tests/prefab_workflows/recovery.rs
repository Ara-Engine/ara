use crate::common::*;
use ara_doc::{Workspace, read};
use ara_ops::{Author, Store, Transaction};
use serde_json::json;
use std::collections::BTreeSet;

/// Re-read canonical files and the real journal entry with either document flush
/// interrupted. Recovery must converge to the complete action and retain one undo.
fn assert_recovery(command: &str, records: serde_json::Value, params: serde_json::Value) {
    let (mut store, registry) = setup(records);
    let before = snapshot(&store);
    let original_workspace = store.workspace().clone();
    run(&mut store, &registry, "sc_level", command, params);
    let after = snapshot(&store);
    let journal = serde_json::to_vec(&store.history()[0]).unwrap();
    for written in ["none", "source", "scene", "both"] {
        let mut files = original_workspace.clone();
        let mut pending = BTreeSet::new();
        for (path, doc) in store.workspace().docs() {
            if doc.id.as_str() != "sc_level"
                && !store.history()[0].ops.iter().any(|op| op.doc() == &doc.id)
            {
                continue;
            }
            let saved = written == "both"
                || if doc.id.as_str() == "sc_level" {
                    written == "scene"
                } else {
                    written == "source"
                };
            if saved {
                files.insert(path, doc.clone());
            } else {
                pending.insert(doc.id.clone());
            }
        }
        let mut loaded = Workspace::new(files.index().clone());
        for (path, doc) in files.docs() {
            let canonical = ara_doc::write(doc, store.schema());
            // An interrupted source flush can temporarily leave an absent prefab;
            // defer cross-document reference checks until its journal is restored.
            let parsed = read(&canonical, path, store.schema(), None);
            assert!(
                !parsed.diags.iter().any(|d| d.is_error()),
                "{command}/{written}: {:#?}",
                parsed.diags
            );
            loaded.insert(path, parsed.doc.unwrap());
        }
        let mut recovered = Store::new(store.schema().clone(), loaded);
        let txn: Transaction = serde_json::from_slice(&journal).unwrap();
        recovered.restore_documents(txn, &pending).unwrap();
        assert_eq!(snapshot(&recovered), after, "{command}/{written}");
        assert_eq!(
            resolved(&recovered),
            resolved(&store),
            "{command}/{written}"
        );
        assert_eq!(recovered.history().len(), 1);
        recovered.undo_last(&Author::Human).unwrap();
        assert_eq!(snapshot(&recovered), before, "undo {command}/{written}");
        assert!(recovered.workspace().doc("pf_created").is_none());
    }
}

#[test]
fn interrupted_create_flush_recovers_both_documents_and_one_undo() {
    assert_recovery(
        "prefab.create",
        json!({
            "e_root":{"Transform":{"position":[2,3,4]},"Probe":{"target":"e_child"}},
            "e_child":{"parent":"e_root","Transform":{}},
            "e_external":{"Probe":{"target":"e_child"}}
        }),
        json!({"entity":"e_root","path":"created.prefab","id":"pf_created"}),
    );
}

#[test]
fn interrupted_apply_flush_recovers_source_instance_and_one_undo() {
    assert_recovery("prefab.apply", instances(), json!({"entity":"e_first"}));
}
