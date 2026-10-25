//! Transactions: atomicity, revisions, idempotency, undo/redo and the op stream.

use ara_diag::codes;
use ara_doc::{AssetId, AssetIndex, Document, Edit, IdGen, RecordId, Workspace, read, write};
use ara_ops::{Author, Commit, Store, TxnMeta};
use ara_schema::SchemaDb;
use proptest::prelude::*;
use proptest::sample::Index;
use serde_json::{Value, json};

const SCENE: &str = r#"{
  "$schema": "ara://schema/scene/1",
  "id": "sc_test0001",
  "records": {
    "e_a": { "name": "A", "order": "a0", "Transform": {} },
    "e_b": { "name": "B", "parent": "e_a", "order": "a0" }
  }
}"#;

fn store() -> Store {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    let doc = read(SCENE, "test.scene", &db, None)
        .doc
        .expect("fixture reads");
    let mut ws = Workspace::new(AssetIndex::new());
    ws.insert("test.scene", doc);
    Store::with_ids(db, ws, IdGen::seeded(1))
}

fn doc_id() -> AssetId {
    AssetId::parse("sc_test0001").expect("id")
}

fn rid(s: &str) -> RecordId {
    RecordId::parse(s).expect("record id")
}

fn set(id: &str, path: &str, value: Value) -> Edit {
    Edit::Set {
        doc: doc_id(),
        id: rid(id),
        path: path.parse().expect("path"),
        value: Some(value),
    }
}

fn agent() -> Author {
    Author::Agent("claude".into())
}

fn text(s: &Store) -> String {
    write(s.workspace().doc("sc_test0001").expect("doc"), s.schema())
}

