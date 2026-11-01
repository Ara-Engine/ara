//! Error-ordered, patch-stratified residual propagation. The priority is an
//! algebraic residual and the stopping certificate is ||r||inf/(1-||T||inf),
//! not an estimated visual error. Images are scored against an independent solve.

use super::{magnitude, valid_radiance};
use crate::look::transport::{
    links::{LinkError, LinkStore},
    reverse::ReverseLinks,
};
use anyhow::{Result, ensure};
use glam::Vec3;
use std::collections::BTreeSet;

const BLOCK: usize = 32;

/// Exact current transpose, supplied either by A2's immutable CSR or by the
/// sparse source-redirect overlay maintained during visibility repair.
pub(crate) trait Dependencies {
    fn edges(&self, source: u32) -> impl Iterator<Item = u32> + '_;
}
impl Dependencies for ReverseLinks {
    fn edges(&self, source: u32) -> impl Iterator<Item = u32> + '_ {
        self.dependencies(source).iter().copied()
    }
}

/// Work cap, expressed in measured patch updates. Preparation and reverse-edge
/// work are counted separately; this is not a promise that a CPU packet takes 1 ms.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    pub patch_passes: usize,
    pub reverse_edges: usize,
    pub solution_tolerance: f32,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            patch_passes: 78_000,
            reverse_edges: 78_000 * 48,
            solution_tolerance: 1e-5,
        }
    }
}

/// Persistent debt and actual completed work, including work caused by older events.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SolveStats {
    pub patch_passes: usize,
    pub reverse_edges: usize,
    pub active_patches: usize,
    pub debt_blocks: usize,
    pub oldest_age: u64,
    pub missed_deadlines: usize,
    pub solution_bound: f32,
}

#[derive(Default, Clone, Copy)]
struct Block {
    first_dirty: Option<u64>,
    generation: u64,
    priority: u32,
    visited: u32,
}

/// Outgoing diffuse radiance solves x = forcing + rho * links * x.
/// Non-patch sources already enter forcing; albedo is FP32 and never quantized.
pub(crate) struct EventSolver {
    pub radiance: Vec<Vec3>,
    pub forcing: Vec<Vec3>,
    pub albedo: Vec<Vec3>,
    residual: Vec<Vec3>,
    blocks: Vec<Block>,
    queue: BTreeSet<(u32, std::cmp::Reverse<usize>)>,
    contraction: f32,
    row_norms: Vec<f32>,
    frame: u64,
    epoch: u64,
}

impl EventSolver {
    pub fn new(links: &LinkStore) -> Result<Self> {
        let albedo = (0..links.patches.len())
            .map(|i| Vec3::from_array(links.patches.albedo.each_ref().map(|c| c[i])))
            .collect();
        Self::new_albedo(links, albedo)
    }

    pub fn new_albedo(links: &LinkStore, albedo: Vec<Vec3>) -> Result<Self> {
        let n = links.patches.len();
        ensure!(
            albedo.len() == n
                && albedo
                    .iter()
                    .all(|&a| valid_radiance(a) && a.cmple(Vec3::ONE).all()),
            "invalid patch reflectance"
        );
        let mut result = Self {
            radiance: vec![Vec3::ZERO; n],
            forcing: vec![Vec3::ZERO; n],
            albedo,
            residual: vec![Vec3::ZERO; n],
            blocks: vec![Block::default(); n.div_ceil(BLOCK)],
            queue: BTreeSet::new(),
            contraction: 0.,
            row_norms: vec![0.; n.max(1).next_power_of_two() * 2],
            frame: 0,
            epoch: 1,
        };
        result.update_contraction(links)?;
        Ok(result)
    }

