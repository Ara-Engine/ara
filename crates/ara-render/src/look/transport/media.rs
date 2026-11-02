//! Frame-local 25 cm media for small receivers and smoke. Only surface patches
//! participate in the bounce solve; this grid integrates extinction and outgoing
//! radiance along cached links. It never filters a screen image or keeps history.

use anyhow::{Result, ensure};
use glam::{IVec3, Vec3};
use std::collections::BTreeMap;

/// The transport metric is fixed, including when a budget is exceeded.
pub(crate) const EDGE: f32 = 0.25;

/// One occupied cell. `outgoing` is the medium's source radiance, not emission
/// integrated over a voxel; extinction determines its contribution on each path.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Cell {
    pub occupancy: f32,
    pub albedo: Vec3,
    pub outgoing: Vec3,
    pub extinction: f32,
}

/// Ordered sparse storage gives deterministic splatting and traversal. Hosts splat
/// in stable body-id order and rebuild each frame, so a departing body leaves no fog.
#[derive(Default)]
pub(crate) struct MediaGrid {
    cells: BTreeMap<[i32; 3], Cell>,
    generation: u64,
}

/// Exact piecewise-constant integration through the current grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Integral {
    pub transmission: f32,
    pub outgoing: Vec3,
    pub cells: u32,
}

impl MediaGrid {
    /// Begin a new deterministic frame; generations must never wrap or go back.
    pub fn begin_frame(&mut self, generation: u64) -> Result<()> {
        ensure!(
            generation > self.generation,
            "media generation must increase"
        );
        self.cells.clear();
        self.generation = generation;
        Ok(())
    }

    pub fn cell(&self, index: IVec3) -> Cell {
        self.cells
            .get(&index.to_array())
            .copied()
            .unwrap_or_default()
    }

    /// Splat a smoke box or small body's optical proxy. Box/voxel intersection
    /// volume apportions occupancy; overlapping sources mix by extinction, never
    /// by last-writer order. Proxy error is measured against real geometry in G3.
    pub fn splat(
        &mut self,
        min: Vec3,
        max: Vec3,
        occupancy: f32,
        extinction: f32,
        albedo: Vec3,
        outgoing: Vec3,
    ) -> Result<()> {
        ensure!(
            min.is_finite()
                && max.is_finite()
                && min.cmplt(max).all()
                && min.abs().max_element().max(max.abs().max_element()) < 100_000_000.
                && (0.0..=1.0).contains(&occupancy)
                && extinction.is_finite()
                && extinction >= 0.
                && albedo.is_finite()
                && albedo.cmpge(Vec3::ZERO).all()
                && albedo.cmple(Vec3::ONE).all()
                && outgoing.is_finite()
                && outgoing.cmpge(Vec3::ZERO).all(),
            "invalid media splat"
        );
        if occupancy == 0. || extinction == 0. {
            return Ok(());
        }
        let lo = (min / EDGE).floor().as_ivec3();
        let hi = (max / EDGE).ceil().as_ivec3();
        for z in lo.z..hi.z {
            for y in lo.y..hi.y {
                for x in lo.x..hi.x {
                    let cell_min = IVec3::new(x, y, z).as_vec3() * EDGE;
                    let overlap =
                        (max.min(cell_min + Vec3::splat(EDGE)) - min.max(cell_min)).max(Vec3::ZERO);
                    let fraction = overlap.x * overlap.y * overlap.z / EDGE.powi(3);
                    let sigma = extinction * occupancy * fraction;
                    let cell = self.cells.entry([x, y, z]).or_default();
                    let total = cell.extinction + sigma;
                    if total > 0. {
                        cell.albedo = (cell.albedo * cell.extinction + albedo * sigma) / total;
                        cell.outgoing =
                            (cell.outgoing * cell.extinction + outgoing * sigma) / total;
                    }
                    cell.extinction = total;
                    cell.occupancy = 1. - (1. - cell.occupancy) * (1. - occupancy * fraction);
                }
            }
        }
        Ok(())
    }

    /// March each crossed cell once, including negative coordinates and ties.
    /// A too-small packet limit is an error, never an unreported clear suffix.
    pub fn march(&self, start: Vec3, end: Vec3, max_cells: u32) -> Result<Integral> {
        ensure!(
            start.is_finite() && end.is_finite(),
            "nonfinite media segment"
        );
        let delta = end - start;
        let length = delta.length();
        let mut result = Integral {
            transmission: 1.,
            outgoing: Vec3::ZERO,
            cells: 0,
        };
        if length == 0. || self.cells.is_empty() {
            return Ok(result);
        }
        ensure!(length.is_finite(), "media segment is too long");
        let direction = delta / length;
        let mut cell = (start / EDGE).floor().as_ivec3();
        // A negative ray on a cell boundary starts inside the preceding cell.
        for axis in 0..3 {
            if direction[axis] < 0. && start[axis] == cell[axis] as f32 * EDGE {
                cell[axis] -= 1;
            }
        }
        let step = direction.signum().as_ivec3();
        let mut next = Vec3::splat(f32::INFINITY);
        let mut stride = Vec3::splat(f32::INFINITY);
        for axis in 0..3 {
            if direction[axis] != 0. {
                let edge = (cell[axis] + i32::from(direction[axis] > 0.)) as f32 * EDGE;
                next[axis] = ((edge - start[axis]) / direction[axis]).max(0.);
                stride[axis] = EDGE / direction[axis].abs();
            }
        }
        let mut distance = 0.;
        while distance < length {
            ensure!(result.cells < max_cells, "media march packet exhausted");
            let exit = next.min_element().min(length);
            let medium = self.cell(cell);
            let absorbed = -(-medium.extinction * (exit - distance).max(0.)).exp_m1();
            result.outgoing += result.transmission * absorbed * medium.outgoing;
            result.transmission *= 1. - absorbed;
            result.cells += 1;
            distance = exit;
            for axis in 0..3 {
                if next[axis] <= exit {
                    cell[axis] += step[axis];
                    next[axis] += stride[axis];
                }
            }
        }
        Ok(result)
    }

