//! The authority's collaboration rules, scenario by scenario: claims (advisory for
//! humans, enforced for agents), last-writer-wins with a conflict log, dropped
//! submits, per-participant undo, branch sessions, late join and the summary.

mod common;

use ara_collab::claims::Wanted;
use ara_collab::codes;
use ara_collab::{BranchStatus, ConflictKind, Hub, Prefer, Replica, Snapshot, Subject};
use ara_doc::{Edit, IdGen};
use ara_ops::{Author, TxnMeta};
use common::*;
use serde_json::json;

fn agent(name: &str) -> Author {
    Author::Agent(name.into())
}

fn entity(id: &str) -> Wanted {
    Wanted {
        subject: Subject::Entity {
            doc: doc(),
            entity: rid(id),
        },
        label: format!("levels/collab.scene#{id}"),
    }
}

#[test]
fn claims_are_advisory_for_everyone() {
    let mut store = store();
    let mut hub = Hub::new(0, IdGen::seeded(1));
    let (mia, bot) = (pid("ses_mia"), pid("ses_bot"));
    hub.join(mia.clone(), Author::Human, Some("Mia".into()), None, 0);
    hub.join(bot.clone(), agent("bot"), None, None, 0);
    hub.claim(&mut store, &mia, vec![entity("e_door")], 60_000, None, 0)
        .expect("claim");
    store
        .transact(
            TxnMeta::by(agent("bot")),
            [set("e_door", "name", json!("Gate"))],
        )
        .expect("edits just apply");
    let overlap = hub
        .claim(&mut store, &bot, vec![entity("e_door")], 60_000, None, 1)
        .expect("advisory");
    assert_eq!(overlap.overlaps.len(), 1);
    assert_eq!(get(store.workspace(), "e_door", "name"), json!("Gate"));
}

#[test]
fn concurrent_fields_merge_and_same_field_is_last_writer_wins_with_a_log() {
    let mut net = Net::new(2);
    // Both edit from revision 0 without seeing each other.
    net.edit(0, vec![set("e_door", "Transform.position.x", json!(1))]);
    net.edit(1, vec![set("e_door", "name", json!("Gate"))]);
    net.edit(0, vec![set("e_lamp", "name", json!("Lamp A"))]);
    net.edit(1, vec![set("e_lamp", "name", json!("Lamp B"))]);
    net.settle();
    net.assert_converged();
    let ws = net.store.workspace();
    assert_eq!(get(ws, "e_door", "Transform.position"), json!([1, 0, 0]));
    assert_eq!(
        get(ws, "e_door", "name"),
        json!("Gate"),
        "different fields merged"
    );
    // r0's lamp rename reached the authority first; r1's came later and won.
    assert_eq!(get(ws, "e_lamp", "name"), json!("Lamp B"));
    let log: Vec<_> = net.hub.conflicts().all().collect();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].kind, ConflictKind::Overwritten);
    assert_eq!(log[0].loser.participant, Some(pid("ses_r0")));
    assert_eq!(
        log[0].loser.value,
        Some(json!("Lamp A")),
        "the loser is preserved"
    );
    assert_eq!(
        log[0].winner.as_ref().and_then(|w| w.value.clone()),
        Some(json!("Lamp B"))
    );
    let surfaced: Vec<_> = net
        .other
        .iter()
        .filter(|e| e.method == "collab.conflict")
        .collect();
    assert_eq!(surfaced.len(), 2);
    assert!(
        surfaced
            .iter()
            .any(|e| e.to == ara_collab::Audience::Only(pid("ses_r1")))
    );
    assert_eq!(surfaced[0].to, ara_collab::Audience::Only(pid("ses_r0")));
}

