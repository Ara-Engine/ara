//! Traced direction sets and sparse repair of A2's coalesced links. Index segments
//! retain the complete finite ray so a newly exposed suffix remains discoverable
//! after a blocker disappears. That storage is charged, never hidden from G3.

use super::{
    readout::SurfaceMap,
    visibility::{CpuTrace, SurfaceHit},
};
use crate::look::transport::{
    links::{
        Candidate, ExternalSource, GeometryKey, GeometryStamp, LinkStore, Segment, Visibility,
    },
    media::MediaGrid,
    patches::PatchBuffers,
    reverse::{CrossingIndex, Region, ReverseLinks},
};
use anyhow::{Result, ensure};
use ara_bvh::Ray;
use glam::Vec3;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
mod discovery;
mod witnesses;
use witnesses::RepairedWitnesses;

/// G3 compares the same ray count with two deterministic quadratures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectionScheme {
    /// One world-space sphere set, folded into each receiver's hemisphere.
    Global,
    /// Cosine hemisphere in the patch frame, rotated by persistent patch identity.
    PerPatch,
}

#[derive(Clone, Copy)]
pub(crate) enum Optical {
    /// Mask texel rejects this hit; continue through the same BLAS.
    Ignore,
    /// Ordered linear RGB transmission; continue beyond this interface.
    Transmit(Vec3),
    /// An opaque diffuse surface terminates this direction.
    Surface,
}

/// Material service for CPU construction/repair. This is evaluated on actual
/// hit geometry, including active large bodies. Small receivers use media instead.
pub(crate) trait Materials {
    fn optical(&self, hit: SurfaceHit) -> Optical;
    fn generation(&self, instance: u64) -> u64;
}

pub(crate) struct BuildSettings {
    pub directions: u32,
    pub scheme: DirectionScheme,
    pub distance: f32,
}

pub(crate) struct TraceGraph {
    pub links: LinkStore,
    pub reverse: ReverseLinks,
    pub index: CrossingIndex,
    /// Additive medium contribution on each ray (weighted by the receiver later).
    pub media: Vec<Vec3>,
    pub generations: Vec<u64>,
    /// Sparse updated witnesses; untouched links keep A2's interned sets.
    witnesses: RepairedWitnesses,
    /// Repair debt survives events and is ordered by receiver error, then patch.
    pending: BTreeMap<u32, u64>,
    /// Reverse redirects supplement the immutable CSR without a global rebuild.
    redirects: BTreeMap<u32, BTreeSet<u32>>,
    original_sources: BTreeMap<u32, u32>,
    changed_instances: BTreeSet<u64>,
    full_repair: bool,
}

struct Sample {
    source: u32,
    transmission: Vec3,
    geometry: Vec<GeometryStamp>,
    media: Vec3,
}

pub(crate) struct Query<'a, 'scene, M> {
    pub trace: &'a CpuTrace<'scene>,
    pub surfaces: &'a SurfaceMap,
    pub materials: &'a M,
    pub media: &'a MediaGrid,
    /// Active large-body radiance sources: no body-owned receiver rows.
    pub bodies: &'a BTreeMap<u64, u32>,
    pub boundary: u32,
}

