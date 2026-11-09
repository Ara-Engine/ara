//! Automatic reflection-capture placement from what the probe volume saw
//! ([`ProbeSight`]): captures sit where a probe has open space around it, each
//! influencing the room that probe sees (its hit distances along the six axes), and
//! a greedy cover picks the probes whose rooms take in the most open space not yet
//! covered. Captures are traced through (look-captures.slangh `captureTrace`), so
//! one capture serves a whole corridor it can see along, not only the surfaces
//! near its capture point. Object bounds cannot tell a cluttered corridor's open
//! middle from its walls; the probes' rays can.
use super::captures::CaptureSpec;
use super::gi::{PROBE_RECORD, ProbeLayout, ProbeUpdate, SURFACE_TEXELS};
use glam::{Quat, Vec2, Vec3};

/// Distance (m) a probe direction that mostly saw sky counts as.
const OPEN: f32 = 40.;
/// Nearest geometry (m) a capture point keeps.
const CLEARANCE: f32 = 0.3;
/// Longest room extent (m) a capture influences from its point, per direction.
const REACH: f32 = 30.;
/// A capture must newly cover at least this many probes.
const MIN_GAIN: u32 = 8;
/// Share of a probe's directions that may see open sky for it to count as enclosed:
/// only enclosed probes need covering, and only from enclosed ones. Outdoors, sky
/// and screen reflections serve; a probe in the open under a raised building would
/// otherwise "see" a huge room and capture the building from below.
const ENCLOSED: f32 = 0.25;

impl ProbeSight {
    /// Share of directions that saw open sky.
    fn sky(&self) -> f32 {
        self.distance.iter().filter(|&&d| d >= OPEN).count() as f32 / SURFACE_TEXELS as f32
    }
}

/// What one probe saw: its relocated position and how far it saw along each of
/// its [`SURFACE_TEXELS`] octahedral directions (open sky as [`OPEN`]).
#[derive(Debug, Clone, Copy)]
pub struct ProbeSight {
    /// Relocated probe centre (world metres).
    pub position: Vec3,
    /// Hit distance per octahedral direction (metres; open sky as [`OPEN`]).
    pub distance: [f32; SURFACE_TEXELS],
}

impl ProbeUpdate {
    /// The active probes' sights in this update, by probe id.
    pub fn sights(&self) -> impl Iterator<Item = (u32, ProbeSight)> + '_ {
        let (lattices, bases) = (self.layout.lattices(), self.layout.bases());
        (0..self.len()).filter_map(move |r| {
            let record = &self.records[r * PROBE_RECORD..(r + 1) * PROBE_RECORD];
            let word =
                |i: usize| -> u32 { bytemuck::pod_read_unaligned(&record[i * 4..i * 4 + 4]) };
            let id = word(0);
            let lattice = (0..lattices.len())
                .rev()
                .find(|&i| bases[i] <= id as usize)?;
            let (l, local) = (&lattices[lattice], id as usize - bases[lattice]);
            let [cx, cy, _] = l.counts.map(|c| c as usize);
            let cell = Vec3::new(
                (local % cx) as f32,
                (local / cx % cy) as f32,
                (local / (cx * cy)) as f32,
            );
            // Statics follow the texels: 64 moments, then (offset xyz, active).
            let statics = 4 + SURFACE_TEXELS * 4 + SURFACE_TEXELS * 4;
            let float = |i: usize| f32::from_bits(word(i));
            if float(statics + 3) < 0.5 {
                return None;
            }
            let offset = Vec3::new(float(statics), float(statics + 1), float(statics + 2));
            let position = Vec3::from(l.origin) + cell * Vec3::from(l.spacing) + offset;
            let distance = std::array::from_fn(|t| {
                let texel = 4 + t * 4;
                let sky = (word(texel) >> 24) as f32 / 255.;
                // The hits' mean distance (ProbeSurfaces::texels, look::gi::log_distance).
                let mean = ((word(texel + 1) >> 24) as f32 / 255. * 24. - 8.).exp2();
                if sky > 0.5 || !mean.is_finite() {
                    OPEN
                } else {
                    mean.clamp(0., OPEN)
                }
            });
            Some((id, ProbeSight { position, distance }))
        })
    }
}