#[test]
fn submits_that_no_longer_apply_are_dropped_and_reported() {
    let mut net = Net::new(2);
    net.edit(
        0,
        vec![Edit::Delete {
            doc: doc(),
            id: rid("e_crate"),
        }],
    );
    net.edit(1, vec![set("e_crate", "name", json!("Big crate"))]);
    net.settle();
    net.assert_converged();
    let r1 = pid("ses_r1");
    let log: Vec<_> = net.hub.conflicts().lost_by(&r1).collect();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].kind, ConflictKind::Dropped);
    assert_eq!(log[0].winner.as_ref().map(|w| w.name.as_str()), Some("r0"));
    assert!(get(net.store.workspace(), "e_crate", "name").is_null());
}

#[test]
fn undo_is_per_participant_and_rebased_over_others() {
    let mut net = Net::new(2);
    net.edit(0, vec![set("e_door", "name", json!("Mine"))]);
    net.settle();
    net.edit(1, vec![set("e_door", "Transform.position.y", json!(5))]);
    net.settle();
    let r0 = pid("ses_r0");
    let out = net.hub.undo(&mut net.store, &r0).expect("undo");
    assert!(out.txn.is_some() && out.skipped.is_empty());
    net.hub.publish(&net.store, &|_| None, 10);
    net.route();
    net.settle();
    net.assert_converged();
    let ws = net.store.workspace();
    assert_eq!(
        get(ws, "e_door", "name"),
        json!("Door"),
        "my change is undone"
    );
    assert_eq!(
        get(ws, "e_door", "Transform.position"),
        json!([0, 5, 0]),
        "yours stays"
    );

    // Redo brings mine back; then r1 overwrites my field and my undo skips it.
    net.hub.redo(&mut net.store, &r0).expect("redo");
    net.hub.publish(&net.store, &|_| None, 11);
    net.route();
    net.settle();
    assert_eq!(get(net.store.workspace(), "e_door", "name"), json!("Mine"));
    net.edit(1, vec![set("e_door", "name", json!("Theirs"))]);
    net.settle();
    let out = net.hub.undo(&mut net.store, &r0).expect("undo");
    assert!(out.txn.is_none(), "everything was overwritten");
    assert_eq!(out.skipped.len(), 1);
    assert_eq!(out.skipped[0].by, "r1");
    assert_eq!(
        get(net.store.workspace(), "e_door", "name"),
        json!("Theirs")
    );
    let err = net
        .hub
        .undo(&mut net.store, &pid("ses_nobody"))
        .expect_err("not joined");
    assert_eq!(err[0].code, codes::NOT_JOINED);
}

