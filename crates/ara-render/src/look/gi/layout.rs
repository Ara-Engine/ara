//! The bake's small allocation descriptor, published before tracing its probes.
use anyhow::{Result, ensure};

/// Lattices a probe pool holds at most.
pub const MAX_LATTICES: usize = 4;

/// One regular probe lattice: positions only, no surface payload.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProbeLattice {
    /// First probe centre in world metres.
    pub origin: [f32; 3],
    /// Positive probe separation per axis, in metres.
    pub spacing: [f32; 3],
    /// Lattice dimensions (2..=256 per axis); probe `i` is at x = i % x, then y, z.
    pub counts: [u32; 3],
}

impl ProbeLattice {
    /// Probe count; invalid oversized input saturates instead of overflowing.
    pub fn probe_count(&self) -> usize {
        self.counts
            .iter()
            .fold(1usize, |n, &c| n.saturating_mul(c as usize))
    }

    /// Shading's surface bias for this lattice's probes, in metres.
    pub(crate) fn bias(&self) -> f32 {
        (self.spacing.iter().copied().fold(f32::INFINITY, f32::min) * 0.2).min(0.15)
    }
}

/// Dimensions of a probe pool: up to [`MAX_LATTICES`] lattices whose probes share
/// one set of GPU resources, numbered lattice after lattice (global probe ids).
/// Earlier lattices are finer: shading takes the first that covers a point and
/// blends to the next across its edge (`look-gi.slangh`). A startup owner can
/// reserve and clear the pool while acquisition continues.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeLayout {
    lattices: [ProbeLattice; MAX_LATTICES],
    len: usize,
}

impl ProbeLayout {
    /// A pool of `lattices`, finest first (one to [`MAX_LATTICES`]).
    pub fn new(lattices: &[ProbeLattice]) -> Result<Self> {
        ensure!(
            (1..=MAX_LATTICES).contains(&lattices.len()),
            "A probe pool holds one to {MAX_LATTICES} lattices"
        );
        let mut all = [ProbeLattice::default(); MAX_LATTICES];
        all[..lattices.len()].copy_from_slice(lattices);
        Ok(Self {
            lattices: all,
            len: lattices.len(),
        })
    }

    /// A pool of one lattice.
    pub fn single(lattice: ProbeLattice) -> Self {
        let mut lattices = [ProbeLattice::default(); MAX_LATTICES];
        lattices[0] = lattice;
        Self { lattices, len: 1 }
    }

    /// The lattices, finest first.
    pub fn lattices(&self) -> &[ProbeLattice] {
        &self.lattices[..self.len]
    }

    /// First global probe id of each lattice.
    pub fn bases(&self) -> [usize; MAX_LATTICES] {
        let mut bases = [0; MAX_LATTICES];
        let mut next = 0usize;
        for (base, lattice) in bases.iter_mut().zip(self.lattices()) {
            *base = next;
            next = next.saturating_add(lattice.probe_count());
        }
        bases
    }

    /// Probes in the pool; invalid oversized input saturates instead of overflowing.
    pub fn probe_count(&self) -> usize {
        self.lattices()
            .iter()
            .fold(0usize, |n, l| n.saturating_add(l.probe_count()))
    }

    /// Check the same lattice limits as acquired probe surfaces before allocation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_LATTICES).contains(&self.len),
            "Invalid probe lattice count"
        );
        for lattice in self.lattices() {
            ensure!(
                lattice.counts.iter().all(|&c| (2..=256).contains(&c)),
                "Invalid probe lattice"
            );
            ensure!(
                lattice
                    .origin
                    .iter()
                    .chain(&lattice.spacing)
                    .all(|v| v.is_finite())
                    && lattice.spacing.iter().all(|&s| s > 0.),
                "Invalid probe positions"
            );
        }
        ensure!(self.probe_count() <= 1 << 18, "Invalid probe lattice");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lattice(counts: [u32; 3]) -> ProbeLattice {
        ProbeLattice {
            origin: [0.; 3],
            spacing: [1.; 3],
            counts,
        }
    }

    #[test]
    fn pools_number_probes_lattice_after_lattice_within_limits() -> Result<()> {
        let pool = ProbeLayout::new(&[lattice([4, 2, 4]), lattice([3, 3, 3])])?;
        pool.validate()?;
        assert_eq!(pool.probe_count(), 32 + 27);
        assert_eq!(&pool.bases()[..2], &[0, 32]);
        assert!(ProbeLayout::new(&[]).is_err());
        assert!(ProbeLayout::new(&[lattice([2, 2, 2]); MAX_LATTICES + 1]).is_err());
        assert!(
            ProbeLayout::new(&[lattice([256, 256, 4]), lattice([2, 2, 2])])?
                .validate()
                .is_err()
        );
        Ok(())
    }
}
