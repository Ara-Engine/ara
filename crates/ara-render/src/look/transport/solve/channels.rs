//! Optional sparse, memory-capped linear light responses. Support is discovered
//! through reverse links, including every nonzero reachable patch. A rejected or
//! invalid channel goes back to the event solver, never a dense implicit cache.

use super::{magnitude, valid_radiance};
use crate::look::transport::{
    changes::{TransportChanges, flags},
    links::LinkStore,
    reverse::ReverseLinks,
};
use anyhow::{Result, ensure};
use glam::Vec3;
use std::collections::{BTreeMap, BTreeSet};

/// Generations of every scene domain a channel's support relies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Domain {
    Instance(u64),
    Geometry(u64),
    Material(u64),
    Portal(u64),
}

pub(crate) struct Channel {
    pub light: u64,
    pub shape_generation: u64,
    pub support: Vec<(Domain, u64)>,
    pub values: Vec<(u32, Vec3)>,
    pub forcing: Vec<(u32, Vec3)>,
    pub irradiance: Vec<(u32, Vec3)>,
    pub valid: bool,
    pub truncated: bool,
    /// Certified absolute error at unit input, propagated at readout with gain.
    pub error_bound: f32,
}

impl Channel {
    pub fn bytes(&self) -> usize {
        size_of::<Self>()
            + self.values.capacity() * size_of::<(u32, Vec3)>()
            + self.forcing.capacity() * size_of::<(u32, Vec3)>()
            + self.irradiance.capacity() * size_of::<(u32, Vec3)>()
            + self.support.capacity() * size_of::<(Domain, u64)>()
            + 80
    }
}

pub(crate) struct Channels {
    entries: BTreeMap<u64, Channel>,
    gains: BTreeMap<u64, Vec3>,
    by_patch: BTreeMap<u32, Vec<u64>>,
    cap: usize,
}

impl Channels {
    pub fn contains(&self, light: u64) -> bool {
        self.entries
            .get(&light)
            .is_some_and(|c| c.valid && !c.truncated)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn forcing(&self, patch: u32) -> Vec3 {
        self.by_patch
            .get(&patch)
            .into_iter()
            .flatten()
            .fold(Vec3::ZERO, |sum, id| {
                let c = &self.entries[id];
                sum + c
                    .forcing
                    .binary_search_by_key(&patch, |v| v.0)
                    .ok()
                    .map_or(Vec3::ZERO, |i| c.forcing[i].1 * self.gains[id])
            })
    }

    /// Sparse inputs are combined by the pixel readout, never over the whole
    /// patch atlas on an intensity event.
    pub fn readout(&self, patch: u32) -> impl Iterator<Item = (Vec3, Vec3)> + '_ {
        self.by_patch
            .get(&patch)
            .into_iter()
            .flatten()
            .filter_map(move |id| {
                let c = &self.entries[id];
                if !c.valid || c.truncated {
                    return None;
                }
                c.irradiance
                    .binary_search_by_key(&patch, |v| v.0)
                    .ok()
                    .map(|i| (c.irradiance[i].1, self.gains[id]))
            })
    }