    /// Includes a conservative B-tree node/allocator allowance, not just payload.
    pub fn resident_bytes(&self) -> usize {
        self.cells.len() * (size_of::<Cell>() + size_of::<[i32; 3]>() + 6 * size_of::<usize>())
    }
}

/// Readout-only irradiance samples for awake bodies. Six axis lobes retain normal
/// dependence. Values are acquired from surface radiance, never solved on this grid.
#[derive(Default)]
pub(crate) struct IrradianceVolume {
    samples: BTreeMap<[i32; 3], [Vec3; 6]>,
}

impl IrradianceVolume {
    pub fn clear(&mut self) {
        self.samples.clear();
    }
    pub fn contains(&self, cell: IVec3) -> bool {
        self.samples.contains_key(&cell.to_array())
    }
    pub fn records(&self) -> impl Iterator<Item = ([i32; 3], &[Vec3; 6])> {
        self.samples.iter().map(|(&cell, lobes)| (cell, lobes))
    }

    pub fn insert(&mut self, cell: IVec3, irradiance: [Vec3; 6]) {
        self.samples.insert(cell.to_array(), irradiance);
    }

    /// Trilinear world-space readout. Missing corners are reported to the caller
    /// for acquisition; substituting a stale or unrelated sample would hide debt.
    pub fn sample(&self, position: Vec3, normal: Vec3) -> Option<Vec3> {
        let p = position / EDGE;
        let base = p.floor().as_ivec3();
        let fraction = p - base.as_vec3();
        let n = normal.normalize_or_zero();
        let mut result = Vec3::ZERO;
        for z in 0..=1 {
            for y in 0..=1 {
                for x in 0..=1 {
                    let corner = IVec3::new(x, y, z);
                    let weight =
                        Vec3::select(corner.cmpeq(IVec3::ONE), fraction, Vec3::ONE - fraction);
                    let w = weight.x * weight.y * weight.z;
                    if w == 0. {
                        continue;
                    }
                    let lobes = self.samples.get(&(base + corner).to_array())?;
                    for axis in 0..3 {
                        let side = axis * 2 + usize::from(n[axis] < 0.);
                        result += lobes[side] * (n[axis] * n[axis] * w);
                    }
                }
            }
        }
        Some(result)
    }

    pub fn resident_bytes(&self) -> usize {
        self.samples.len() * 144
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_integrates_beer_lambert_and_radiance_in_both_directions() -> Result<()> {
        let mut grid = MediaGrid::default();
        grid.begin_frame(1)?;
        grid.splat(
            Vec3::new(-0.25, 0., 0.),
            Vec3::new(0.25, 0.25, 0.25),
            1.,
            2.,
            Vec3::splat(0.7),
            Vec3::new(1., 2., 3.),
        )?;
        let a = Vec3::new(-0.5, 0.125, 0.125);
        let b = Vec3::new(0.5, 0.125, 0.125);
        for (start, end) in [(a, b), (b, a)] {
            let value = grid.march(start, end, 16)?;
            assert!((value.transmission - (-1f32).exp()).abs() < 1e-6);
            assert!(
                (value.outgoing - Vec3::new(1., 2., 3.) * (1. - (-1f32).exp())).length() < 1e-6
            );
        }
        assert!(grid.march(a, b, 1).is_err());
        grid.begin_frame(2)?;
        assert_eq!(grid.march(a, b, 1)?.transmission, 1.);
        Ok(())
    }

    #[test]
    fn zero_and_fractional_occupancy_scale_extinction() -> Result<()> {
        let mut grid = MediaGrid::default();
        grid.begin_frame(1)?;
        let end = Vec3::splat(0.25);
        grid.splat(Vec3::ZERO, end, 0., 8., Vec3::ONE, Vec3::ONE)?;
        assert_eq!(
            grid.march(Vec3::ZERO, Vec3::new(0.25, 0., 0.), 8)?
                .transmission,
            1.
        );
        grid.splat(Vec3::ZERO, end, 0.5, 8., Vec3::ONE, Vec3::ONE)?;
        let sample = grid.march(
            Vec3::new(0., 0.125, 0.125),
            Vec3::new(0.25, 0.125, 0.125),
            8,
        )?;
        assert!((sample.transmission - (-1f32).exp()).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn receiver_volume_is_normal_dependent_and_requires_current_corners() {
        let mut volume = IrradianceVolume::default();
        assert!(volume.sample(Vec3::ZERO, Vec3::X).is_none());
        let lobes = std::array::from_fn(|i| Vec3::splat(i as f32 + 1.));
        for z in 0..=1 {
            for y in 0..=1 {
                for x in 0..=1 {
                    volume.insert(IVec3::new(x, y, z), lobes);
                }
            }
        }
        assert_eq!(
            volume.sample(Vec3::splat(0.125), Vec3::NEG_Z),
            Some(Vec3::splat(6.))
        );
    }
}
