//! The editor seam: [`CollabView`], a client-side model of everyone else, fed by
//! the `collab.*` notifications, with the queries the editor's panels draw from.
//! No UI here; see `AGENTS.md` for what each panel renders.
//!
//! ```ignore
//! let mut view = CollabView::new(me);
//! view.join(&join_result);                     // collab.join's result
//! for n in client.notifications() {           // {method, params}
//!     if view.apply(&n["method"], &n["params"]) { redraw = true; }
//! }
//! for a in view.avatars("levels/outpost.scene", "e_door01") { /* hierarchy dot */ }
//! if let Some(b) = view.badge(&doc_id, "e_door01") { /* lock badge */ }
//! if let Some(cam) = view.follow_camera() { /* drive the viewport */ }
//! ```

use std::collections::BTreeMap;

use ara_doc::{AssetId, RecordId};
use serde_json::Value;

use crate::branch::BranchStatus;
use crate::claims::Claim;
use crate::participant::{Kind, Participant, ParticipantId};
use crate::presence::{Camera, Presence};
use crate::subject::Subject;
use crate::sync::{Conflict, methods};

/// A participant's marker on an entity (hierarchy row, viewport outline).
#[derive(Debug, Clone, PartialEq)]
pub struct Avatar {
    /// Who.
    pub participant: ParticipantId,
    /// Display name.
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    /// Human or agent (agents draw a robot glyph).
    pub kind: Kind,
}

/// A claim badge on an entity, component or document.
#[derive(Debug, Clone, PartialEq)]
pub struct Badge {
    /// The claim.
    pub claim: Claim,
    /// The claimant's colour.
    pub color: String,
    /// True if I hold it.
    pub mine: bool,
    /// True if it blocks me (I am an agent and someone else holds it); humans see
    /// it as advisory.
    pub blocks_me: bool,
}

/// A viewport camera to draw (a frustum gizmo with the name).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteCamera {
    /// Whose.
    pub avatar: Avatar,
    /// Where.
    pub camera: Camera,
    /// Their pointer, if over the scene.
    pub pointer: Option<[f32; 3]>,
    /// Their open document.
    pub doc: Option<String>,
}

/// A branch session row for the Agents panel.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchRow {
    /// Branch ID.
    pub id: String,
    /// Owner's name.
    pub name: String,
    /// Status.
    pub status: BranchStatus,
    /// The turn's why.
    pub why: Option<String>,
    /// Claims it waits on.
    pub blocked_by: Vec<String>,
}

/// Everyone else, as the editor sees them.
#[derive(Debug, Clone)]
pub struct CollabView {
    me: ParticipantId,
    my_kind: Kind,
    participants: BTreeMap<ParticipantId, Participant>,
    presence: BTreeMap<ParticipantId, Presence>,
    claims: Vec<Claim>,
    conflicts: Vec<Conflict>,
    branches: BTreeMap<String, BranchRow>,
    following: Option<ParticipantId>,
}

fn parse<T: serde::de::DeserializeOwned>(v: &Value) -> Option<T> {
    serde_json::from_value(v.clone()).ok()
}

impl CollabView {
    /// An empty view for participant `me` (a human editor by default).
    pub fn new(me: ParticipantId) -> CollabView {
        CollabView {
            me,
            my_kind: Kind::Human,
            participants: BTreeMap::new(),
            presence: BTreeMap::new(),
            claims: Vec::new(),
            conflicts: Vec::new(),
            branches: BTreeMap::new(),
            following: None,
        }
    }

    /// Loads `collab.join`'s result (`participants`, `presence`, `claims`,
    /// `conflicts`, `branches`).
    pub fn join(&mut self, result: &Value) {
        self.apply(methods::ROSTER, result);
        self.apply(methods::CLAIMS, result);
        if let Some(all) = result.get("presence").and_then(Value::as_object) {
            for (id, p) in all {
                if let Some(p) = parse(p) {
                    self.presence.insert(ParticipantId(id.clone()), p);
                }
            }
        }
        for c in result["conflicts"].as_array().into_iter().flatten() {
            self.apply(methods::CONFLICT, c);
        }
        for b in result["branches"].as_array().into_iter().flatten() {
            self.apply(methods::BRANCH, &serde_json::json!({ "branch": b }));
        }
    }