impl<M: Materials> Query<'_, '_, M> {
    /// A cached clear prefix plus an unchanged opaque endpoint needs only local
    /// blocker tests. Ordered transmission and invalid endpoints use full trace.
    fn local(
        &self,
        segment: Segment,
        old_source: u32,
        witness: Option<GeometryStamp>,
        changed: &BTreeSet<u64>,
    ) -> Result<Sample> {
        let start = Vec3::from(segment.start);
        let delta = Vec3::from(segment.end) - start;
        let mut ray = Ray {
            origin: start,
            direction: delta.normalize(),
            t_min: 1e-4,
            t_max: delta.length(),
        };
        let endpoint = if let Some(stamp) = witness {
            let Some(slot) = self
                .trace
                .scene
                .slot(stamp.key.instance.slot)
                .filter(|s| s.key == stamp.key.instance)
            else {
                return self.sample(segment);
            };
            if changed.contains(&slot.id) || old_source == self.boundary {
                return self.sample(segment);
            }
            let hit = self.trace.closest_where(
                ray,
                |id| id == slot.id,
                |h| !matches!(self.materials.optical(h), Optical::Ignore),
            );
            let Some(hit) = hit.filter(|h| {
                h.primitive == stamp.key.primitive
                    && matches!(self.materials.optical(*h), Optical::Surface)
            }) else {
                return self.sample(segment);
            };
            ray.t_max = hit.distance;
            Some(hit)
        } else {
            None
        };
        let hit = self
            .trace
            .closest_where(
                ray,
                |id| changed.contains(&id),
                |h| !matches!(self.materials.optical(h), Optical::Ignore),
            )
            .or(endpoint);
        if hit.is_some_and(|h| matches!(self.materials.optical(h), Optical::Transmit(_))) {
            return self.sample(segment);
        }
        let distance = hit.map_or(ray.t_max, |h| h.distance);
        let medium = self.media.march(
            start + ray.direction * ray.t_min,
            start + ray.direction * distance,
            1_000_000,
        )?;
        let source = hit
            .and_then(|h| {
                self.surfaces
                    .patch(h.id, h.primitive, h.barycentric)
                    .or_else(|| self.bodies.get(&h.id).copied())
            })
            .unwrap_or(self.boundary);
        let black = hit.is_some_and(|h| !h.front || source == self.boundary);
        Ok(Sample {
            source,
            transmission: Vec3::splat(if black { 0. } else { medium.transmission }),
            media: medium.outgoing,
            geometry: hit
                .into_iter()
                .map(|h| GeometryStamp {
                    key: GeometryKey {
                        instance: h.instance,
                        primitive: h.primitive,
                    },
                    generation: self.materials.generation(h.id),
                })
                .collect(),
        })
    }

    fn sample(&self, segment: Segment) -> Result<Sample> {
        let start = Vec3::from(segment.start);
        let delta = Vec3::from(segment.end) - start;
        let length = delta.length();
        let direction = delta / length;
        let mut ray = Ray {
            origin: start,
            direction,
            t_min: 1e-4,
            t_max: length,
        };
        let mut value = Sample {
            source: self.boundary,
            transmission: Vec3::ONE,
            geometry: Vec::new(),
            media: Vec3::ZERO,
        };
        let mut distance = length;
        let mut endpoint_transmission = Vec3::ONE;
        while let Some(hit) = self.trace.closest(ray, |hit| {
            !matches!(self.materials.optical(hit), Optical::Ignore)
        }) {
            value.geometry.push(GeometryStamp {
                key: GeometryKey {
                    instance: hit.instance,
                    primitive: hit.primitive,
                },
                generation: self.materials.generation(hit.id),
            });
            match self.materials.optical(hit) {
                Optical::Ignore => unreachable!("filtered trace rejects ignored hits"),
                Optical::Transmit(rgb) => {
                    ensure!(
                        rgb.is_finite()
                            && rgb.cmpge(Vec3::ZERO).all()
                            && rgb.cmple(Vec3::ONE).all(),
                        "invalid surface transmission"
                    );
                    // Integrate the preceding medium before this interface, preserving
                    // ordering between coloured sheets and emitting smoke.
                    let medium = self.media.march(
                        start + direction * ray.t_min,
                        start + direction * hit.distance,
                        1_000_000,
                    )?;
                    value.media += value.transmission * medium.outgoing;
                    value.transmission *= medium.transmission * rgb;
                    ray.t_min = next_float(hit.distance);
                    if ray.t_min >= ray.t_max {
                        distance = ray.t_min;
                        break;
                    }
                }
                Optical::Surface => {
                    distance = hit.distance;
                    value.source = self
                        .surfaces
                        .patch(hit.id, hit.primitive, hit.barycentric)
                        .or_else(|| self.bodies.get(&hit.id).copied())
                        .unwrap_or(self.boundary);
                    // A back face or an unmapped opaque surface is black, never sky.
                    if !hit.front
                        || (value.source == self.boundary && !self.bodies.contains_key(&hit.id))
                    {
                        endpoint_transmission = Vec3::ZERO;
                    }
                    break;
                }
            }
        }
        let medium = self.media.march(
            start + direction * ray.t_min,
            start + direction * distance,
            1_000_000,
        )?;
        value.media += value.transmission * medium.outgoing;
        value.transmission *= medium.transmission * endpoint_transmission;
        Ok(value)
    }
}