#[test]
fn late_joiners_replay_a_snapshot_plus_the_op_log() {
    let mut net = Net::new(2);
    net.edit(0, vec![set("e_door", "name", json!("A"))]);
    net.settle();
    let snap = Snapshot::of(&net.store);
    net.edit(1, vec![set("e_lamp", "name", json!("B"))]);
    net.edit(
        0,
        vec![set("e_crate", "Transform", json!({ "scale": [2, 2, 2] }))],
    );
    net.settle();
    let late = pid("ses_late");
    net.hub.join(late.clone(), Author::Human, None, None, 5);
    let mut r = Replica::from_snapshot(late, net.store.schema().clone(), &snap).expect("snapshot");
    for ev in net.hub.replay(&net.store, snap.revision) {
        r.receive(&ev).expect("replays");
    }
    assert_eq!(
        text(r.workspace(), r.schema()),
        text(net.store.workspace(), net.store.schema())
    );
    // A gap is detected, not silently applied.
    let mut stale =
        Replica::from_snapshot(pid("ses_x"), net.store.schema().clone(), &snap).expect("snapshot");
    let last = net.hub.replay(&net.store, 0).pop().expect("events");
    assert_eq!(
        stale.receive(&last).expect_err("gap").code,
        codes::REPLICA_GAP
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn branch_document_lifecycle_survives_persistence_merge_and_undo() {
    let mut live = store();
    let mut hub = Hub::new(0, IdGen::seeded(3));
    let bot = pid("ses_bot");
    hub.join(bot.clone(), agent("bot"), None, None, 0);
    let info = hub
        .branch_open(&live, &bot, Some("New level".into()))
        .unwrap();
    let created = ara_doc::AssetId::parse("sc_newlevel").unwrap();
    hub.branch_store(&info.id, &bot)
        .unwrap()
        .transact(
            TxnMeta::by(agent("bot")),
            [
                Edit::Document {
                    doc: created.clone(),
                    path: "levels/new.scene".into(),
                    kind: Some("scene".into()),
                },
                Edit::Create {
                    doc: created.clone(),
                    id: rid("e_root"),
                    record: json!({"name":"Root"}).as_object().unwrap().clone(),
                },
            ],
        )
        .unwrap();
    let saved = hub.branch(&info.id).unwrap().save();
    assert!(saved.base.is_empty());
    let mut restored = Hub::new(0, IdGen::seeded(4));
    restored.join(bot.clone(), agent("bot"), None, None, 0);
    restored.restore_branch(saved, &live).unwrap();
    let changes = restored.branch(&info.id).unwrap().changes();
    assert_eq!(changes[0].doc, "levels/new.scene");
    assert_eq!(changes[0].created, [rid("e_root")]);
    restored
        .branch_submit(&mut live, &bot, &info.id, None, 1)
        .unwrap();
    assert!(live.workspace().doc(created.as_str()).is_some());
    let txn = live.history().last().unwrap().id.clone();
    live.undo(&txn, agent("bot")).unwrap();
    assert!(live.workspace().doc(created.as_str()).is_none());
}

#[test]
fn branch_sessions_stay_isolated_until_merged_with_a_conflict_report() {
    let mut store = store();
    let mut hub = Hub::new(0, IdGen::seeded(3));
    let (mia, bot) = (pid("ses_mia"), pid("ses_bot"));
    hub.join(mia.clone(), Author::Human, Some("Mia".into()), None, 0);
    hub.join(bot.clone(), agent("bot"), None, None, 0);
    hub.claim(
        &mut store,
        &mia,
        vec![entity("e_door")],
        60_000,
        Some("gate".into()),
        0,
    )
    .expect("claim");

    let info = hub
        .branch_open(&store, &bot, Some("Rearrange the yard".into()))
        .expect("open");
    let live_before = text(store.workspace(), store.schema());
    {
        let b = hub.branch_store(&info.id, &bot).expect("own open branch");
        b.transact(
            TxnMeta::by(agent("bot")),
            [
                set("e_door", "Transform.position.z", json!(4)),
                set("e_lamp", "name", json!("Yard lamp")),
                set("e_crate", "name", json!("Bot crate")),
            ],
        )
        .expect("claims do not apply inside a branch");
    }
    assert_eq!(
        text(store.workspace(), store.schema()),
        live_before,
        "live untouched"
    );
    let err = hub.branch_store(&info.id, &mia).expect_err("not the owner");
    assert_eq!(err[0].code, codes::BRANCH_STATE);

    // Meanwhile the human changes the crate's name live.
    store
        .transact(
            TxnMeta::by(Author::Human),
            [set("e_crate", "name", json!("Mia crate"))],
        )
        .expect("live edit");

    // Submitted under `immediately`, but Mia's claim on the door holds it back.
    let submitted = hub
        .branch_submit(&mut store, &bot, &info.id, None, 1)
        .expect("submit");
    assert_eq!(submitted.status, BranchStatus::Merged);
    assert!(submitted.blocked_by.is_empty());

    // Releasing the advisory claim lets the next tick merge it: the crate keeps Mia's name.
    hub.release(&mut store, &mia, None, 2);
    hub.tick(&mut store, 3);
    let b = hub.branch(&info.id).expect("branch");
    assert_eq!(b.status, BranchStatus::Merged);
    let report = b.report.clone().expect("report");
    let changes = b.info().changes;
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].record, "e_crate");
    assert_eq!(report.conflicts[0].kept, Prefer::Live);
    let ws = store.workspace();
    assert_eq!(get(ws, "e_lamp", "name"), json!("Yard lamp"));
    assert_eq!(get(ws, "e_door", "Transform.position"), json!([0, 0, 4]));
    assert_eq!(get(ws, "e_crate", "name"), json!("Mia crate"));
    let merged = store.history().last().expect("merge txn");
    assert_eq!(merged.author, agent("bot"));
    assert_eq!(merged.why.as_deref(), Some("Rearrange the yard"));
    let lost: Vec<_> = hub.conflicts().lost_by(&bot).collect();
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].kind, ConflictKind::Merge);

    // Persisted branches load back with the same changes.
    let saved = hub.take_dirty_branches();
    assert_eq!(saved.len(), 1);
    let mut again = Hub::new(store.revision(), IdGen::seeded(4));
    again
        .restore_branch(saved[0].clone(), &store)
        .expect("restores");
    assert_eq!(
        again.branch(&info.id).expect("loaded").info().changes,
        changes
    );
}

