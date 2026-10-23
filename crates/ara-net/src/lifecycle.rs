use crate::{NetError, Result};
use ara_input::PlayerId;
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Session-scoped identity. Epochs prevent delayed references aliasing reused slots.
#[derive(
    Component, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct NetId {
    /// Reusable network slot; unrelated to Bevy entity bits or persistent asset IDs.
    pub slot: u32,
    /// Generation incremented before a slot is reused.
    pub epoch: u32,
}

/// Cooked-prefab identity and initial spawn metadata replicated with the baseline.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spawn {
    /// Stable network lifetime.
    pub id: NetId,
    /// Stable simulation identity used for commands and replay.
    pub identity: String,
    /// The receiver's scene projection or cell streamer owns this row's lifetime.
    /// Bind it when resident; replication must not create a fallback entity for it.
    pub projected: bool,
    /// Cooked prefab reference (`@path#id`), or the scene's authored reference.
    pub prefab: String,
    /// Cooked prefab content/version digest.
    pub version: [u8; 32],
    /// Input owner; distinct from server authority.
    pub owner: Option<PlayerId>,
    /// Optional team used by interest predicates.
    pub team: Option<u32>,
}

/// Authority-owned lifetime allocator. Never derive these IDs from entity handles.
#[derive(Default)]
pub struct WorldIds {
    active: BTreeMap<String, NetId>,
    epochs: Vec<u32>,
    free: BTreeSet<u32>,
}
impl WorldIds {
    /// Allocates once per lifetime, retaining existing live identities.
    pub fn spawn(&mut self, identity: &str) -> Result<NetId> {
        if let Some(id) = self.active.get(identity) {
            return Ok(*id);
        }
        let id = if let Some(slot) = self.free.pop_first() {
            let epoch = self.epochs[slot as usize]
                .checked_add(1)
                .ok_or_else(|| NetError("network epoch exhausted".into()))?;
            self.epochs[slot as usize] = epoch;
            NetId { slot, epoch }
        } else {
            let slot = u32::try_from(self.epochs.len()).map_err(crate::error)?;
            self.epochs.push(1);
            NetId { slot, epoch: 1 }
        };
        self.active.insert(identity.into(), id);
        Ok(id)
    }
    /// Retires a lifetime; future spawns at this address receive a new epoch.
    pub fn despawn(&mut self, identity: &str) -> Option<NetId> {
        let id = self.active.remove(identity)?;
        self.free.insert(id.slot);
        Some(id)
    }
    /// Resolves a currently live identity.
    pub fn get(&self, identity: &str) -> Option<NetId> {
        self.active.get(identity).copied()
    }
}

/// Per-connection interest input; owned entities always remain visible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionView {
    /// Authenticated player.
    pub player: PlayerId,
    /// Camera/actor position in metres.
    pub position: [f32; 2],
    /// Maximum spatial range in metres.
    pub radius: f32,
    /// Team membership supplied by the authority.
    pub team: Option<u32>,
    /// Require team equality for entities carrying a team.
    pub team_only: bool,
    /// Application datagram byte budget per second (transport control is separate).
    pub bytes_per_second: u32,
}

/// Coarse spatial hash with deterministic candidate ordering and explicit predicates.
pub struct Interest {
    cell_size: f32,
    cells: BTreeMap<(i32, i32), BTreeSet<NetId>>,
    entries: BTreeMap<NetId, GridEntry>,
}
type GridEntry = ([f32; 2], Option<PlayerId>, Option<u32>);
impl Interest {
    /// Constructs a grid with finite positive cell size.
    pub fn new(cell_size: f32) -> Result<Self> {
        if !cell_size.is_finite() || cell_size <= 0.0 {
            return Err(NetError("invalid interest cell size".into()));
        }
        Ok(Self {
            cell_size,
            cells: BTreeMap::new(),
            entries: BTreeMap::new(),
        })
    }
    fn cell(&self, p: [f32; 2]) -> (i32, i32) {
        (
            (p[0] / self.cell_size).floor() as i32,
            (p[1] / self.cell_size).floor() as i32,
        )
    }
    /// Inserts or moves one live network entity.
    pub fn update(&mut self, spawn: &Spawn, position: [f32; 2]) -> Result<()> {
        if !position.into_iter().all(f32::is_finite) {
            return Err(NetError("non-finite interest position".into()));
        }
        self.remove(spawn.id);
        self.cells
            .entry(self.cell(position))
            .or_default()
            .insert(spawn.id);
        self.entries
            .insert(spawn.id, (position, spawn.owner, spawn.team));
        Ok(())
    }
    /// Removes a despawned lifetime from the grid.
    pub fn remove(&mut self, id: NetId) {
        if let Some((p, _, _)) = self.entries.remove(&id) {
            let cell = self.cell(p);
            if let Some(ids) = self.cells.get_mut(&cell) {
                ids.remove(&id);
                if ids.is_empty() {
                    self.cells.remove(&cell);
                }
            }
        }
    }
    /// Returns spatially relevant, owned, and allowed-team identities.
    pub fn visible(&self, view: &ConnectionView) -> Result<BTreeSet<NetId>> {
        if !view.radius.is_finite()
            || view.radius < 0.0
            || !view.position.into_iter().all(f32::is_finite)
            || view.bytes_per_second < 1200
        {
            return Err(NetError("invalid connection interest/budget".into()));
        }
        let min = self.cell([
            view.position[0] - view.radius,
            view.position[1] - view.radius,
        ]);
        let max = self.cell([
            view.position[0] + view.radius,
            view.position[1] + view.radius,
        ]);
        let mut visible = BTreeSet::new();
        // Iterate occupied cells, bounding work independently of an untrusted radius.
        for (&(x, z), ids) in &self.cells {
            if x < min.0 || x > max.0 || z < min.1 || z > max.1 {
                continue;
            }
            for id in ids {
                let (p, owner, team) = self.entries[id];
                let distance = (p[0] - view.position[0]).hypot(p[1] - view.position[1]);
                if owner == Some(view.player)
                    || (distance <= view.radius
                        && (!view.team_only || team.is_none() || team == view.team))
                {
                    visible.insert(*id);
                }
            }
        }
        for (&id, &(_, owner, _)) in &self.entries {
            if owner == Some(view.player) {
                visible.insert(id);
            }
        }
        Ok(visible)
    }
}