fn next_float(value: f32) -> f32 {
    f32::from_bits(value.to_bits() + 1)
}

impl TraceGraph {
    pub fn build<M: Materials>(
        patches: Arc<PatchBuffers>,
        external: Vec<ExternalSource>,
        query: &Query<'_, '_, M>,
        settings: BuildSettings,
    ) -> Result<Self> {
        ensure!(
            settings.directions > 0
                && settings.directions <= 4096
                && settings.distance.is_finite()
                && settings.distance > 0.,
            "invalid direction set"
        );
        let count = patches.len();
        let mut media_rows = vec![Vec::new(); count];
        let links = LinkStore::build(
            Arc::clone(&patches),
            external,
            count * settings.directions as usize,
            |receiver| {
                let position = Vec3::from(patches.position(receiver));
                let normal =
                    Vec3::from_array(patches.normal.each_ref().map(|c| c[receiver as usize]));
                let id = patches.stamps[receiver as usize].id;
                let seed = id.instance ^ (u64::from(id.local) << 32) ^ u64::from(id.incarnation);
                let directions = directions(normal, seed, settings.directions, settings.scheme);
                let mut row = Vec::with_capacity(directions.len());
                for (ordinal, (direction, weight)) in directions.into_iter().enumerate() {
                    let segment = Segment {
                        start: position.to_array(),
                        end: (position + direction * settings.distance).to_array(),
                    };
                    let sample = query
                        .sample(segment)
                        .map_err(|_| crate::look::transport::links::LinkError::InvalidVisibility)?;
                    media_rows[receiver as usize].push(sample.media);
                    row.push(Candidate {
                        ordinal: ordinal as u32,
                        source: sample.source,
                        weight,
                        visibility: Visibility {
                            transmittance: sample.transmission.to_array(),
                            geometry: sample.geometry,
                        },
                        segment: Some(segment),
                    });
                }
                Ok(row)
            },
        )?;
        let mut media = vec![Vec3::ZERO; links.len()];
        for (receiver, row) in media_rows.into_iter().enumerate() {
            for (link, value) in links.receiver_links(receiver as u32).zip(row) {
                media[link as usize] = value;
            }
        }
        let reverse = ReverseLinks::build(&links)?;
        let index = CrossingIndex::build(&links);
        let generations = vec![1; links.tiles.len()];
        Ok(Self {
            links,
            reverse,
            index,
            media,
            generations,
            witnesses: RepairedWitnesses::default(),
            pending: BTreeMap::new(),
            redirects: BTreeMap::new(),
            original_sources: BTreeMap::new(),
            changed_instances: BTreeSet::new(),
            full_repair: false,
        })
    }

    pub fn discover(&mut self, region: Region, frame: u64) -> usize {
        let mut found = 0;
        self.index.region(&self.links, region, |link| {
            let age = self.pending.entry(link).or_insert(frame);
            *age = (*age).min(frame);
            found += 1;
        });
        found
    }

    pub fn changed(&mut self, id: u64) {
        self.changed_instances.insert(id);
    }

    pub fn invalidate_all(&mut self, frame: u64) {
        self.full_repair = true;
        for link in 0..self.links.len() as u32 {
            let age = self.pending.entry(link).or_insert(frame);
            *age = (*age).min(frame);
        }
    }