    /// Frame advance does not clear debt, residuals, or queue order.
    pub fn begin_frame(&mut self, frame: u64) -> Result<()> {
        ensure!(frame >= self.frame, "transport frame went backwards");
        self.frame = frame;
        self.epoch = self
            .epoch
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("transport epoch exhausted"))?;
        Ok(())
    }

    pub fn update_contraction(&mut self, links: &LinkStore) -> Result<()> {
        let leaves = self.row_norms.len() / 2;
        for receiver in 0..self.radiance.len() {
            self.row_norms[leaves + receiver] = self.row_norm(links, receiver);
        }
        for node in (1..leaves).rev() {
            self.row_norms[node] = self.row_norms[node * 2].max(self.row_norms[node * 2 + 1]);
        }
        self.check_contraction()
    }

    fn row_norm(&self, links: &LinkStore, receiver: usize) -> f32 {
        let mut row = Vec3::ZERO;
        for link in links.receiver_links(receiver as u32) {
            let i = link as usize;
            if (links.source[i] as usize) < self.radiance.len() {
                row +=
                    Vec3::from_array(links.transmission.each_ref().map(|c| c[i])) * links.weight[i];
            }
        }
        (row * self.albedo[receiver]).max_element()
    }

    /// Changed rows update a flat max tree. Visibility repair never needs a
    /// complete graph scan merely to re-establish the stopping factor.
    pub fn update_contraction_rows(&mut self, links: &LinkStore, rows: &[u32]) -> Result<()> {
        let leaves = self.row_norms.len() / 2;
        for &row in rows {
            let mut node = leaves + row as usize;
            self.row_norms[node] = self.row_norm(links, row as usize);
            while node > 1 {
                node /= 2;
                self.row_norms[node] = self.row_norms[node * 2].max(self.row_norms[node * 2 + 1]);
            }
        }
        self.check_contraction()
    }

    fn check_contraction(&mut self) -> Result<()> {
        let maximum = self.row_norms[1];
        ensure!(
            maximum.is_finite() && maximum < 1.,
            "transport has no infinity-norm contraction certificate ({maximum})"
        );
        self.contraction = maximum;
        Ok(())
    }

    /// A forcing delta is immediately visible to the residual queue. Existing
    /// first_dirty values survive, including repeated toggles on every frame.
    pub fn set_forcing(&mut self, patch: u32, value: Vec3) -> Result<()> {
        ensure!(valid_radiance(value), "invalid patch forcing");
        let p = patch as usize;
        ensure!(p < self.forcing.len(), "forcing patch is absent");
        self.residual[p] += value - self.forcing[p];
        self.forcing[p] = value;
        self.mark(p, self.frame);
        Ok(())
    }

    /// Re-evaluate only rows whose operator/material changed. Source deltas use
    /// reverse propagation in `run`; no full-graph per-frame solve is performed.
    pub fn refresh_row(&mut self, links: &LinkStore, patch: u32, origin: u64) {
        let p = patch as usize;
        self.residual[p] = self.evaluate(links, patch) - self.radiance[p];
        self.mark(p, origin);
    }

    pub fn evaluate(&self, links: &LinkStore, receiver: u32) -> Vec3 {
        let mut sum = Vec3::ZERO;
        for link in links.receiver_links(receiver) {
            let i = link as usize;
            if let Some(value) = self.radiance.get(links.source[i] as usize) {
                sum += *value
                    * Vec3::from_array(links.transmission.each_ref().map(|c| c[i]))
                    * links.weight[i];
            }
        }
        self.forcing[receiver as usize] + self.albedo[receiver as usize] * sum
    }

    fn mark(&mut self, patch: usize, origin: u64) {
        let block = patch / BLOCK;
        self.queue
            .remove(&(self.blocks[block].priority, std::cmp::Reverse(block)));
        let state = &mut self.blocks[block];
        state.first_dirty = Some(state.first_dirty.map_or(origin, |old| old.min(origin)));
        state.generation = self.epoch;
        let end = ((block + 1) * BLOCK).min(self.residual.len());
        let priority = self.residual[block * BLOCK..end]
            .iter()
            .map(|v| magnitude(*v))
            .fold(0., f32::max);
        state.priority = priority.to_bits();
        self.queue
            .insert((state.priority, std::cmp::Reverse(block)));
    }

    pub fn solution_bound(&self) -> f32 {
        let residual = self.queue.last().map_or(0., |key| f32::from_bits(key.0));
        residual / (1. - self.contraction)
    }

    pub fn oldest_dirty(&self) -> Option<u64> {
        self.blocks.iter().filter_map(|b| b.first_dirty).min()
    }

    /// A topology publication cannot forgive outstanding work from an older
    /// snapshot. Its debt timestamp follows the replacement until certified.
    pub fn retain_debt(&mut self, origin: u64) {
        for p in 0..self.radiance.len() {
            self.mark(p, origin);
        }
    }

    pub fn errors(&self) -> Vec<f32> {
        self.residual
            .iter()
            .map(|v| magnitude(*v) / (1. - self.contraction))
            .collect()
    }

    /// The maintained algebraic residual contains the current incoming patch
    /// sum. Readout recovers it without gathering every link again on the CPU.
    pub fn gathered(&self, patch: usize) -> Vec3 {
        let reflected = self.radiance[patch] + self.residual[patch] - self.forcing[patch];
        Vec3::from_array(std::array::from_fn(|axis| {
            if self.albedo[patch][axis] > 0. {
                (reflected[axis] / self.albedo[patch][axis]).max(0.)
            } else {
                0.
            }
        }))
    }

    /// Process whole patches, then propagate every changed dependency before
    /// considering another receiver. No partial reverse update can be skipped by
    /// a cap. Patches within each 32-lane block take one turn before repeating.
    pub fn run(
        &mut self,
        links: &LinkStore,
        reverse: &impl Dependencies,
        budget: Budget,
    ) -> Result<SolveStats> {
        ensure!(
            budget.solution_tolerance.is_finite() && budget.solution_tolerance >= 0.,
            "invalid transport tolerance"
        );
        let mut stats = SolveStats::default();
        while stats.patch_passes < budget.patch_passes
            && self.solution_bound() > budget.solution_tolerance
        {
            let Some(&(_, std::cmp::Reverse(block))) = self.queue.last() else {
                break;
            };
            let start = block * BLOCK;
            let end = (start + BLOCK).min(self.residual.len());
            let candidates =
                |visited: u32| (start..end).filter(move |&p| visited & (1 << (p - start)) == 0);
            let mut patch = candidates(self.blocks[block].visited).max_by(|&a, &b| {
                magnitude(self.residual[a])
                    .total_cmp(&magnitude(self.residual[b]))
                    .then_with(|| b.cmp(&a))
            });
            if patch.is_none() {
                self.blocks[block].visited = 0;
                patch = candidates(0).max_by(|&a, &b| {
                    magnitude(self.residual[a])
                        .total_cmp(&magnitude(self.residual[b]))
                        .then_with(|| b.cmp(&a))
                });
            }
            let p = patch.expect("queued blocks contain at least one patch");
            let edge_count = reverse.edges(p as u32).count();
            if stats.reverse_edges.saturating_add(edge_count) > budget.reverse_edges {
                break;
            }
            self.blocks[block].visited |= 1 << (p - start);
            let origin = self.blocks[block].first_dirty.unwrap_or(self.frame);
            let delta = self.residual[p];
            self.radiance[p] += delta;
            self.residual[p] = Vec3::ZERO;
            self.mark(p, origin);
            // This includes self edges and zero-weight paths. No dependent receives
            // a fresh timestamp simply because a newer event also touched it.
            for link in reverse.edges(p as u32) {
                let i = link as usize;
                let receiver = links.receiver(link) as usize;
                let transfer = self.albedo[receiver]
                    * links.weight[i]
                    * Vec3::from_array(links.transmission.each_ref().map(|c| c[i]));
                self.residual[receiver] += delta * transfer;
                if magnitude(delta * transfer) > 0. {
                    self.mark(receiver, origin);
                }
            }
            stats.patch_passes += 1;
            stats.reverse_edges += edge_count;
        }
        stats.solution_bound = self.solution_bound();
        if stats.solution_bound <= budget.solution_tolerance {
            // The certificate includes all residuals, including a receiver about
            // to be skipped. Retain those residuals for the next event's solve.
            for block in &mut self.blocks {
                block.first_dirty = None;
                block.visited = 0;
            }
        }
        for (i, block) in self.blocks.iter().enumerate() {
            if let Some(first) = block.first_dirty {
                stats.debt_blocks += 1;
                stats.active_patches += ((i + 1) * BLOCK).min(self.radiance.len()) - i * BLOCK;
                let age = self.frame.saturating_sub(first);
                stats.oldest_age = stats.oldest_age.max(age);
                stats.missed_deadlines += usize::from(age >= 3);
            }
        }
        Ok(stats)
    }

    /// Independent synchronous FP64 Jacobi oracle. It converges by the same
    /// contraction theorem with a much tighter tolerance, never residual alone.
    pub fn exact(&self, links: &LinkStore, tolerance: f64) -> Result<Vec<Vec3>> {
        self.exact_active(links, tolerance, None)
    }

    /// The caller supplies the complete reverse-reachable support. Outside rows
    /// stay exactly zero; they are not a truncated tail of the solution.
    pub fn exact_support(
        &self,
        links: &LinkStore,
        tolerance: f64,
        support: &[u32],
    ) -> Result<Vec<Vec3>> {
        self.exact_active(links, tolerance, Some(support))
    }

    fn exact_active(
        &self,
        links: &LinkStore,
        tolerance: f64,
        support: Option<&[u32]>,
    ) -> Result<Vec<Vec3>> {
        let mut values = vec![glam::DVec3::ZERO; self.radiance.len()];
        let mut next = vec![glam::DVec3::ZERO; self.radiance.len()];
        for _ in 0..100_000 {
            let mut difference = 0f64;
            for index in 0..support.map_or(next.len(), <[u32]>::len) {
                let p = support.map_or(index, |rows| rows[index] as usize);
                let output = &mut next[p];
                let mut gathered = glam::DVec3::ZERO;
                for link in links.receiver_links(p as u32) {
                    let i = link as usize;
                    if let Some(source) = values.get(links.source[i] as usize) {
                        gathered += *source
                            * glam::DVec3::from_array(
                                links.transmission.each_ref().map(|v| f64::from(v[i])),
                            )
                            * f64::from(links.weight[i]);
                    }
                }
                *output = self.forcing[p].as_dvec3() + self.albedo[p].as_dvec3() * gathered;
                difference = difference.max((*output - values[p]).abs().max_element());
            }
            std::mem::swap(&mut values, &mut next);
            if difference / (1. - f64::from(self.contraction)) <= tolerance {
                return Ok(values.into_iter().map(|v| v.as_vec3()).collect());
            }
        }
        anyhow::bail!("exact transport oracle did not converge")
    }

    pub fn resident_bytes(&self) -> usize {
        (self.radiance.capacity()
            + self.forcing.capacity()
            + self.albedo.capacity()
            + self.residual.capacity())
            * size_of::<Vec3>()
            + self.blocks.capacity() * size_of::<Block>()
            + self.row_norms.capacity() * size_of::<f32>()
            + self.queue.len() * 64
    }
}

impl From<LinkError> for anyhow::Error {
    fn from(value: LinkError) -> Self {
        anyhow::anyhow!("transport links: {value:?}")
    }
}