/// Bounded reference fixups. Epoch tombstones reject delayed spawns/references.
pub struct DeferredReferences {
    live: BTreeSet<NetId>,
    retired: BTreeMap<u32, u32>,
    pending: BTreeMap<NetId, BTreeSet<NetId>>,
    limit: usize,
}
impl DeferredReferences {
    /// Clears obsolete fixups before re-reading a source's latest replicated fields.
    pub fn clear_source(&mut self, source: NetId) {
        self.pending.retain(|_, sources| {
            sources.remove(&source);
            !sources.is_empty()
        });
    }
    /// True when an exact lifetime has a received, unretired baseline.
    pub fn is_live(&self, id: NetId) -> bool {
        self.live.contains(&id)
    }
    /// True when a lifetime was retired or superseded by a newer epoch.
    pub fn is_retired(&self, id: NetId) -> bool {
        id.epoch == 0 || self.retired.get(&id.slot).is_some_and(|e| *e >= id.epoch)
    }
    /// Number of outstanding fixups, for editor/agent diagnostics.
    pub fn pending(&self) -> usize {
        self.pending.values().map(BTreeSet::len).sum()
    }
    /// Hides a lifetime on interest exit without retiring its authority epoch.
    pub fn hidden(&mut self, id: NetId) {
        self.live.remove(&id);
        self.pending.retain(|_, sources| {
            sources.remove(&id);
            !sources.is_empty()
        });
    }
    /// Sets a hard bound on outstanding `(source,target)` fixups.
    pub fn new(limit: usize) -> Self {
        Self {
            live: BTreeSet::new(),
            retired: BTreeMap::new(),
            pending: BTreeMap::new(),
            limit,
        }
    }
    /// Marks a baseline present, returning sources whose references now resolve.
    pub fn spawned(&mut self, id: NetId) -> Result<Vec<NetId>> {
        if self.is_retired(id)
            || self
                .live
                .iter()
                .any(|live| live.slot == id.slot && live.epoch > id.epoch)
        {
            return Err(NetError("stale network epoch".into()));
        }
        self.live
            .retain(|live| live.slot != id.slot || live.epoch == id.epoch);
        self.live.insert(id);
        self.retired
            .entry(id.slot)
            .and_modify(|e| *e = (*e).max(id.epoch - 1))
            .or_insert(id.epoch - 1);
        self.pending
            .retain(|target, _| target.slot != id.slot || target.epoch >= id.epoch);
        Ok(self
            .pending
            .remove(&id)
            .unwrap_or_default()
            .into_iter()
            .collect())
    }
    /// Resolves now or records a bounded deferred reference.
    pub fn resolve(&mut self, source: NetId, target: NetId) -> Result<bool> {
        if self.live.contains(&target) {
            return Ok(true);
        }
        if self
            .retired
            .get(&target.slot)
            .is_some_and(|e| *e >= target.epoch)
        {
            return Err(NetError("reference to retired epoch".into()));
        }
        if self
            .pending
            .get(&target)
            .is_some_and(|sources| sources.contains(&source))
        {
            return Ok(false);
        }
        if self.pending.values().map(BTreeSet::len).sum::<usize>() >= self.limit {
            return Err(NetError("deferred reference budget exceeded".into()));
        }
        self.pending.entry(target).or_default().insert(source);
        Ok(false)
    }
    /// Drops references involving a retired lifetime and records its epoch.
    pub fn despawned(&mut self, id: NetId) {
        self.live.remove(&id);
        self.retired
            .entry(id.slot)
            .and_modify(|e| *e = (*e).max(id.epoch))
            .or_insert(id.epoch);
        self.pending.remove(&id);
        self.pending.retain(|_, sources| {
            sources.remove(&id);
            !sources.is_empty()
        });
    }
}
