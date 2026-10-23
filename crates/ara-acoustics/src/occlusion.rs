//! Volumetric occlusion and material transmission from batched ray queries.
//!
//! Each source traces a center ray plus rays to Fibonacci-sphere points within its
//! occlusion radius. Visibility is the unblocked fraction (partial occlusion around
//! corners); transmission multiplies each distinct blocking surface's material.

use crate::{
    geometry::{RayHit, RayHits, RayScene, Segment},
    material::AcousticMaterial,
    math::{V3, add, distance, fibonacci, scale},
};
use ara_audio::{AudioError, Result};

/// One source/listener occlusion request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OcclusionQuery {
    /// Source position.
    pub source: V3,
    /// Listener position.
    pub listener: V3,
    /// Sampling radius around the source; zero traces only the center ray.
    pub radius: f32,
    /// Rays including the center ray, at least one.
    pub rays: u32,
}

/// A surface on a source's direct path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blocker {
    /// Surface identity (see `RayScene::label`).
    pub surface: u64,
    /// Its material.
    pub material: AcousticMaterial,
    /// Distance from the source along the path, in meters.
    pub distance: f32,
}

/// Occlusion result for one query.
#[derive(Debug, Clone, PartialEq)]
pub struct Occlusion {
    /// Fraction of unblocked rays, 0 (fully occluded) to 1 (clear).
    pub visibility: f32,
    /// Mean low/mid/high transmission of the blocked rays; zero when nothing blocks.
    pub transmission: [f32; 3],
    /// Distinct surfaces on the center ray (or the first blocked ray), nearest the source first.
    pub blockers: Vec<Blocker>,
}

impl Occlusion {
    /// An unoccluded result.
    pub fn clear() -> Self {
        Self {
            visibility: 1.0,
            transmission: [0.0; 3],
            blockers: Vec::new(),
        }
    }
}

/// Reusable batched solver; keeps ray and hit buffers between updates.
#[derive(Default)]
pub struct OcclusionSolver {
    segments: Vec<Segment>,
    hits: RayHits,
}

/// Distinct consecutive surfaces: entering and leaving one solid counts once.
fn distinct(hits: &[RayHit]) -> impl Iterator<Item = &RayHit> {
    hits.iter()
        .enumerate()
        .filter(|(i, h)| *i == 0 || hits[i - 1].surface != h.surface)
        .map(|(_, h)| h)
}

fn transmission(hits: &[RayHit]) -> [f32; 3] {
    distinct(hits).fold([1.0; 3], |t, h| {
        std::array::from_fn(|b| t[b] * h.material.transmission[b])
    })
}

impl OcclusionSolver {
    /// A solver with empty buffers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Solves every query with one `RayScene::trace` call; results match query order.
    pub fn solve_batch(
        &mut self,
        scene: &mut dyn RayScene,
        queries: &[OcclusionQuery],
        out: &mut Vec<Occlusion>,
    ) -> Result<()> {
        self.segments.clear();
        for q in queries {
            if q.rays == 0 || q.rays > 64 || !q.radius.is_finite() || q.radius < 0.0 {
                return Err(AudioError::Invalid("occlusion rays or radius"));
            }
            self.segments.push(Segment {
                from: q.listener,
                to: q.source,
            });
            let extra = if q.radius > 0.0 { q.rays - 1 } else { 0 };
            for i in 0..extra {
                let offset = scale(fibonacci(i as usize, extra as usize), q.radius);
                self.segments.push(Segment {
                    from: q.listener,
                    to: add(q.source, offset),
                });
            }
        }
        scene.trace(&self.segments, &mut self.hits)?;
        if self.hits.len() != self.segments.len() {
            return Err(AudioError::Backend(
                "ray scene returned the wrong number of hit lists".into(),
            ));
        }
        out.clear();
        let mut cursor = 0;
        for q in queries {
            let count = if q.radius > 0.0 { q.rays as usize } else { 1 };
            let mut blocked = 0;
            let mut sum = [0.0; 3];
            let mut first_blocked = None;
            for ray in cursor..cursor + count {
                let hits = self.hits.get(ray);
                if !hits.is_empty() {
                    blocked += 1;
                    let t = transmission(hits);
                    for b in 0..3 {
                        sum[b] += t[b];
                    }
                    first_blocked.get_or_insert(ray);
                }
            }
            let length = distance(q.listener, q.source);
            // Walk from the source end so each blocker reports its face nearest the source.
            let blockers = first_blocked.map_or_else(Vec::new, |ray| {
                let hits = self.hits.get(ray);
                hits.iter()
                    .rev()
                    .enumerate()
                    .filter(|(i, h)| *i == 0 || hits[hits.len() - i].surface != h.surface)
                    .map(|(_, h)| Blocker {
                        surface: h.surface,
                        material: h.material,
                        distance: (1.0 - h.fraction) * length,
                    })
                    .collect()
            });
            out.push(Occlusion {
                visibility: (count - blocked) as f32 / count as f32,
                transmission: if blocked == 0 {
                    [0.0; 3]
                } else {
                    sum.map(|s| s / blocked as f32)
                },
                blockers,
            });
            cursor += count;
        }
        Ok(())
    }

    /// Solves one query.
    pub fn solve(&mut self, scene: &mut dyn RayScene, query: OcclusionQuery) -> Result<Occlusion> {
        let mut out = Vec::with_capacity(1);
        self.solve_batch(scene, &[query], &mut out)?;
        out.pop()
            .ok_or_else(|| AudioError::Backend("occlusion produced no result".into()))
    }
}