/// The room a probe sees: from its position, the median distance it saw within 37
/// degrees of each axis direction (+x, -x, +y, -y, +z, -z), at most [`REACH`].
fn room(sight: &ProbeSight) -> [Vec3; 2] {
    let directions: [Vec3; SURFACE_TEXELS] = std::array::from_fn(|t| {
        super::gi::oct_decode((Vec2::new((t % 8) as f32, (t / 8) as f32) + 0.5) / 8.)
    });
    let axes = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ];
    let extent = axes.map(|axis| {
        let mut along: Vec<f32> = directions
            .iter()
            .zip(&sight.distance)
            .filter(|(d, _)| d.dot(axis) > 0.8)
            .map(|(d, &t)| t * d.dot(axis))
            .collect();
        along.sort_by(f32::total_cmp);
        along
            .get(along.len() / 2)
            .copied()
            .unwrap_or(CLEARANCE)
            .min(REACH)
    });
    let p = sight.position;
    [
        p - Vec3::new(extent[1], extent[3], extent[5]),
        p + Vec3::new(extent[0], extent[2], extent[4]),
    ]
}

/// At most `limit` captures over the probes of `layout` with `sights` (by id):
/// greedy, each time the open, enclosed probe whose room holds the most enclosed
/// probes no capture covers yet (counted per lattice with 3D prefix sums, so a pass is linear in the
/// probes), until `limit` or no room adds [`MIN_GAIN`] probes. Rooms become the
/// influence and projection boxes, captured from the probe's own position.
pub fn place_from_probes(
    layout: &ProbeLayout,
    sights: &[Option<ProbeSight>],
    limit: usize,
) -> Vec<CaptureSpec> {
    let candidates: Vec<(u32, [Vec3; 2])> = sights
        .iter()
        .enumerate()
        .filter_map(|(id, s)| {
            let s = s.as_ref()?;
            let near = s.distance.iter().copied().fold(f32::INFINITY, f32::min);
            (near >= CLEARANCE && s.sky() <= ENCLOSED).then(|| (id as u32, room(s)))
        })
        .collect();
    let mut covered: Vec<bool> = sights
        .iter()
        .map(|s| s.as_ref().is_none_or(|s| s.sky() > ENCLOSED))
        .collect();
    let (lattices, bases) = (layout.lattices(), layout.bases());
    let mut specs = Vec::new();
    while specs.len() < limit {
        // Uncovered active probes per lattice cell, as inclusive 3D prefix sums.
        let sums: Vec<Vec<u32>> = lattices
            .iter()
            .zip(bases)
            .map(|(l, base)| {
                let [cx, cy, cz] = l.counts.map(|c| c as usize);
                let mut s = vec![0u32; (cx + 1) * (cy + 1) * (cz + 1)];
                let at = |x: usize, y: usize, z: usize| (z * (cy + 1) + y) * (cx + 1) + x;
                for z in 0..cz {
                    for y in 0..cy {
                        for x in 0..cx {
                            let id = base + (z * cy + y) * cx + x;
                            let own = u32::from(!covered.get(id).copied().unwrap_or(true));
                            s[at(x + 1, y + 1, z + 1)] = own
                                + s[at(x, y + 1, z + 1)]
                                + s[at(x + 1, y, z + 1)]
                                + s[at(x + 1, y + 1, z)]
                                + s[at(x, y, z)]
                                - s[at(x, y, z + 1)]
                                - s[at(x, y + 1, z)]
                                - s[at(x + 1, y, z)];
                        }
                    }
                }
                s
            })
            .collect();
        // Cells of each lattice inside `room`: an inclusive index range per axis.
        let gain = |room: &[Vec3; 2]| -> u32 {
            lattices
                .iter()
                .zip(&sums)
                .map(|(l, s)| {
                    let [cx, cy, _] = l.counts.map(|c| c as usize);
                    let (origin, spacing) = (Vec3::from(l.origin), Vec3::from(l.spacing));
                    let lo = ((room[0] - origin) / spacing).ceil().max(Vec3::ZERO);
                    let hi = ((room[1] - origin) / spacing)
                        .floor()
                        .min(Vec3::from(l.counts.map(|c| c as f32 - 1.)));
                    if lo.cmpgt(hi).any() {
                        return 0;
                    }
                    let (a, b) = (lo.as_uvec3(), hi.as_uvec3() + 1);
                    let at = |x: u32, y: u32, z: u32| {
                        s[(z as usize * (cy + 1) + y as usize) * (cx + 1) + x as usize]
                    };
                    at(b.x, b.y, b.z) + at(a.x, a.y, b.z) + at(a.x, b.y, a.z) + at(b.x, a.y, a.z)
                        - at(a.x, b.y, b.z)
                        - at(b.x, a.y, b.z)
                        - at(b.x, b.y, a.z)
                        - at(a.x, a.y, a.z)
                })
                .sum()
        };
        let Some((id, room, best)) = candidates
            .iter()
            .filter(|(id, _)| {
                !specs
                    .iter()
                    .any(|(chosen, _): &(u32, CaptureSpec)| chosen == id)
            })
            .map(|(id, room)| (*id, *room, gain(room)))
            .max_by_key(|&(id, _, gain)| (gain, std::cmp::Reverse(id)))
        else {
            break;
        };
        if best < MIN_GAIN {
            break;
        }
        for (other, sight) in sights.iter().enumerate() {
            if let Some(s) = sight
                && s.position.cmpge(room[0]).all()
                && s.position.cmple(room[1]).all()
            {
                covered[other] = true;
            }
        }
        let point = sights[id as usize]
            .as_ref()
            .map_or(Vec3::ZERO, |s| s.position);
        let (centre, half) = (
            (room[0] + room[1]) * 0.5,
            ((room[1] - room[0]) * 0.5).max(Vec3::splat(0.05)),
        );
        // The room's own walls, floor and ceiling lie on its box: influence reaches
        // past them by its blend distance, so they receive the capture fully.
        let blend = (half.min_element() * 0.25).clamp(0.05, 1.);
        specs.push((
            id,
            CaptureSpec {
                position: centre,
                rotation: Quat::IDENTITY,
                influence: half + Vec3::splat(blend + 0.1),
                blend,
                projection_centre: Vec3::ZERO,
                projection: half,
                capture_offset: point - centre,
                priority: 0,
                resolution: 0,
                intensity: 1.,
                key: 0,
                explicit: false,
            },
        ));
    }
    specs.into_iter().map(|(_, spec)| spec).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::gi::ProbeLattice;

    /// A 2 m wide, 3 m tall, 20 m long corridor along z, sampled by a lattice.
    fn corridor() -> (ProbeLayout, Vec<Option<ProbeSight>>) {
        let lattice = ProbeLattice {
            origin: [-0.5, 0.5, -9.],
            spacing: [1., 1., 1.5],
            counts: [2, 3, 13],
        };
        let layout = ProbeLayout::new(&[lattice]).expect("one lattice");
        let sights = (0..lattice.probe_count())
            .map(|i| {
                let cell = Vec3::new((i % 2) as f32, (i / 2 % 3) as f32, (i / 6) as f32);
                let p = Vec3::from(lattice.origin) + cell * Vec3::from(lattice.spacing);
                let distance = std::array::from_fn(|t| {
                    let d = super::super::gi::oct_decode(
                        (Vec2::new((t % 8) as f32, (t / 8) as f32) + 0.5) / 8.,
                    );
                    // Distance to the corridor's box walls along d.
                    let lo = Vec3::new(-1., 0., -10.);
                    let hi = Vec3::new(1., 3., 10.);
                    let exit = |axis: usize| {
                        if d[axis] > 1e-4 {
                            (hi[axis] - p[axis]) / d[axis]
                        } else if d[axis] < -1e-4 {
                            (lo[axis] - p[axis]) / d[axis]
                        } else {
                            f32::INFINITY
                        }
                    };
                    exit(0).min(exit(1)).min(exit(2))
                });
                Some(ProbeSight {
                    position: p,
                    distance,
                })
            })
            .collect();
        (layout, sights)
    }

    #[test]
    fn a_corridor_is_covered_by_captures_seeing_along_it() {
        let (layout, sights) = corridor();
        let specs = place_from_probes(&layout, &sights, 8);
        assert!(
            !specs.is_empty() && specs.len() <= 2,
            "{} captures",
            specs.len()
        );
        let first = specs[0];
        first.validate().expect("valid capture");
        // The room spans the corridor's length, and the point keeps its clearance.
        assert!(first.influence.z > 5., "{:?}", first.influence);
        assert!(first.influence.x < 1.5, "{:?}", first.influence);
        let point = first.capture_point();
        assert!(
            point.x.abs() < 1. && point.y > 0.4 && point.y < 2.6,
            "{point}"
        );
    }
}
