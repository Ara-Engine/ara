use crate::common::*;
use ara_cmd::{Batch, Session};
use ara_diag::{Diagnostic, codes};
use ara_doc::{Op, RecordId};
use ara_ops::{Guard, TxnMeta};
use serde_json::json;

#[derive(Debug)]
struct VetoCompleteCreate;
impl Guard for VetoCompleteCreate {
    fn check(&self, _: &TxnMeta, _: Option<&str>, ops: &[Op]) -> Vec<Diagnostic> {
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Document { kind: Some(_), .. }))
        );
        assert!(
            ops.iter()
                .filter(|op| matches!(op, Op::Create { doc, .. } if doc.as_str() == "pf_created"))
                .count()
                >= 2
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Delete { doc, .. } if doc.as_str() == "sc_level"))
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Create { doc, .. } if doc.as_str() == "sc_level"))
        );
        vec![Diagnostic::error(
            codes::INVALID_PARAMS,
            "Test commit veto after every prefab phase",
        )]
    }
}

fn authoring() -> serde_json::Value {
    json!({
        "e_root":{"Transform":{},"Probe":{"target":"e_child"}},
        "e_child":{"parent":"e_root","Transform":{}},
        "e_external":{"Probe":{"target":"e_child"}}
    })
}

#[test]
fn create_failures_rollback_lifecycle_population_references_and_replacement() {
    for phase in ["path", "commit", "following call", "source validation"] {
        let (mut store, registry) = setup(authoring());
        if phase == "source validation" {
            let mut scene = store.workspace().doc("sc_level").unwrap().clone();
            let mut child = scene.record("e_child").unwrap().clone();
            child.insert("Probe".into(), json!({"value":"invalid imported value"}));
            scene.insert_unchecked(RecordId::parse("e_child").unwrap(), child);
            store.load_doc("level.scene", scene);
        }
        let before = snapshot(&store);
        if phase == "commit" {
            store.set_guard(Some(Box::new(VetoCompleteCreate)));
        }
        let mut batch = Batch::new(
            "sc_level",
            [(
                "prefab.create",
                json!({
                    "entity":"e_root","path":if phase == "path" {"base.prefab"} else {"created.prefab"},"id":"pf_created"
                }),
            )],
        );
        if phase == "following call" {
            batch.calls.push(ara_cmd::Call {
                command: "property.set".into(),
                params: json!({
                    "entity":"e_missing","path":"name","value":"fail after replacement"
                }),
            });
        }
        let error = registry
            .apply(&mut store, &batch, &Session::human())
            .unwrap_err();
        assert!(!error.is_empty(), "{phase}");
        assert_eq!(snapshot(&store), before, "{phase}");
        assert_eq!(store.revision(), 0, "{phase}");
        assert!(store.history().is_empty(), "{phase}");
        assert!(store.workspace().doc("pf_created").is_none(), "{phase}");
        store.set_guard(None);
        if phase != "source validation" {
            run(
                &mut store,
                &registry,
                "sc_level",
                "prefab.create",
                json!({"entity":"e_root","path":"created.prefab","id":"pf_created"}),
            );
            let (mut fresh, registry) = setup(authoring());
            run(
                &mut fresh,
                &registry,
                "sc_level",
                "prefab.create",
                json!({"entity":"e_root","path":"created.prefab","id":"pf_created"}),
            );
            assert_eq!(
                snapshot(&store),
                snapshot(&fresh),
                "failed {phase} must restore the ID generator"
            );
        }
    }
}

#[test]
fn a_late_invalid_override_rolls_back_an_earlier_source_write() {
    let (mut store, registry) = setup(instances());
    let mut scene = store.workspace().doc("sc_level").unwrap().clone();
    let mut instance = scene.record("e_first").unwrap().clone();
    instance.insert(
        "overrides".into(),
        json!({"Probe.label":"changed first","Probe.value":"bad number"}),
    );
    scene.insert_unchecked(RecordId::parse("e_first").unwrap(), instance);
    store.load_doc("level.scene", scene);
    let before = snapshot(&store);
    let error = registry
        .apply(
            &mut store,
            &Batch::new("sc_level", [("prefab.apply", json!({"entity":"e_first"}))]),
            &Session::human(),
        )
        .unwrap_err();
    assert!(error.iter().any(|d| d.message.contains("number")));
    assert_eq!(snapshot(&store), before);
    assert_eq!(store.revision(), 0);
    assert!(store.history().is_empty());
}