#[test]
fn optional_manual_branch_merges_are_human_owned() {
    let mut store = store();
    let mut hub = Hub::new(0, IdGen::seeded(5));
    let (mia, bot) = (pid("ses_mia"), pid("ses_bot"));
    hub.join(mia.clone(), Author::Human, Some("Mia".into()), None, 0);
    hub.join(bot.clone(), agent("bot"), None, None, 0);
    let info = hub.branch_open(&store, &bot, None).expect("open");
    hub.branch_store(&info.id, &bot)
        .expect("store")
        .transact(
            TxnMeta::by(agent("bot")),
            [set("e_lamp", "name", json!("X"))],
        )
        .expect("edit");
    let err = hub
        .branch_merge(&mut store, &bot, &info.id, Prefer::Live, false)
        .expect_err("agents cannot approve");
    assert_eq!(err[0].code, codes::HUMAN_ONLY);
    let report = hub
        .branch_merge(&mut store, &mia, &info.id, Prefer::Live, false)
        .expect("approved");
    assert_eq!(report.approved_by, Some(mia));
    assert_eq!(get(store.workspace(), "e_lamp", "name"), json!("X"));
}

#[test]
fn the_who_summary_names_people_activity_claims_and_branches() {
    let mut store = store();
    let mut hub = Hub::new(0, IdGen::seeded(9));
    let (mia, bot) = (pid("ses_mia"), pid("ses_bot"));
    hub.join(mia.clone(), Author::Human, Some("Mia".into()), None, 0);
    hub.join(bot.clone(), agent("bot"), None, None, 0);
    hub.set_presence(
        &mut store,
        &mia,
        ara_collab::Presence {
            doc: Some("levels/collab.scene".into()),
            selection: vec!["e_door".into()],
            ..Default::default()
        },
        1_000,
    )
    .expect("presence");
    hub.claim(
        &mut store,
        &mia,
        vec![entity("e_door")],
        600_000,
        Some("tuning".into()),
        1_000,
    )
    .expect("claim");
    store.set_origin(Some("ses_bot".into()));
    store
        .transact(
            TxnMeta {
                why: Some("tidy up".into()),
                label: Some("property.set".into()),
                ..TxnMeta::by(agent("bot"))
            },
            [set("e_crate", "name", json!("C"))],
        )
        .expect("edit");
    hub.publish(&store, &|_| Some(pid("ses_bot")), 2_000);
    hub.branch_open(&store, &bot, None).expect("branch");
    let text = hub.who(&store, Some(&bot), 5_000);
    assert!(text.starts_with("2 here at revision 1\n"), "{text}");
    assert!(
        text.contains("- Mia (human): in levels/collab.scene; selected e_door"),
        "{text}"
    );
    assert!(
        text.contains("- bot (agent, you): doing property.set \"tidy up\" 3 s ago"),
        "{text}"
    );
    assert!(
        text.contains("- levels/collab.scene#e_door: Mia \"tuning\", about 10 min left"),
        "{text}"
    );
    assert!(text.contains("by bot: open"), "{text}");
    assert!(
        text.len() < 600,
        "summary stays small: {} chars",
        text.len()
    );
}