    /// A sparse repair packet selects greatest current receiver error, cycling
    /// receivers before repairing a second ray of the same patch. Local tests use
    /// A1's actual BLASes; no physics hull silently replaces current geometry.
    pub fn repair<M: Materials>(
        &mut self,
        query: &Query<'_, '_, M>,
        errors: &[f32],
        max_links: usize,
        generation: u64,
    ) -> Result<Vec<(u32, u64)>> {
        let mut candidates: Vec<_> = self.pending.keys().copied().collect();
        candidates.sort_unstable_by(|&a, &b| {
            let ra = self.links.receiver(a);
            let rb = self.links.receiver(b);
            errors[rb as usize]
                .total_cmp(&errors[ra as usize])
                .then_with(|| ra.cmp(&rb))
                .then_with(|| a.cmp(&b))
        });
        let mut visited = BTreeSet::new();
        let mut chosen = Vec::with_capacity(max_links.min(candidates.len()));
        while chosen.len() < max_links && !candidates.is_empty() {
            candidates.retain(|&link| {
                if chosen.len() < max_links && visited.insert(self.links.receiver(link)) {
                    chosen.push(link);
                    false
                } else {
                    true
                }
            });
            visited.clear();
        }
        let mut rows = BTreeMap::<u32, u64>::new();
        for link in chosen {
            let (first, multiple) = if let Some(w) = self.witnesses.get(link) {
                (w.first().copied(), w.len() > 1)
            } else {
                let mut witnesses = self.links.witnesses.iter(self.links.witness[link as usize]);
                (witnesses.next().copied(), witnesses.next().is_some())
            };
            let mut sample = if multiple || self.full_repair {
                query.sample(self.links.segment(link))?
            } else {
                query.local(
                    self.links.segment(link),
                    self.links.source[link as usize],
                    first,
                    &self.changed_instances,
                )?
            };
            let i = link as usize;
            self.redirect(link, sample.source);
            for axis in 0..3 {
                self.links.transmission[axis][i] = sample.transmission[axis];
            }
            self.media[i] = sample.media;
            // Match A2's canonical witness-set order. Unchanged geometry reuses
            // its interned set even when media/transmission on the ray changed.
            sample.geometry.sort_unstable();
            sample.geometry.dedup();
            if sample
                .geometry
                .iter()
                .eq(self.links.witnesses.iter(self.links.witness[i]))
            {
                self.witnesses.clear(link);
            } else {
                self.witnesses
                    .set(link, sample.geometry, self.links.len())?;
            }
            let receiver = self.links.receiver(link);
            let tile = self
                .links
                .tiles
                .partition_point(|t| t.first_receiver <= receiver)
                - 1;
            self.generations[tile] = generation;
            let origin = self.pending.remove(&link).expect("selected pending link");
            rows.entry(receiver)
                .and_modify(|v| *v = (*v).min(origin))
                .or_insert(origin);
        }
        if self.pending.is_empty() {
            self.changed_instances.clear();
            self.full_repair = false;
        }
        Ok(rows.into_iter().collect())
    }

    fn redirect(&mut self, link: u32, source: u32) {
        let old = self.links.source[link as usize];
        if old == source {
            return;
        }
        let original = *self.original_sources.entry(link).or_insert(old);
        if let Some(set) = self.redirects.get_mut(&old) {
            set.remove(&link);
            if set.is_empty() {
                self.redirects.remove(&old);
            }
        }
        if source == original {
            self.original_sources.remove(&link);
        } else {
            self.redirects.entry(source).or_default().insert(link);
        }
        self.links.source[link as usize] = source;
    }

    pub fn debt(&self, frame: u64) -> (usize, u64) {
        (
            self.pending.len(),
            self.pending
                .values()
                .map(|first| frame.saturating_sub(*first))
                .max()
                .unwrap_or(0),
        )
    }

    /// Repaired visibility witnesses remain part of the G3 index charge, even
    /// though they supplement A2's immutable, interned original witnesses.
    pub fn repaired_index_bytes(&self) -> usize {
        self.witnesses.resident_bytes()
    }

