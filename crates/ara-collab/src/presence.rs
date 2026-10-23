//! Presence: what each participant is looking at and doing. Ephemeral (never in
//! documents), broadcast on `collab.presence` with per-participant rate limiting:
//! an update inside the interval is coalesced and sent by the next flush, so a 200 Hz
//! camera drag costs at most `1000 / interval_ms` broadcasts a second.

use std::collections::BTreeMap;

use ara_schema::Schema;
use serde::{Deserialize, Serialize};

use crate::ParticipantId;

/// A viewport camera, in the scene's conventions (Y-up, meters, degrees).
#[derive(Debug, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Camera {
    /// World position (meters).
    pub position: [f32; 3],
    /// XYZ Euler rotation (degrees), like `Transform.rotation`.
    pub rotation: [f32; 3],
    /// Vertical field of view (degrees), if known.
    pub fov: Option<f32>,
}

/// What a participant is doing: the current command or agent turn and its why.
#[derive(Debug, Clone, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Activity {
    /// The command or turn (`entity.create, transform.set`, `editing lights`).
    pub what: String,
    /// Why, in the participant's words.
    pub why: Option<String>,
    /// When it started (host clock, ms); set by the host.
    pub at: Option<u64>,
}

/// Where a participant is and what they are doing. Clients send the whole state;
/// absent fields are cleared.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Presence {
    /// The open document (project path, `levels/outpost.scene`).
    pub doc: Option<String>,
    /// The viewport camera.
    pub camera: Option<Camera>,
    /// Selected entity addresses (`e_8b21`, `e_4f9a/e_m4tz`).
    pub selection: Vec<String>,
    /// World point under the pointer in the viewport.
    pub pointer: Option<[f32; 3]>,
    /// The current command or turn. Agents' activity also follows their commits.
    pub activity: Option<Activity>,
    /// The participant (session handle) whose camera this one follows.
    pub following: Option<String>,
}

#[derive(Debug, Default)]
struct Slot {
    presence: Presence,
    last_sent: Option<u64>,
    dirty: bool,
    /// Last update from the participant (ms); idle detection.
    updated: u64,
}

/// Everyone's presence, with the broadcast rate limit.
#[derive(Debug)]
pub struct PresenceBoard {
    slots: BTreeMap<ParticipantId, Slot>,
    interval_ms: u64,
}

impl PresenceBoard {
    /// A board that broadcasts each participant at most once per `interval_ms`.
    pub fn new(interval_ms: u64) -> Self {
        PresenceBoard {
            slots: BTreeMap::new(),
            interval_ms,
        }
    }

    /// Stores `presence`. Returns it when it should be broadcast now; otherwise it is
    /// coalesced and returned by a later [`flush`](Self::flush).
    pub fn set(&mut self, id: &ParticipantId, presence: Presence, now: u64) -> Option<Presence> {
        let slot = self.slots.entry(id.clone()).or_default();
        if slot.presence == presence && !slot.dirty {
            slot.updated = now;
            return None;
        }
        slot.presence = presence;
        slot.updated = now;
        let due = slot
            .last_sent
            .is_none_or(|t| now.saturating_sub(t) >= self.interval_ms);
        if due {
            slot.last_sent = Some(now);
            slot.dirty = false;
            Some(slot.presence.clone())
        } else {
            slot.dirty = true;
            None
        }
    }

    /// Sets only the activity (agent commits); same rate limit as [`set`](Self::set).
    pub fn set_activity(
        &mut self,
        id: &ParticipantId,
        activity: Activity,
        now: u64,
    ) -> Option<Presence> {
        let mut p = self.get(id).cloned().unwrap_or_default();
        p.activity = Some(activity);
        self.set(id, p, now)
    }

    /// Coalesced updates whose interval has passed, to broadcast now.
    pub fn flush(&mut self, now: u64) -> Vec<(ParticipantId, Presence)> {
        let mut out = Vec::new();
        for (id, slot) in &mut self.slots {
            let due = slot
                .last_sent
                .is_none_or(|t| now.saturating_sub(t) >= self.interval_ms);
            if slot.dirty && due {
                slot.dirty = false;
                slot.last_sent = Some(now);
                out.push((id.clone(), slot.presence.clone()));
            }
        }
        out
    }

    /// A participant's latest presence.
    pub fn get(&self, id: &ParticipantId) -> Option<&Presence> {
        self.slots.get(id).map(|s| &s.presence)
    }

    /// When a participant last updated its presence (ms).
    pub fn updated(&self, id: &ParticipantId) -> Option<u64> {
        self.slots.get(id).map(|s| s.updated)
    }

    /// Forgets a participant.
    pub fn remove(&mut self, id: &ParticipantId) {
        self.slots.remove(id);
    }

    /// Everyone's latest presence.
    pub fn all(&self) -> impl Iterator<Item = (&ParticipantId, &Presence)> {
        self.slots.iter().map(|(id, s)| (id, &s.presence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(x: f32) -> Presence {
        Presence {
            camera: Some(Camera {
                position: [x, 0.0, 0.0],
                rotation: [0.0; 3],
                fov: None,
            }),
            ..Presence::default()
        }
    }

    #[test]
    fn updates_inside_the_interval_coalesce() {
        let mut board = PresenceBoard::new(50);
        let id = ParticipantId::from("ses_a");
        assert!(board.set(&id, cam(0.0), 0).is_some());
        // 200 Hz for 100 ms: only the coalesced latest goes out per interval.
        let mut sent = 0;
        for i in 1..=20u64 {
            let now = i * 5;
            sent += usize::from(board.set(&id, cam(i as f32), now).is_some());
            sent += board.flush(now).len();
        }
        assert_eq!(sent, 2, "one broadcast per 50 ms");
        assert_eq!(board.get(&id), Some(&cam(20.0)));
        assert!(board.set(&id, cam(20.0), 200).is_none(), "unchanged");
    }
}
