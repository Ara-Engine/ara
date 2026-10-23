//! Convergence: random concurrent edits from 3–5 replicas, delivered in random
//! interleavings (FIFO per link), always end with every replica byte-identical to
//! the authority and nothing pending.

mod common;

use ara_collab::ConflictKind;
use ara_doc::{Edit, IdGen, OrderKey};
use common::*;
use proptest::prelude::*;
use serde_json::json;

/// One random step of the simulation.
#[derive(Debug, Clone)]
enum Step {
    /// Replica `who` edits record number `target` with edit kind `kind` / value `v`.
    Edit {
        who: usize,
        target: usize,
        kind: u8,
        v: i64,
    },
    /// Deliver one submit from `who` to the authority.
    Up(usize),
    /// Deliver one message from the authority to `who`.
    Down(usize),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => (0usize..5, 0usize..8, 0u8..8, -3i64..4)
            .prop_map(|(who, target, kind, v)| Step::Edit { who, target, kind, v }),
        2 => (0usize..5).prop_map(Step::Up),
        2 => (0usize..5).prop_map(Step::Down),
    ]
}

/// A random edit against what replica `who` currently sees.
fn edit(net: &mut Net, who: usize, target: usize, kind: u8, v: i64, ids: &mut IdGen) -> Vec<Edit> {
    let ws = net.replicas[who].workspace();
    let Some(d) = ws.doc("sc_collab01") else {
        return Vec::new();
    };
    let records: Vec<String> = d.records().keys().map(|k| k.to_string()).collect();
    if records.is_empty() {
        return Vec::new();
    }
    let id = records[target % records.len()].clone();
    let other = records[(target + 1) % records.len()].clone();
    match kind {
        0 => vec![set(&id, "name", json!(format!("n{v}")))],
        1 => vec![set(&id, "Transform.position.x", json!(v))],
        2 => vec![set(&id, "Transform.position.y", json!(v))],
        3 => vec![
            set(&id, "Transform.scale", json!([v, 1, 1])),
            set(&id, "name", json!(format!("s{v}"))),
        ],
        // Deletes only while the scene has more than four records, so it never drains.
        4 | 5 if kind == 4 || records.len() <= 4 => {
            let new = ids.record_id("e", |s| records.iter().any(|r| r == s));
            let mut record = serde_json::Map::new();
            record.insert("name".into(), json!(format!("new{v}")));
            record.insert("parent".into(), json!(id));
            vec![Edit::Create {
                doc: doc(),
                id: new,
                record,
            }]
        }
        5 => vec![Edit::Delete {
            doc: doc(),
            id: rid(&id),
        }],
        6 => vec![Edit::Move {
            doc: doc(),
            id: rid(&id),
            parent: (v > 0).then_some(other),
            order: OrderKey::parse("a5").ok(),
        }],
        _ => vec![set(&id, "Transform", json!({ "rotation": [0, v * 10, 0] }))],
    }
}

fn run(n: usize, steps: Vec<Step>) -> Net {
    let mut net = Net::new(n);
    let mut ids: Vec<IdGen> = (0..n).map(|i| IdGen::seeded(1000 + i as u64)).collect();
    for s in steps {
        match s {
            Step::Edit {
                who,
                target,
                kind,
                v,
            } => {
                let who = who % n;
                let e = edit(&mut net, who, target, kind, v, &mut ids[who]);
                if !e.is_empty() {
                    net.edit(who, e);
                }
            }
            Step::Up(who) => {
                net.deliver_up(who % n);
            }
            Step::Down(who) => {
                net.deliver_down(who % n);
            }
        }
    }
    net.settle();
    net.assert_converged();
    net
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    #[test]
    fn random_concurrent_interleavings_converge(
        n in 3usize..=5,
        steps in proptest::collection::vec(step(), 10..120),
    ) {
        run(n, steps);
    }
}

#[test]
fn a_long_busy_session_converges() {
    // A deterministic heavy run: 5 replicas, 2000 steps.
    let mut rng = IdGen::seeded(42);
    let mut steps = Vec::new();
    for _ in 0..2000 {
        let t = rng.token("x", 6);
        let b = t.as_bytes();
        let pick = |i: usize| usize::from(b[2 + i]);
        steps.push(match pick(0) % 7 {
            0..=2 => Step::Edit {
                who: pick(1) % 5,
                target: pick(2),
                kind: (pick(3) % 8) as u8,
                v: (pick(4) % 7) as i64 - 3,
            },
            3 | 4 => Step::Up(pick(1) % 5),
            _ => Step::Down(pick(1) % 5),
        });
    }
    let net = run(5, steps);
    let kinds = |k| net.hub.conflicts().all().filter(|c| c.kind == k).count();
    let (over, dropped) = (
        kinds(ConflictKind::Overwritten),
        kinds(ConflictKind::Dropped),
    );
    assert!(
        net.store.revision() > 100,
        "only {} commits",
        net.store.revision()
    );
    assert!(
        over > 0 && dropped > 0,
        "overwritten {over}, dropped {dropped}"
    );
    eprintln!(
        "5 replicas, 2000 steps: {} commits, {over} overwritten, {dropped} dropped, converged",
        net.store.revision()
    );
}