    /// Sparse reverse redirects and original-source identities are link data.
    pub fn repaired_reverse_bytes(&self) -> usize {
        self.original_sources.len() * 64
            + self
                .redirects
                .values()
                .map(|s| 64 + s.len() * 48)
                .sum::<usize>()
    }

    pub fn extra_bytes(&self) -> usize {
        self.media.capacity() * size_of::<Vec3>()
            + self.generations.capacity() * 8
            + self.repaired_index_bytes()
            + self.repaired_reverse_bytes()
            + self.pending.len() * 64
            + self.changed_instances.len() * 48
    }
}

impl super::scheduler::Dependencies for TraceGraph {
    fn edges(&self, source: u32) -> impl Iterator<Item = u32> + '_ {
        self.reverse
            .dependencies(source)
            .iter()
            .copied()
            .filter(move |&link| self.links.source[link as usize] == source)
            .chain(
                self.redirects
                    .get(&source)
                    .into_iter()
                    .flat_map(|rows| rows.iter().copied()),
            )
    }
}

pub(crate) fn directions(
    normal: Vec3,
    seed: u64,
    count: u32,
    scheme: DirectionScheme,
) -> Vec<(Vec3, f32)> {
    let mut result = Vec::with_capacity(count as usize);
    let tangent = normal.any_orthonormal_vector();
    let bitangent = normal.cross(tangent);
    let phase = (seed.wrapping_mul(0x9e3779b97f4a7c15) >> 40) as f32 / (1u32 << 24) as f32;
    let mut total = 0.;
    for i in 0..count {
        let (direction, weight) = match scheme {
            DirectionScheme::Global => {
                let z = 1. - 2. * (i as f32 + 0.5) / count as f32;
                let phi = i as f32 * 2.3999631;
                let r = (1. - z * z).sqrt();
                let d = Vec3::new(r * phi.cos(), r * phi.sin(), z);
                let d = if d.dot(normal) < 0. { -d } else { d };
                (d, d.dot(normal))
            }
            DirectionScheme::PerPatch => {
                let r = ((i as f32 + 0.5) / count as f32).sqrt();
                let phi = std::f32::consts::TAU
                    * (i.reverse_bits() as f64 / 4294967296. + f64::from(phase)) as f32;
                (
                    tangent * (r * phi.cos())
                        + bitangent * (r * phi.sin())
                        + normal * (1. - r * r).sqrt(),
                    1.,
                )
            }
        };
        result.push((direction, weight));
        total += weight;
    }
    for (_, weight) in &mut result {
        *weight /= total;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::transport::solve::scheduler::Dependencies;
    #[test]
    fn sparse_redirects_match_rebuilt_transpose_and_release_restored_edges() -> Result<()> {
        let links = crate::look::transport::solve::tests::two_patches()?;
        let reverse = ReverseLinks::build(&links)?;
        let index = CrossingIndex::build(&links);
        let mut graph = TraceGraph {
            media: vec![Vec3::ZERO; links.len()],
            generations: vec![1; links.tiles.len()],
            links,
            reverse,
            index,
            witnesses: RepairedWitnesses::default(),
            pending: BTreeMap::new(),
            redirects: BTreeMap::new(),
            original_sources: BTreeMap::new(),
            changed_instances: BTreeSet::new(),
            full_repair: false,
        };
        let original = graph.links.source[0];
        for source in [0, 1, 0, 1, original] {
            graph.redirect(0, source);
            let reference = ReverseLinks::build(&graph.links)?;
            for source in 0..graph.links.source_count() as u32 {
                let mut actual: Vec<_> = graph.edges(source).collect();
                actual.sort_unstable();
                assert_eq!(actual, reference.dependencies(source));
            }
        }
        assert!(graph.redirects.is_empty());
        assert!(graph.original_sources.is_empty());
        Ok(())
    }
}