#[test]
fn late_join_snapshot_keeps_reference_root_storage_paths_and_project_scope() {
    use ara_doc::{AssetId, Document, RecordId, Workspace};
    use ara_ops::Store;
    use ara_schema::DocKindDesc;
    let mut schema = db();
    for (name, prefix, extension) in [
        ("project", "pj", "project.json"),
        ("import", "as", "import.json"),
    ] {
        schema
            .add_kind(DocKindDesc {
                name: name.into(),
                version: 1,
                id_prefix: prefix.into(),
                extension: extension.into(),
                doc: "Snapshot fixture metadata.".into(),
                records: vec![],
                single_root: false,
            })
            .expect("fixture kind");
    }
    let mut workspace = Workspace::default();
    workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
    let mut scene = Document::new("scene", AssetId::parse("sc_scoped").expect("scene ID"));
    scene.insert_unchecked(
        RecordId::parse("e_instance").expect("entity ID"),
        serde_json::from_value(json!({"prefab":"@prefabs/box.prefab#pf_box"})).expect("record"),
    );
    workspace.insert(workspace.storage_path("scenes/main.scene"), scene);
    workspace.insert(
        workspace.storage_path("prefabs/box.prefab"),
        Document::new("prefab", AssetId::parse("pf_box").expect("prefab ID")),
    );
    workspace.insert_project_document(
        "ara.project.json",
        Document::new("project", AssetId::parse("pj_project").expect("marker ID")),
    );
    workspace.insert_project_document(
        "settings/defaults.scene",
        Document::new("scene", AssetId::parse("sc_policy").expect("policy ID")),
    );
    workspace.insert_with_asset_path(
        workspace.storage_path("textures/logo.png.import.json"),
        Document::new("import", AssetId::parse("as_logo").expect("source ID")),
        workspace.storage_path("textures/logo.png"),
    );
    let store = Store::new(schema.clone(), workspace);
    let snapshot = Snapshot::of(&store);
    let wire = serde_json::to_value(&snapshot).expect("snapshot wire");
    assert_eq!(wire["referenceRoot"], ara_doc::AUTHORING_ROOT);
    let snapshot: Snapshot = serde_json::from_value(wire).expect("wire roundtrip");
    let replica = Replica::from_snapshot(pid("ses_scoped"), schema, &snapshot).expect("late join");
    let joined = replica.workspace();
    assert_eq!(joined.reference_root(), ara_doc::AUTHORING_ROOT);
    assert_eq!(
        joined.path_of("sc_scoped"),
        Some(format!("{}/scenes/main.scene", ara_doc::AUTHORING_ROOT).as_str())
    );
    assert_eq!(
        joined.find("scenes/main.scene").map(|id| id.as_str()),
        Some("sc_scoped")
    );
    assert_eq!(joined.index().path_of("pf_box"), Some("prefabs/box.prefab"));
    assert_eq!(joined.index().path_of("as_logo"), Some("textures/logo.png"));
    assert_eq!(
        joined
            .doc("sc_scoped")
            .expect("scene")
            .record("e_instance")
            .expect("instance")["prefab"],
        "@prefabs/box.prefab#pf_box"
    );
    assert!(joined.is_project_document("pj_project"));
    assert!(joined.is_project_document("sc_policy"));
    assert_eq!(joined.index().path_of("pj_project"), None);
    assert_eq!(joined.index().path_of("sc_policy"), None);
}

#[test]
fn legacy_late_join_snapshot_wire_keeps_an_empty_reference_root() {
    let store = store();
    let wire = serde_json::to_value(Snapshot::of(&store)).expect("generic snapshot wire");
    assert!(
        wire.get("referenceRoot").is_none(),
        "empty roots preserve the old wire shape"
    );
    let snapshot: Snapshot = serde_json::from_value(wire).expect("old snapshot wire");
    assert!(snapshot.reference_root.is_empty());
    let workspace = snapshot
        .workspace(store.schema())
        .expect("generic workspace");
    assert!(workspace.reference_root().is_empty());
    assert_eq!(
        workspace.path_of("sc_collab01"),
        Some("levels/collab.scene")
    );
}