    /// Dissolve at publication, before invalid support can be sampled. Keeping
    /// the complete response as the solver's starting value preserves old debt.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.gains.clear();
        self.by_patch.clear();
    }

    pub fn new(cap: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            gains: BTreeMap::new(),
            by_patch: BTreeMap::new(),
            cap,
        }
    }

    /// Admission is atomic and includes the readout incidence table. No eviction
    /// silently truncates another light's response or changes the bounce solution.
    pub fn insert(&mut self, mut channel: Channel, gain: Vec3) -> Result<bool> {
        ensure!(valid_radiance(gain), "invalid channel gain");
        ensure!(
            !self.entries.contains_key(&channel.light),
            "replace a channel after removing its old response"
        );
        channel.values.sort_unstable_by_key(|v| v.0);
        ensure!(
            !channel.values.windows(2).any(|w| w[0].0 == w[1].0)
                && channel.values.iter().all(|v| valid_radiance(v.1)),
            "invalid sparse response"
        );
        channel.support.sort_unstable();
        channel.support.dedup();
        let additional = channel.bytes() + channel.values.len() * 88 + 64;
        if self.resident_bytes().saturating_add(additional) > self.cap {
            return Ok(false);
        }
        for &(patch, _) in &channel.values {
            self.by_patch.entry(patch).or_default().push(channel.light);
        }
        self.gains.insert(channel.light, gain);
        self.entries.insert(channel.light, channel);
        Ok(true)
    }

    /// Pure intensity/tint changes do not mutate response arrays. Shape/transform
    /// invalidation is a distinct operation and makes the fast readout unavailable.
    pub fn set_gain(&mut self, light: u64, gain: Vec3) -> Result<()> {
        ensure!(valid_radiance(gain), "invalid channel gain");
        if let Some(current) = self.gains.get_mut(&light) {
            *current = gain;
        }
        Ok(())
    }

    pub fn invalidate(&mut self, events: &TransportChanges) -> Vec<u64> {
        let changed: BTreeSet<_> = events
            .instances
            .iter()
            .map(|e| Domain::Instance(e.change.id))
            .chain(events.geometries.iter().map(|e| Domain::Geometry(e.id)))
            .chain(events.materials.iter().map(|e| Domain::Material(e.id)))
            .chain(events.portals.iter().map(|e| Domain::Portal(e.id)))
            .collect();
        let mut invalid = Vec::new();
        for channel in self.entries.values_mut() {
            let shape_changed = events.lights.iter().any(|e| {
                e.id == channel.light
                    && e.flags & (flags::ADDED | flags::REMOVED | flags::MOVED | flags::SHAPE) != 0
            });
            if channel.valid
                && (events.reset
                    || shape_changed
                    || channel
                        .support
                        .iter()
                        .any(|(domain, _)| changed.contains(domain)))
            {
                channel.valid = false;
                invalid.push(channel.light);
            }
        }
        invalid
    }

    pub fn validate_generations(&mut self, mut generation: impl FnMut(Domain) -> Option<u64>) {
        for channel in self.entries.values_mut() {
            channel.valid &= channel
                .support
                .iter()
                .all(|&(domain, stamp)| generation(domain) == Some(stamp));
        }
    }

    /// Called for read patches only. `None` means the caller must use its capped
    /// solver's complete response; it must not add stale channels to that response.
    pub fn read(&self, patch: u32) -> Option<(Vec3, f32)> {
        let Some(lights) = self.by_patch.get(&patch) else {
            return Some((Vec3::ZERO, 0.));
        };
        let mut result = Vec3::ZERO;
        let mut error = 0.;
        for light in lights {
            let channel = &self.entries[light];
            if !channel.valid || channel.truncated {
                return None;
            }
            let index = channel.values.binary_search_by_key(&patch, |v| v.0).ok()?;
            let gain = self.gains[light];
            result += channel.values[index].1 * gain;
            error += channel.error_bound * magnitude(gain);
        }
        Some((result, error))
    }

    pub fn resident_bytes(&self) -> usize {
        self.entries.values().map(Channel::bytes).sum::<usize>()
            + self.gains.len() * 64
            + self
                .by_patch
                .values()
                .map(|v| 80 + v.capacity() * 8)
                .sum::<usize>()
    }

    /// Discover all exact support before allocating a response. A local-support
    /// limit rejects admission; it never drops reachable patches to make it fit.
    pub fn support(
        links: &LinkStore,
        reverse: &ReverseLinks,
        roots: &[u32],
        limit: usize,
    ) -> Option<Vec<u32>> {
        let mut support = BTreeSet::new();
        let mut pending = Vec::new();
        for &root in roots {
            if support.insert(root) {
                pending.push(root);
            }
        }
        while let Some(source) = pending.pop() {
            if support.len() > limit {
                return None;
            }
            for &link in reverse.dependencies(source) {
                let i = link as usize;
                let receiver = links.receiver(link);
                let transfer = (0..3).any(|c| {
                    links.weight[i]
                        * links.transmission[c][i]
                        * links.patches.albedo[c][receiver as usize]
                        > 0.
                });
                if transfer && support.insert(receiver) {
                    pending.push(receiver);
                }
            }
        }
        Some(support.into_iter().collect())
    }
}