fn get(s: &Store, id: &str, key: &str) -> Value {
    s.workspace()
        .doc("sc_test0001")
        .and_then(|d| d.record(id))
        .and_then(|r| r.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
#[allow(clippy::unwrap_used)]
fn document_creation_rejects_path_aliases_and_rolls_back_the_batch() {
    for path in [
        "levels/./new.scene",
        "levels//new.scene",
        "./new.scene",
        "levels/../new.scene",
    ] {
        let mut s = store();
        let edits = [
            Edit::Document {
                doc: AssetId::parse("sc_first").unwrap(),
                path: "levels/new.scene".into(),
                kind: Some("scene".into()),
            },
            Edit::Document {
                doc: AssetId::parse("sc_alias").unwrap(),
                path: path.into(),
                kind: Some("scene".into()),
            },
        ];
        assert!(
            s.transact(TxnMeta::by(Author::Human), edits).is_err(),
            "{path}"
        );
        assert_eq!(s.workspace().docs().count(), 1, "creation rolled back");
        assert_eq!(s.revision(), 0);
    }
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let mut index = AssetIndex::new();
    index.insert("sc_reserved", "levels/unloaded.scene", Some("scene".into()));
    let mut s = Store::new(db, Workspace::new(index));
    assert!(
        s.transact(
            TxnMeta::by(Author::Human),
            [Edit::Document {
                doc: AssetId::parse("sc_reserved").unwrap(),
                path: "levels/other.scene".into(),
                kind: Some("scene".into()),
            }]
        )
        .is_err(),
        "indexed identities remain reserved even when their document is not loaded"
    );
    #[cfg(windows)]
    {
        let mut s = store();
        let edits = [
            Edit::Document {
                doc: AssetId::parse("sc_first").unwrap(),
                path: "levels/new.scene".into(),
                kind: Some("scene".into()),
            },
            Edit::Document {
                doc: AssetId::parse("sc_alias").unwrap(),
                path: "levels/NEW.scene".into(),
                kind: Some("scene".into()),
            },
        ];
        assert!(s.transact(TxnMeta::by(Author::Human), edits).is_err());
        assert_eq!(s.workspace().docs().count(), 1);
    }
}

#[test]
fn source_path_loading_preserves_transaction_and_undo_save_paths() {
    let mut s = store();
    let doc = s
        .workspace()
        .doc("sc_test0001")
        .expect("valid fixture")
        .clone();
    s.load_doc_with_asset_path("settings.json", doc, "source.glb");
    assert_eq!(s.revision(), 0);
    assert!(s.history().is_empty());
    s.transact(
        TxnMeta::by(Author::Human),
        [set("e_a", "name", json!("Edited"))],
    )
    .expect("valid fixture");
    assert_eq!(get(&s, "e_a", "name"), json!("Edited"));
    s.undo_last(&Author::Human).expect("valid fixture");
    assert_eq!(get(&s, "e_a", "name"), json!("A"));
    assert_eq!(s.workspace().path_of("sc_test0001"), Some("settings.json"));
    assert_eq!(
        s.workspace().index().path_of("sc_test0001"),
        Some("source.glb")
    );

    let doc = s
        .workspace()
        .doc("sc_test0001")
        .expect("valid fixture")
        .clone();
    s.load_doc("ordinary.scene", doc);
    assert_eq!(s.workspace().path_of("sc_test0001"), Some("ordinary.scene"));
    assert_eq!(
        s.workspace().index().path_of("sc_test0001"),
        Some("ordinary.scene")
    );
}

#[test]
fn transactions_are_all_or_nothing() {
    let mut s = store();
    let before = text(&s);
    // The rename is fine; deleting e_a strands its child e_b, so the whole thing fails.
    let err = s
        .transact(
            TxnMeta::by(agent()),
            [
                set("e_a", "name", json!("Renamed")),
                Edit::Delete {
                    doc: doc_id(),
                    id: rid("e_a"),
                },
            ],
        )
        .expect_err("dangling child");
    assert_eq!(err[0].code, codes::MISSING_PARENT);
    assert_eq!(text(&s), before);
    assert_eq!(s.revision(), 0);

    // A bad edit fails immediately and leaves the transaction open for the caller.
    let mut staged = s.begin(TxnMeta::by(Author::Human));
    staged.apply(set("e_a", "name", json!("X"))).expect("valid");
    assert_eq!(
        staged
            .apply(set("e_a", "Transform.position", json!("up")))
            .expect_err("type")
            .code,
        codes::TYPE_MISMATCH
    );
    let diff = staged.rollback();
    assert_eq!(diff.len(), 1);
    assert_eq!(text(&s), before);
}

#[test]
fn records_author_why_and_revisions() {
    let mut s = store();
    let rx = s.subscribe();
    let meta = TxnMeta {
        why: Some("Make room".into()),
        label: Some("property.set".into()),
        ..TxnMeta::by(agent())
    };
    let t = s
        .transact(meta, [set("e_a", "Transform.position.y", json!(2))])
        .expect("commit")
        .txn()
        .cloned()
        .expect("txn");
    assert_eq!(
        (t.author.to_string(), t.why.as_deref(), t.base, t.result),
        ("agent:claude".into(), Some("Make room"), 0, 1)
    );
    assert_eq!(rx.try_recv().expect("streamed"), t);
    assert_eq!(s.since(0).len(), 1);
    assert!(s.since(1).is_empty());
    let json = serde_json::to_value(&t).expect("serializes");
    // `prev` is the effective value the edit replaced (the omitted default here).
    assert_eq!(
        json["ops"][0],
        json!({ "op": "set", "doc": "sc_test0001", "id": "e_a", "path": "Transform.position.y", "value": 2, "prev": 0 })
    );
}

#[test]
fn idempotency_keys_replay() {
    let mut s = store();
    let meta = TxnMeta {
        idempotency_key: Some("turn-7".into()),
        ..TxnMeta::by(agent())
    };
    let first = s
        .transact(meta.clone(), [set("e_a", "name", json!("One"))])
        .expect("commit")
        .txn()
        .cloned()
        .expect("txn");
    let again = s
        .transact(meta, [set("e_a", "name", json!("Two"))])
        .expect("replay");
    assert!(matches!(again, Commit::Replayed(t) if t.id == first.id));
    assert_eq!(get(&s, "e_a", "name"), json!("One"));
    assert_eq!(s.revision(), 1);
}

#[test]
fn stale_bases_conflict_only_on_overlapping_paths() {
    let mut s = store();
    s.transact(
        TxnMeta::by(Author::Human),
        [set("e_a", "Transform.position", json!([1, 0, 0]))],
    )
    .expect("commit");
    let stale = TxnMeta {
        base: Some(0),
        ..TxnMeta::by(agent())
    };
    let err = s
        .transact(
            stale.clone(),
            [set("e_a", "Transform.position.x", json!(5))],
        )
        .expect_err("overlap");
    assert_eq!(err[0].code, codes::STALE_BASE);
    s.transact(stale, [set("e_a", "Transform.scale", json!([2, 2, 2]))])
        .expect("disjoint path");
}

#[test]
fn undo_detects_later_edits_to_the_same_path() {
    let mut s = store();
    let t1 = s
        .transact(TxnMeta::by(agent()), [set("e_a", "name", json!("Agent"))])
        .expect("commit")
        .txn()
        .cloned()
        .expect("txn");
    s.transact(
        TxnMeta::by(Author::Human),
        [set("e_b", "name", json!("Other"))],
    )
    .expect("commit");
    let t3 = s
        .transact(
            TxnMeta::by(Author::Human),
            [set("e_a", "name", json!("Human"))],
        )
        .expect("commit")
        .txn()
        .cloned()
        .expect("txn");

    let err = s.undo(&t1.id, Author::Human).expect_err("conflict");
    assert_eq!(err[0].code, codes::UNDO_CONFLICT);
    assert_eq!(get(&s, "e_a", "name"), json!("Human"));

    // Once the human's edit is undone, the pair cancels out and the agent's turn can go.
    s.undo(&t3.id, Author::Human).expect("undo human");
    let u1 = s.undo(&t1.id, Author::Human).expect("undo agent").clone();
    assert_eq!(u1.reverts.as_ref(), Some(&t1.id));
    assert_eq!(get(&s, "e_a", "name"), json!("A"));
    assert_eq!(
        s.undo(&t1.id, Author::Human).expect_err("twice")[0].code,
        codes::ALREADY_REVERTED
    );
}

#[test]
fn undo_and_redo_stacks_are_per_author() {
    let mut s = store();
    s.transact(TxnMeta::by(agent()), [set("e_a", "name", json!("1"))])
        .expect("commit");
    s.transact(TxnMeta::by(Author::Human), [set("e_b", "name", json!("H"))])
        .expect("commit");
    s.transact(TxnMeta::by(agent()), [set("e_a", "name", json!("2"))])
        .expect("commit");

    s.undo_last(&agent()).expect("undo 2");
    assert_eq!(get(&s, "e_a", "name"), json!("1"));
    s.undo_last(&agent()).expect("undo 1");
    assert_eq!(get(&s, "e_a", "name"), json!("A"));
    assert_eq!(
        s.undo_last(&agent()).expect_err("empty")[0].code,
        codes::NOTHING_TO_UNDO
    );
    assert_eq!(get(&s, "e_b", "name"), json!("H"));

    s.redo_last(&agent()).expect("redo 1");
    s.redo_last(&agent()).expect("redo 2");
    assert_eq!(get(&s, "e_a", "name"), json!("2"));
    s.undo_last(&agent()).expect("undo the redo");
    assert_eq!(get(&s, "e_a", "name"), json!("1"));

    // A new edit clears the redo stack.
    s.transact(TxnMeta::by(agent()), [set("e_b", "name", json!("new"))])
        .expect("commit");
    assert_eq!(
        s.redo_last(&agent()).expect_err("cleared")[0].code,
        codes::NOTHING_TO_UNDO
    );
}

#[derive(Debug, Clone)]
enum Choice {
    Name(Index, u8),
    Position(Index, i8),
    Create(Index),
    DeleteLeaf(Index),
    Reparent(Index, Option<Index>),
}

fn choice() -> impl Strategy<Value = Choice> {
    prop_oneof![
        (any::<Index>(), any::<u8>()).prop_map(|(i, n)| Choice::Name(i, n)),
        (any::<Index>(), any::<i8>()).prop_map(|(i, v)| Choice::Position(i, v)),
        any::<Index>().prop_map(Choice::Create),
        any::<Index>().prop_map(Choice::DeleteLeaf),
        (any::<Index>(), prop::option::of(any::<Index>()))
            .prop_map(|(i, p)| Choice::Reparent(i, p)),
    ]
}

fn edits(doc: &Document, c: &Choice, n: &mut u32) -> Vec<Edit> {
    let ids: Vec<RecordId> = doc.records().keys().cloned().collect();
    if ids.is_empty() {
        *n += 1;
        return vec![Edit::Create {
            doc: doc_id(),
            id: rid(&format!("e_n{n}")),
            record: serde_json::Map::new(),
        }];
    }
    let pick = |i: &Index| ids[i.index(ids.len())].clone();
    match c {
        Choice::Name(i, v) => vec![set(pick(i).as_str(), "name", json!(format!("n{v}")))],
        Choice::Position(i, v) => vec![Edit::Set {
            doc: doc_id(),
            id: pick(i),
            path: "Transform".parse().expect("path"),
            value: Some(json!({ "position": [v, 0, 0] })),
        }],
        Choice::Create(parent) => {
            *n += 1;
            let mut record = serde_json::Map::new();
            record.insert("parent".into(), json!(pick(parent).as_str()));
            vec![Edit::Create {
                doc: doc_id(),
                id: rid(&format!("e_n{n}")),
                record,
            }]
        }
        Choice::DeleteLeaf(i) => vec![Edit::Delete {
            doc: doc_id(),
            id: pick(i),
        }],
        Choice::Reparent(i, p) => vec![Edit::Move {
            doc: doc_id(),
            id: pick(i),
            parent: p.as_ref().map(|p| pick(p).to_string()),
            order: None,
        }],
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Undoing every committed transaction restores the start; redoing them all
    /// restores the end.
    #[test]
    fn undo_all_then_redo_all(choices in prop::collection::vec(choice(), 1..25)) {
        let mut s = store();
        let start = text(&s);
        let mut n = 0;
        for c in &choices {
            let doc = s.workspace().doc("sc_test0001").expect("doc").clone();
            // Invalid transactions (cycles, dangling parents) are rejected; that's fine.
            let _ = s.transact(TxnMeta::by(Author::Human), edits(&doc, c, &mut n));
        }
        let end = text(&s);
        let mut undone = 0;
        while s.undo_last(&Author::Human).is_ok() {
            undone += 1;
        }
        prop_assert_eq!(text(&s), start);
        for _ in 0..undone {
            s.redo_last(&Author::Human).expect("redo");
        }
        prop_assert_eq!(text(&s), end);
    }
}

#[test]
fn restore_rebuilds_history_with_ids_and_optionally_reapplies() {
    let mut s = store();
    s.transact(TxnMeta::by(agent()), [set("e_a", "name", json!("One"))])
        .expect("first");
    s.transact(TxnMeta::by(agent()), [set("e_b", "name", json!("Two"))])
        .expect("second");
    let journal = s.history().to_vec();

    // A restart: the disk has the first change but not the second.
    let mut fresh = store();
    fresh
        .transact(
            TxnMeta::by(Author::Human),
            [set("e_a", "name", json!("One"))],
        )
        .expect("disk state");
    let mut fresh = {
        let ws = fresh.workspace().clone();
        Store::with_ids(fresh.schema().clone(), ws, IdGen::seeded(9))
    };
    fresh.restore(journal[0].clone(), false).expect("on disk");
    fresh.restore(journal[1].clone(), true).expect("lost");
    assert_eq!(fresh.revision(), 2);
    assert_eq!(get(&fresh, "e_b", "name"), json!("Two"));
    assert_eq!(fresh.txn(&journal[0].id), Some(&journal[0]));
    assert_eq!(
        fresh
            .restore(journal[1].clone(), true)
            .expect_err("not past")
            .remove(0)
            .code,
        codes::STALE_BASE
    );

    // Restored transactions are undoable, newest first.
    let undo = fresh.undo_last(&agent()).expect("undo");
    assert_eq!(undo.reverts.as_ref(), Some(&journal[1].id));
    assert_eq!(get(&fresh, "e_b", "name"), json!("B"));
}

/// Blocks agent ops on `e_b` unless the committing origin is `ses_owner`.
#[derive(Debug)]
struct OwnsB;

impl ara_ops::Guard for OwnsB {
    fn check(
        &self,
        meta: &TxnMeta,
        origin: Option<&str>,
        ops: &[ara_doc::Op],
    ) -> Vec<ara_diag::Diagnostic> {
        let agent = matches!(meta.author, Author::Agent(_));
        let touches_b = ops.iter().any(|op| op.id().as_str() == "e_b");
        if agent && touches_b && origin != Some("ses_owner") {
            vec![ara_diag::Diagnostic::error(
                codes::INVALID_PARAMS,
                "e_b is claimed",
            )]
        } else {
            Vec::new()
        }
    }
}

#[test]
fn guards_veto_commits_and_dry_runs_by_origin() {
    let mut s = store();
    s.set_guard(Some(Box::new(OwnsB)));
    let before = text(&s);
    let err = s
        .transact(TxnMeta::by(agent()), [set("e_b", "name", json!("X"))])
        .expect_err("guarded");
    assert_eq!(err[0].message, "e_b is claimed");
    assert_eq!(text(&s), before);

    // Dry runs see the same verdict.
    let mut staged = s.begin(TxnMeta::by(agent()));
    staged
        .apply(set("e_b", "name", json!("X")))
        .expect("applies");
    assert_eq!(staged.check().len(), 1);
    staged.rollback();

    // Humans, other records and the owning origin pass.
    s.transact(TxnMeta::by(Author::Human), [set("e_b", "name", json!("H"))])
        .expect("human");
    s.transact(TxnMeta::by(agent()), [set("e_a", "name", json!("A"))])
        .expect("other record");
    s.set_origin(Some("ses_owner".into()));
    assert_eq!(s.origin(), Some("ses_owner"));
    s.transact(TxnMeta::by(agent()), [set("e_b", "name", json!("O"))])
        .expect("owner");
    s.set_guard(None);
    s.set_origin(None);
    s.transact(TxnMeta::by(agent()), [set("e_b", "name", json!("Free"))])
        .expect("no guard");
}

#[test]
fn rebased_undo_skips_what_others_changed_since() {
    let mut s = store();
    let mia = Author::Agent("mia".into());
    let sam = Author::Agent("sam".into());
    let mine = s
        .transact(
            TxnMeta::by(mia.clone()),
            [
                set("e_a", "name", json!("Mine")),
                set("e_a", "Transform.position", json!([1, 2, 3])),
            ],
        )
        .expect("commit")
        .txn()
        .cloned()
        .expect("txn");
    // Sam changes the position afterwards; the name stays Mia's.
    s.transact(
        TxnMeta::by(sam.clone()),
        [set("e_a", "Transform.position.x", json!(9))],
    )
    .expect("commit");
    let foreign = |t: &ara_ops::Transaction| t.author != Author::Agent("mia".into());
    let r = s
        .undo_rebased(&mine.id, mia.clone(), &foreign)
        .expect("rebased undo");
    let undo = r.txn.cloned().expect("an undo transaction");
    assert_eq!(undo.reverts.as_ref(), Some(&mine.id));
    assert_eq!(r.skipped.len(), 1, "the position op is skipped");
    assert_eq!(get(&s, "e_a", "name"), json!("A"));
    assert_eq!(
        get(&s, "e_a", "Transform")["position"],
        json!([9, 2, 3]),
        "Sam's later change stands"
    );
    assert_eq!(s.reverted_by(&mine.id), Some(&undo.id));

    // Everything overwritten: nothing to commit, the transaction stays in effect.
    let only = s
        .transact(TxnMeta::by(mia.clone()), [set("e_b", "name", json!("M"))])
        .expect("commit")
        .txn()
        .cloned()
        .expect("txn");
    s.transact(TxnMeta::by(sam), [set("e_b", "name", json!("S"))])
        .expect("commit");
    let rev = s.revision();
    let r = s.undo_rebased(&only.id, mia.clone(), &foreign).expect("ok");
    assert!(r.txn.is_none() && r.skipped.len() == 1);
    assert_eq!(s.revision(), rev);

    // A later change by the same participant still conflicts, as with plain undo.
    let older = s
        .transact(TxnMeta::by(mia.clone()), [set("e_a", "name", json!("1"))])
        .expect("commit")
        .txn()
        .cloned()
        .expect("txn");
    s.transact(TxnMeta::by(mia.clone()), [set("e_a", "name", json!("2"))])
        .expect("commit");
    let err = s
        .undo_rebased(&older.id, mia, &foreign)
        .expect_err("conflict");
    assert_eq!(err[0].code, codes::UNDO_CONFLICT);
}

/// A scene with a pre-existing error (instance-only `overrides` on a plain record):
/// transactions are judged only on the errors they add.
const FLAWED: &str = r#"{
  "$schema": "ara://schema/scene/1",
  "id": "sc_test0001",
  "records": {
    "e_a": { "name": "A", "order": "a0", "Transform": {} },
    "e_b": { "name": "B", "parent": "e_a", "order": "a0" },
    "e_c": { "name": "C", "order": "a1", "overrides": {} }
  }
}"#;

fn flawed() -> Store {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    let doc = read(FLAWED, "test.scene", &db, None)
        .doc
        .expect("fixture reads");
    let mut ws = Workspace::new(AssetIndex::new());
    ws.insert("test.scene", doc);
    Store::with_ids(db, ws, IdGen::seeded(1))
}

fn key(d: &ara_diag::Diagnostic) -> String {
    format!("{}|{}|{}", d.code, d.location, d.message)
}

/// What committing `edits` must report, from whole-document checks before and
/// after: `Err` with the first edit that fails to apply, else the new errors.
fn reference(s: &Store, edits: &[Edit]) -> Result<std::collections::BTreeSet<String>, String> {
    let errors = |ws: &Workspace| -> std::collections::BTreeSet<String> {
        ws.check(&doc_id(), None, s.schema())
            .iter()
            .filter(|d| d.is_error())
            .map(key)
            .collect()
    };
    let mut ws = s.workspace().clone();
    let before = errors(&ws);
    for edit in edits {
        ws.apply(edit.clone(), s.schema())
            .map_err(|d| d.code.to_string())?;
    }
    Ok(errors(&ws).difference(&before).cloned().collect())
}

#[test]
fn local_edits_validate_their_records_and_keep_earlier_errors_through_structural_ones() {
    let mut s = flawed();
    // Editing a record with a pre-existing error commits; the error is not new.
    s.transact(
        TxnMeta::by(Author::Human),
        [set("e_c", "name", json!("C2"))],
    )
    .expect("pre-existing error does not block");
    // A local edit that adds an error is rejected, alone or followed by a valid move.
    let add_error = set("e_a", "overrides", json!({}));
    let move_b = Edit::Move {
        doc: doc_id(),
        id: rid("e_b"),
        parent: None,
        order: None,
    };
    for edits in [vec![add_error.clone()], vec![add_error, move_b.clone()]] {
        let before = text(&s);
        let err = s
            .transact(TxnMeta::by(Author::Human), edits.clone())
            .expect_err("new error");
        assert_eq!(
            err.iter().map(|d| d.code).collect::<Vec<_>>(),
            [codes::NOT_AN_INSTANCE],
            "{edits:?}"
        );
        assert_eq!(text(&s), before);
    }
    // A local edit on the flawed record followed by a structural edit still commits.
    s.transact(
        TxnMeta::by(Author::Human),
        [set("e_c", "name", json!("C3")), move_b],
    )
    .expect("pre-existing error survives the switch to whole-document checks");
    assert_eq!(get(&s, "e_c", "name"), json!("C3"));
}

#[derive(Debug, Clone)]
enum Step {
    Name(Index, u8),
    Overrides(Index),
    ClearOverrides(Index),
    Create(Index),
    DeleteLeaf(Index),
    Reparent(Index, Option<Index>),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (any::<Index>(), any::<u8>()).prop_map(|(i, n)| Step::Name(i, n)),
        any::<Index>().prop_map(Step::Overrides),
        any::<Index>().prop_map(Step::ClearOverrides),
        any::<Index>().prop_map(Step::Create),
        any::<Index>().prop_map(Step::DeleteLeaf),
        (any::<Index>(), prop::option::of(any::<Index>())).prop_map(|(i, p)| Step::Reparent(i, p)),
    ]
}

fn step_edit(doc: &Document, step: &Step, n: &mut u32) -> Edit {
    let ids: Vec<RecordId> = doc.records().keys().cloned().collect();
    if ids.is_empty() {
        *n += 1;
        return Edit::Create {
            doc: doc_id(),
            id: rid(&format!("e_n{n}")),
            record: serde_json::Map::new(),
        };
    }
    let pick = |i: &Index| ids[i.index(ids.len())].clone();
    match step {
        Step::Name(i, v) => set(pick(i).as_str(), "name", json!(format!("n{v}"))),
        Step::Overrides(i) => set(pick(i).as_str(), "overrides", json!({})),
        Step::ClearOverrides(i) => Edit::Set {
            doc: doc_id(),
            id: pick(i),
            path: "overrides".parse().expect("path"),
            value: None,
        },
        Step::Create(parent) => {
            *n += 1;
            let mut record = serde_json::Map::new();
            record.insert("parent".into(), json!(pick(parent).as_str()));
            Edit::Create {
                doc: doc_id(),
                id: rid(&format!("e_n{n}")),
                record,
            }
        }
        Step::DeleteLeaf(i) => Edit::Delete {
            doc: doc_id(),
            id: pick(i),
        },
        Step::Reparent(i, p) => Edit::Move {
            doc: doc_id(),
            id: pick(i),
            parent: p.as_ref().map(|p| pick(p).to_string()),
            order: None,
        },
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Transactions of any mix of local and structural edits commit or fail exactly
    /// as whole-document checks before and after them say, with the same errors.
    #[test]
    fn scoped_validation_matches_whole_document_checks(
        txns in prop::collection::vec(prop::collection::vec(step(), 1..5), 1..12)
    ) {
        let mut s = flawed();
        let mut n = 0;
        for steps in &txns {
            let doc = s.workspace().doc("sc_test0001").expect("doc").clone();
            let edits: Vec<Edit> = steps.iter().map(|st| step_edit(&doc, st, &mut n)).collect();
            let want = reference(&s, &edits);
            let before = text(&s);
            match (s.transact(TxnMeta::by(Author::Human), edits.clone()), want) {
                (Ok(_), Ok(new)) => prop_assert!(new.is_empty(), "{edits:?} committed despite {new:?}"),
                (Err(got), Ok(new)) => {
                    let got: std::collections::BTreeSet<String> = got.iter().map(key).collect();
                    prop_assert_eq!(got, new, "{:?}", edits);
                    prop_assert_eq!(text(&s), before);
                }
                (Err(got), Err(code)) => prop_assert_eq!(got[0].code.to_string(), code),
                (Ok(_), Err(code)) => prop_assert!(false, "{edits:?} committed although an edit fails with {code}"),
            }
        }
    }
}