    /// Applies one notification. Returns true if anything visible changed.
    pub fn apply(&mut self, method: &str, params: &Value) -> bool {
        match method {
            methods::ROSTER => {
                let Some(list) = parse::<Vec<Participant>>(&params["participants"]) else {
                    return false;
                };
                self.participants = list.into_iter().map(|p| (p.id.clone(), p)).collect();
                if let Some(me) = self.participants.get(&self.me) {
                    self.my_kind = me.kind;
                }
                self.presence
                    .retain(|id, _| self.participants.contains_key(id));
                true
            }
            methods::PRESENCE => {
                let (Some(id), Some(p)) = (
                    parse::<ParticipantId>(&params["participant"]),
                    parse::<Presence>(&params["presence"]),
                ) else {
                    return false;
                };
                self.presence.insert(id, p.clone()) != Some(p)
            }
            methods::CLAIMS => match parse::<Vec<Claim>>(&params["claims"]) {
                Some(c) => {
                    self.claims = c;
                    true
                }
                None => false,
            },
            methods::CONFLICT => match parse::<Conflict>(params) {
                Some(c) if !self.conflicts.iter().any(|x| x.id == c.id) => {
                    self.conflicts.push(c);
                    true
                }
                _ => false,
            },
            methods::BRANCH => {
                let b = &params["branch"];
                let (Some(id), Some(status)) =
                    (b["id"].as_str(), parse::<BranchStatus>(&b["status"]))
                else {
                    return false;
                };
                let row = BranchRow {
                    id: id.to_owned(),
                    name: b["name"].as_str().unwrap_or_default().to_owned(),
                    status,
                    why: b["why"].as_str().map(str::to_owned),
                    blocked_by: parse(&b["blockedBy"]).unwrap_or_default(),
                };
                self.branches.insert(id.to_owned(), row);
                true
            }
            _ => false,
        }
    }

    fn avatar(&self, id: &ParticipantId) -> Option<Avatar> {
        let p = self.participants.get(id)?;
        Some(Avatar {
            participant: p.id.clone(),
            name: p.name.clone(),
            color: p.color.clone(),
            kind: p.kind,
        })
    }

    /// Everyone here (me included), by ID.
    pub fn participants(&self) -> impl Iterator<Item = &Participant> {
        self.participants.values()
    }

    /// Others who have `entity` (an address) selected in `doc` (a path): the
    /// hierarchy row's avatar dots and the viewport selection outline colours.
    pub fn avatars(&self, doc: &str, entity: &str) -> Vec<Avatar> {
        self.presence
            .iter()
            .filter(|(id, p)| {
                **id != self.me
                    && p.doc.as_deref() == Some(doc)
                    && p.selection.iter().any(|s| s == entity)
            })
            .filter_map(|(id, _)| self.avatar(id))
            .collect()
    }

    /// Others' cameras and pointers in `doc` (a path): frustum gizmos in the viewport.
    pub fn cameras(&self, doc: &str) -> Vec<RemoteCamera> {
        self.presence
            .iter()
            .filter(|(id, p)| **id != self.me && p.doc.as_deref() == Some(doc))
            .filter_map(|(id, p)| {
                Some(RemoteCamera {
                    avatar: self.avatar(id)?,
                    camera: p.camera?,
                    pointer: p.pointer,
                    doc: p.doc.clone(),
                })
            })
            .collect()
    }

    /// The claim badge for `entity` (a record ID or address) of document `doc`: the
    /// entity's own claim, else a component/field claim on it, else a claim on the
    /// whole document.
    pub fn badge(&self, doc: &AssetId, entity: &str) -> Option<Badge> {
        let record = entity.split('/').next().unwrap_or(entity);
        let rank = |c: &Claim| match &c.subject {
            Subject::Entity { doc: d, entity: e } if d == doc && e.as_str() == record => Some(0),
            Subject::Field {
                doc: d, entity: e, ..
            } if d == doc && e.as_str() == record => Some(1),
            Subject::Doc { doc: d } if d == doc => Some(2),
            _ => None,
        };
        let c = self
            .claims
            .iter()
            .filter_map(|c| rank(c).map(|r| (r, c)))
            .min_by_key(|(r, _)| *r)?
            .1;
        let mine = c.holder == self.me;
        Some(Badge {
            color: self
                .participants
                .get(&c.holder)
                .map_or_else(|| "#888888".into(), |p| p.color.clone()),
            mine,
            blocks_me: !mine && self.my_kind == Kind::Agent,
            claim: c.clone(),
        })
    }

    /// Every live claim on records of `doc`, by record (inspector headers).
    pub fn claims_in(&self, doc: &AssetId) -> BTreeMap<RecordId, Vec<&Claim>> {
        let mut out: BTreeMap<RecordId, Vec<&Claim>> = BTreeMap::new();
        for c in &self.claims {
            if let Subject::Entity { doc: d, entity } | Subject::Field { doc: d, entity, .. } =
                &c.subject
                && d == doc
            {
                out.entry(entity.clone()).or_default().push(c);
            }
        }
        out
    }

    /// Follow `who`'s camera (`None` stops). Send the same in my presence
    /// (`following`) so others see it.
    pub fn follow(&mut self, who: Option<ParticipantId>) {
        self.following = who;
    }

    /// The camera to drive my viewport with while following someone.
    pub fn follow_camera(&self) -> Option<Camera> {
        self.presence.get(self.following.as_ref()?)?.camera
    }

    /// My changes that lost, newest last: the conflict log panel.
    pub fn my_conflicts(&self) -> impl Iterator<Item = &Conflict> {
        self.conflicts
            .iter()
            .filter(|c| c.loser.participant.as_ref() == Some(&self.me))
    }

    /// Branch sessions, by ID: the Agents panel's pending-merge rows.
    pub fn branches(&self) -> impl Iterator<Item = &BranchRow> {
        self.branches.values()
    }
}
