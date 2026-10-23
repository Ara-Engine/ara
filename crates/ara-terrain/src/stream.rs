//! Frame-boundary publication of completed worker tiles. No I/O, waits or cooking.
use crate::{Tile, TileCoord, invalid};
use ara_diag::Diagnostic;
use std::collections::BTreeMap;

struct Resident {
    tile: Tile,
    revision: u64,
    used: u64,
}

/// Last-good resident set with explicit revision rejection and bounded memory.
pub struct Residency {
    tiles: BTreeMap<TileCoord, Resident>,
    desired: BTreeMap<TileCoord, u64>,
    budget: usize,
    used: usize,
    clock: u64,
}
impl Residency {
    /// Sets a strict CPU tile-byte budget (GPU upload budgets belong to the caller).
    pub fn new(budget: usize) -> Self {
        Self {
            tiles: BTreeMap::new(),
            desired: BTreeMap::new(),
            budget,
            used: 0,
            clock: 0,
        }
    }
    /// Marks the newest needed revision. Old resident data remains available.
    pub fn request(&mut self, coord: TileCoord, revision: u64) {
        self.desired
            .entry(coord)
            .and_modify(|r| *r = (*r).max(revision))
            .or_insert(revision);
    }
    /// Reads a last-good tile, updating deterministic least-recently-used priority.
    pub fn get(&mut self, coord: TileCoord) -> Option<(&Tile, u64)> {
        self.clock = self.clock.saturating_add(1);
        self.tiles.get_mut(&coord).map(|r| {
            r.used = self.clock;
            (&r.tile, r.revision)
        })
    }
    /// Publishes an already completed result at a caller-approved frame boundary.
    /// Returns false for obsolete results or when policy forbids applying changes.
    pub fn publish(
        &mut self,
        coord: TileCoord,
        revision: u64,
        tile: Tile,
        allowed: bool,
    ) -> Result<bool, Diagnostic> {
        if !allowed || self.desired.get(&coord) != Some(&revision) {
            return Ok(false);
        }
        tile.validate()?;
        let bytes = tile.heights.len() * 9;
        if bytes > self.budget {
            return Err(invalid("tile exceeds residency budget"));
        }
        if let Some(old) = self.tiles.remove(&coord) {
            self.used -= old.tile.heights.len() * 9;
        }
        while self.used + bytes > self.budget {
            let victim = self
                .tiles
                .iter()
                .min_by_key(|(c, r)| (r.used, **c))
                .map(|(c, _)| *c);
            if let Some(c) = victim {
                if let Some(r) = self.tiles.remove(&c) {
                    self.used -= r.tile.heights.len() * 9;
                }
            } else {
                break;
            }
        }
        self.clock = self.clock.saturating_add(1);
        self.used += bytes;
        self.tiles.insert(
            coord,
            Resident {
                tile,
                revision,
                used: self.clock,
            },
        );
        Ok(true)
    }
    /// Actual sample storage bytes, excluding small map bookkeeping.
    pub fn bytes(&self) -> usize {
        self.used
    }
}
