//! Shared fixtures: a small scene, an authority (hub + store) and a simulated
//! network of replicas with per-link FIFO queues.

#![allow(dead_code)]

use std::collections::VecDeque;

use ara_collab::{Audience, Event, Hub, OpEvent, ParticipantId, Reject, Replica, Submit};
use ara_doc::{AssetId, AssetIndex, Edit, IdGen, RecordId, Workspace, read, write};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::Value;

pub const SCENE: &str = r#"{
  "$schema": "ara://schema/scene/1",
  "id": "sc_collab01",
  "records": {
    "e_door": { "name": "Door", "order": "a0", "Transform": { "position": [0, 0, 0] } },
    "e_lamp": { "name": "Lamp", "order": "a1", "Transform": { "position": [1, 0, 0] } },
    "e_bulb": { "name": "Bulb", "parent": "e_lamp", "order": "a0" },
    "e_crate": { "name": "Crate", "order": "a2" }
  }
}"#;

pub fn db() -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    db
}

pub fn workspace(db: &SchemaDb) -> Workspace {
    let doc = read(SCENE, "levels/collab.scene", db, None)
        .doc
        .expect("fixture reads");
    let mut ws = Workspace::new(AssetIndex::new());
    ws.insert("levels/collab.scene", doc);
    ws
}

pub fn store() -> Store {
    let db = db();
    let ws = workspace(&db);
    Store::with_ids(db, ws, IdGen::seeded(7))
}

pub fn doc() -> AssetId {
    AssetId::parse("sc_collab01").expect("id")
}

pub fn rid(s: &str) -> RecordId {
    RecordId::parse(s).expect("record id")
}

pub fn set(id: &str, path: &str, value: Value) -> Edit {
    Edit::Set {
        doc: doc(),
        id: rid(id),
        path: path.parse().expect("path"),
        value: Some(value),
    }
}

pub fn text(ws: &Workspace, db: &SchemaDb) -> String {
    ws.docs()
        .map(|(p, d)| format!("{p}\n{}", write(d, db)))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn get(ws: &Workspace, id: &str, path: &str) -> Value {
    let rec = ws.doc("sc_collab01").and_then(|d| d.record(id));
    let mut v = rec.map(|r| Value::Object(r.clone())).unwrap_or(Value::Null);
    for key in path.split('.') {
        v = v.get(key).cloned().unwrap_or(Value::Null);
    }
    v
}

pub fn pid(s: &str) -> ParticipantId {
    ParticipantId(s.to_owned())
}

/// What a replica receives.
#[derive(Debug, Clone)]
pub enum Inbound {
    Op(Box<OpEvent>),
    Reject(Reject),
}

/// One authority and replicas joined to it, with per-link FIFO queues.
pub struct Net {
    pub hub: Hub,
    pub store: Store,
    pub replicas: Vec<Replica>,
    pub ids: Vec<ParticipantId>,
    pub outbox: Vec<VecDeque<Submit>>,
    pub inbox: Vec<VecDeque<Inbound>>,
    pub other: Vec<Event>,
    pub now: u64,
}

impl Net {
    /// An authority with `n` human replicas (`ses_r0`, `ses_r1`, …).
    pub fn new(n: usize) -> Net {
        let store = store();
        let mut hub = Hub::new(store.revision(), IdGen::seeded(11));
        let mut replicas = Vec::new();
        let mut ids = Vec::new();
        for i in 0..n {
            let id = pid(&format!("ses_r{i}"));
            hub.join(id.clone(), Author::Human, Some(format!("r{i}")), None, 0);
            replicas.push(Replica::new(
                id.clone(),
                store.schema().clone(),
                store.workspace().clone(),
                store.revision(),
            ));
            ids.push(id);
        }
        let mut net = Net {
            hub,
            store,
            replicas,
            ids,
            outbox: vec![VecDeque::new(); n],
            inbox: vec![VecDeque::new(); n],
            other: Vec::new(),
            now: 0,
        };
        net.route();
        net.other.clear();
        net
    }

    /// Replica `i` edits locally; queues the submit.
    pub fn edit(&mut self, i: usize, edits: Vec<Edit>) -> bool {
        match self.replicas[i].edit(edits, None, None) {
            Ok(s) => {
                self.outbox[i].push_back(s);
                true
            }
            Err(_) => false,
        }
    }

    /// Delivers replica `i`'s oldest submit to the authority.
    pub fn deliver_up(&mut self, i: usize) -> bool {
        let Some(s) = self.outbox[i].pop_front() else {
            return false;
        };
        self.now += 1;
        let id = self.ids[i].clone();
        let _ = self.hub.submit(&mut self.store, &id, s, self.now);
        self.hub.publish(&self.store, &|_| None, self.now);
        self.route();
        true
    }

    /// Delivers replica `i`'s oldest inbound message.
    pub fn deliver_down(&mut self, i: usize) -> bool {
        let Some(m) = self.inbox[i].pop_front() else {
            return false;
        };
        match m {
            Inbound::Op(ev) => self.replicas[i]
                .receive(&ev)
                .expect("in-order event applies"),
            Inbound::Reject(r) => self.replicas[i].rejected(r.seq),
        }
        true
    }

    /// Routes the hub's events into replica inboxes (others kept in `other`).
    pub fn route(&mut self) {
        for e in self.hub.drain_events() {
            let parsed = match e.method {
                "collab.op" => serde_json::from_value(e.params.clone())
                    .ok()
                    .map(|ev| Inbound::Op(Box::new(ev))),
                "collab.reject" => serde_json::from_value(e.params.clone())
                    .ok()
                    .map(Inbound::Reject),
                _ => None,
            };
            match parsed {
                Some(m) => {
                    for (i, id) in self.ids.iter().enumerate() {
                        if e.to == Audience::All || e.to == Audience::Only(id.clone()) {
                            self.inbox[i].push_back(m.clone());
                        }
                    }
                }
                None => self.other.push(e),
            }
        }
    }

    /// Delivers everything still queued, in order.
    pub fn settle(&mut self) {
        loop {
            let mut moved = false;
            for i in 0..self.replicas.len() {
                while self.deliver_up(i) {
                    moved = true;
                }
            }
            for i in 0..self.replicas.len() {
                while self.deliver_down(i) {
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
    }

    /// Asserts every replica equals the authority and has nothing pending.
    pub fn assert_converged(&self) {
        let want = text(self.store.workspace(), self.store.schema());
        for (i, r) in self.replicas.iter().enumerate() {
            assert_eq!(r.pending(), 0, "replica {i} still has pending changes");
            assert_eq!(r.revision(), self.store.revision(), "replica {i} revision");
            assert_eq!(
                text(r.workspace(), r.schema()),
                want,
                "replica {i} diverged"
            );
        }
    }
}
