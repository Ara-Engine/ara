//! The authority's collaboration rules, scenario by scenario: claims (advisory for
//! humans, enforced for agents), last-writer-wins with a conflict log, dropped
//! submits, per-participant undo, branch sessions, late join and the summary.

mod common;

use ara_collab::claims::Wanted;
use ara_collab::codes;
use ara_collab::{
    ApplyPolicy, BranchStatus, ConflictKind, Hub, Kind, Prefer, Replica, Snapshot, Subject,
};
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
fn claims_are_advisory_for_humans_and_enforced_for_agents() {
    let mut store = store();
    let mut hub = Hub::new(0, IdGen::seeded(1));
    let (mia, bot, bot2) = (pid("ses_mia"), pid("ses_bot"), pid("ses_bot2"));
    hub.join(mia.clone(), Author::Human, Some("Mia".into()), None, 0);
    hub.join(bot.clone(), agent("bot"), None, None, 0);
    hub.join(bot2.clone(), agent("bot"), Some("bot-2".into()), None, 0);
    let granted = hub
        .claim(
            &mut store,
            &mia,
            vec![entity("e_door")],
            60_000,
            Some("tuning the gate".into()),
            0,
        )
        .expect("claim");
    assert_eq!(granted.claims.len(), 1);

    // An agent's edit there is rejected, naming the claimant and the ways forward.
    store.set_origin(Some("ses_bot".into()));
    let err = store
        .transact(
            TxnMeta::by(agent("bot")),
            [set("e_door", "Transform.position.x", json!(3))],
        )
        .expect_err("claimed");
    assert_eq!(err[0].code, codes::CLAIMED);
    assert!(err[0].message.contains("Mia (human)"), "{}", err[0].message);
    assert!(err[0].message.contains("tuning the gate"));
    assert!(err[0].message.contains("branch session"));
    assert_eq!(err[0].fixes.len(), 2);
    // Other entities are fine; a human is never blocked.
    store
        .transact(
            TxnMeta::by(agent("bot")),
            [set("e_lamp", "name", json!("L"))],
        )
        .expect("unclaimed");
    store.set_origin(Some("ses_other_human".into()));
    store
        .transact(
            TxnMeta::by(Author::Human),
            [set("e_door", "name", json!("Gate"))],
        )
        .expect("humans are never blocked");

    // An agent cannot claim over it; a human can (and sees the overlap).
    let err = hub
        .claim(&mut store, &bot, vec![entity("e_door")], 60_000, None, 1)
        .expect_err("held");
    assert_eq!(err[0].code, codes::CLAIM_HELD);
    let bot_claim = hub
        .claim(&mut store, &bot, vec![entity("e_crate")], 60_000, None, 1)
        .expect("free subject");
    assert_eq!(bot_claim.claims[0].kind, Kind::Agent);
    // Claims are per participant even when two agents share an author name.
    store.set_origin(Some("ses_bot2".into()));
    let err = store
        .transact(
            TxnMeta::by(agent("bot")),
            [set("e_crate", "name", json!("X"))],
        )
        .expect_err("bot's claim binds bot-2");
    assert_eq!(err[0].code, codes::CLAIMED);
    store.set_origin(Some("ses_bot".into()));
    store
        .transact(
            TxnMeta::by(agent("bot")),
            [set("e_crate", "name", json!("Mine"))],
        )
        .expect("own claim");

    // Heartbeats extend; expiry and release free the subject.
    hub.heartbeat(&mut store, &mia, 50_000);
    hub.tick(&mut store, 70_000);
    assert_eq!(
        hub.claims().len(),
        1,
        "mia's claim was extended, bot's expired"
    );
    hub.release(&mut store, &mia, None, 70_001);
    assert!(hub.claims().is_empty());
    store
        .transact(
            TxnMeta::by(agent("bot")),
            [set("e_door", "name", json!("Bot"))],
        )
        .expect("released");
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
    assert_eq!(surfaced.len(), 1);
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
fn branch_sessions_stay_isolated_until_merged_with_a_conflict_report() {
    let mut store = store();
    let mut hub = Hub::new(0, IdGen::seeded(3));
    let (mia, bot) = (pid("ses_mia"), pid("ses_bot"));
    hub.join(mia.clone(), Author::Human, Some("Mia".into()), None, 0);
    hub.join(bot.clone(), agent("bot"), None, None, 0);
    hub.set_policy(ApplyPolicy::Immediately, None);
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
    assert_eq!(submitted.status, BranchStatus::Submitted);
    assert_eq!(submitted.blocked_by.len(), 1);
    assert!(submitted.blocked_by[0].contains("Mia"));
    assert!(!text(store.workspace(), store.schema()).contains("Yard lamp"));

    // Releasing the claim lets the next tick merge it: the crate keeps Mia's name.
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
fn humans_approve_merges_and_agents_cannot() {
    let mut store = store();
    let mut hub = Hub::new(0, IdGen::seeded(5));
    let (mia, bot) = (pid("ses_mia"), pid("ses_bot"));
    hub.join(mia.clone(), Author::Human, Some("Mia".into()), None, 0);
    hub.join(bot.clone(), agent("bot"), None, None, 0);
    hub.set_policy(ApplyPolicy::OnApproval, None);
    let info = hub.branch_open(&store, &bot, None).expect("open");
    hub.branch_store(&info.id, &bot)
        .expect("store")
        .transact(
            TxnMeta::by(agent("bot")),
            [set("e_lamp", "name", json!("X"))],
        )
        .expect("edit");
    hub.branch_submit(&mut store, &bot, &info.id, None, 1)
        .expect("submit");
    hub.tick(&mut store, 10_000);
    assert_eq!(
        hub.branch(&info.id).expect("b").status,
        BranchStatus::Submitted
    );
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
