use super::{FeatureWriter, TrajectoryPoint};
use crate::{CookedClip, Error, Pose, RootMotion, Sampler, Skeleton, ensure, signature};
use ara_schema::{AssetRef, Schema};
use glam::{Vec3, Vec4};
use serde::{Deserialize, Serialize};

/// Cook-time feature schema. Feature order is six floats per bone (position/velocity),
/// three root-velocity floats, then six floats per horizon (position/facing).
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct MatchConfig {
    /// Ordered canonical source clip references; empty accepts importer-supplied bindings.
    #[ara(asset_kind = "animation")]
    pub clips: Vec<AssetRef>,
    /// Ordered stable joint IDs used for pose position and velocity features.
    pub bones: Vec<String>,
    /// Increasing future root sample times in source clip ticks.
    pub horizons: Vec<u32>,
    /// Cook-time sampling interval in source ticks.
    #[ara(default = 800)]
    pub step: u32,
    /// Normalized position-feature cost weight.
    #[ara(default = 1)]
    pub position_weight: f32,
    /// Normalized bone-velocity cost weight.
    #[ara(default = 1)]
    pub velocity_weight: f32,
    /// Normalized root-velocity and future-trajectory cost weight.
    #[ara(default = 2)]
    pub trajectory_weight: f32,
}
impl Default for MatchConfig {
    fn default() -> Self {
        Self {
            clips: Vec::new(),
            bones: Vec::new(),
            horizons: vec![9600, 19200, 28800],
            step: 800,
            position_weight: 1.0,
            velocity_weight: 1.0,
            trajectory_weight: 2.0,
        }
    }
}
impl MatchConfig {
    /// Raw scalar feature count, excluding zero SIMD padding.
    pub fn dimensions(&self) -> usize {
        self.bones.len() * 6 + 3 + self.horizons.len() * 6
    }
}
/// Database row's source animation location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchFrame {
    /// Binding index in MotionDatabase::clip_references order.
    pub clip: usize,
    /// Source local clip time, before its duration endpoint.
    pub tick: u32,
}
/// Deterministic nearest candidate and explicit work/accuracy result.
#[derive(Debug, Clone, Copy)]
pub struct SearchResult {
    /// Stable row index; ties choose the first row.
    pub row: usize,
    /// Squared normalized weighted distance.
    pub cost: f32,
    /// Candidate rows examined.
    pub visited: usize,
    /// True only when every row was examined.
    pub exact: bool,
}
/// Immutable CPU feature database; normalization and SIMD packing happen during cooking.
#[derive(Debug, Serialize)]
pub struct MotionDatabase {
    pub(crate) skeleton: u64,
    pub(crate) signature: u64,
    pub(crate) references: Vec<AssetRef>,
    pub(crate) clips: Vec<u64>,
    pub(crate) frequency: u32,
    config: MatchConfig,
    frames: Vec<MatchFrame>,
    mean: Vec<Vec4>,
    factor: Vec<Vec4>,
    features: Vec<Vec4>,
}
#[derive(Deserialize)]
#[serde(remote = "MotionDatabase")]
struct DatabaseWire {
    skeleton: u64,
    signature: u64,
    references: Vec<AssetRef>,
    clips: Vec<u64>,
    frequency: u32,
    config: MatchConfig,
    frames: Vec<MatchFrame>,
    mean: Vec<Vec4>,
    factor: Vec<Vec4>,
    features: Vec<Vec4>,
}
#[derive(Deserialize)]
struct DatabaseBlob(#[serde(with = "DatabaseWire")] MotionDatabase);
impl MotionDatabase {
    /// Writes a versioned derived binary cache with a content checksum. Source
    /// configuration remains canonical text; this blob belongs in the asset cache.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let payload = postcard::to_allocvec(self).map_err(|e| Error(e.to_string()))?;
        let mut out = b"ARAMM001".to_vec();
        out.extend(crate::signature(&payload).to_le_bytes());
        out.extend(payload);
        Ok(out)
    }
    /// Loads a checked derived cache for the exact skeleton generation.
    pub fn from_bytes(bytes: &[u8], skeleton: &Skeleton) -> Result<Self, Error> {
        ensure(
            bytes.len() >= 16 && &bytes[..8] == b"ARAMM001",
            "unsupported motion database cache",
        )?;
        let expected = u64::from_le_bytes(
            bytes[8..16]
                .try_into()
                .expect("header has eight checksum bytes"),
        );
        ensure(
            crate::signature(&bytes[16..]) == expected,
            "motion database checksum mismatch",
        )?;
        let DatabaseBlob(out) =
            postcard::from_bytes(&bytes[16..]).map_err(|e| Error(e.to_string()))?;
        let groups = out.config.dimensions().div_ceil(4);
        ensure(
            out.skeleton == skeleton.signature()
                && out.frequency > 0
                && out.config.step > 0
                && !out.frames.is_empty()
                && !out.references.is_empty()
                && out.references.len() == out.clips.len()
                && out.config.dimensions() <= 4096
                && out.mean.len() == groups
                && out.factor.len() == groups
                && out.frames.len().checked_mul(groups) == Some(out.features.len())
                && out.frames.iter().all(|r| r.clip < out.clips.len())
                && out
                    .mean
                    .iter()
                    .chain(&out.factor)
                    .chain(&out.features)
                    .all(|v| v.is_finite() && v.abs().max_element() < 1e15)
                && out.factor.iter().all(|v| v.min_element() >= 0.0),
            "invalid motion database dimensions or content",
        )?;
        FeatureWriter::new(&out.config, skeleton)?;
        Ok(out)
    }
    /// Samples clips and builds normalized pose/velocity/future-root features offline.
    /// Clip motion is looped through SE(3), including horizons beyond a clip boundary.
    pub fn cook(
        config: &MatchConfig,
        skeleton: &Skeleton,
        clips: &[(AssetRef, &CookedClip)],
    ) -> Result<Self, Error> {
        ensure(
            !clips.is_empty()
                && config.step > 0
                && config.dimensions() <= 4096
                && config.horizons.windows(2).all(|w| w[0] < w[1])
                && config.horizons.iter().all(|t| *t > 0)
                && [
                    config.position_weight,
                    config.velocity_weight,
                    config.trajectory_weight,
                ]
                .iter()
                .all(|w| w.is_finite() && *w >= 0.0)
                && config.position_weight + config.velocity_weight + config.trajectory_weight > 0.0,
            "invalid motion-matching configuration",
        )?;
        let frequency = clips[0].1.frequency;
        ensure(
            config.clips.is_empty() || config.clips.iter().eq(clips.iter().map(|(r, _)| r)),
            "motion source bindings differ from the canonical configuration",
        )?;
        ensure(
            clips
                .iter()
                .all(|(_, c)| c.skeleton == skeleton.signature() && c.frequency == frequency),
            "motion database clip skeleton/timebase mismatch",
        )?;
        let mut writer = FeatureWriter::new(config, skeleton)?;
        let dimensions = config.dimensions();
        let groups = dimensions.div_ceil(4);
        let mut rows = Vec::<Vec<f32>>::new();
        let mut frames = Vec::new();
        let mut pose = Pose::new(skeleton.len());
        let mut previous = Pose::new(skeleton.len());
        let mut trajectory = vec![TrajectoryPoint::default(); config.horizons.len()];
        for (index, (_, clip)) in clips.iter().enumerate() {
            ensure(
                clip.duration >= config.step,
                "motion database clips must contain one sampling interval",
            )?;
            let mut sampler = Sampler::new(clip);
            let mut curves = vec![0.0; clip.curve_count()];
            for tick in (0..clip.duration).step_by(config.step as usize) {
                // Forward difference at tick zero avoids sampling across a pose discontinuity.
                let previous_tick = tick.saturating_sub(config.step);
                let current_tick = if tick == 0 { config.step } else { tick };
                sampler.sample(clip, skeleton, previous_tick, &mut previous, &mut curves)?;
                sampler.sample(clip, skeleton, current_tick, &mut pose, &mut curves)?;
                let before = extended_root(clip, skeleton, u64::from(previous_tick));
                let now = extended_root(clip, skeleton, u64::from(tick));
                let after = extended_root(clip, skeleton, u64::from(current_tick));
                let dt = config.step as f32 / frequency as f32;
                let velocity =
                    before.rotation.conjugate() * (after.translation - before.translation) / dt;
                for (out, horizon) in trajectory.iter_mut().zip(&config.horizons) {
                    let future =
                        extended_root(clip, skeleton, u64::from(tick) + u64::from(*horizon));
                    let delta = now.inverse().compose(future);
                    *out = TrajectoryPoint {
                        position: delta.translation,
                        facing: delta.rotation * Vec3::Z,
                    };
                }
                let mut raw = vec![0.0; dimensions];
                writer.write(
                    skeleton,
                    &pose,
                    &previous,
                    dt,
                    velocity,
                    &trajectory,
                    &mut raw,
                )?;
                // Position should describe this row, not the forward-difference endpoint.
                if tick == 0 {
                    let mut start = vec![0.0; dimensions];
                    writer.write(
                        skeleton,
                        &previous,
                        &previous,
                        dt,
                        velocity,
                        &trajectory,
                        &mut start,
                    )?;
                    for bone in 0..config.bones.len() {
                        raw[bone * 6..bone * 6 + 3].copy_from_slice(&start[bone * 6..bone * 6 + 3]);
                    }
                }
                rows.push(raw);
                frames.push(MatchFrame { clip: index, tick });
            }
        }
        let mut mean = vec![Vec4::ZERO; groups];
        let mut factor = vec![Vec4::ZERO; groups];
        for d in 0..dimensions {
            let average = rows.iter().map(|r| f64::from(r[d])).sum::<f64>() / rows.len() as f64;
            let variance = rows
                .iter()
                .map(|r| (f64::from(r[d]) - average).powi(2))
                .sum::<f64>()
                / rows.len() as f64;
            let weight = if d >= config.bones.len() * 6 {
                config.trajectory_weight
            } else if d % 6 < 3 {
                config.position_weight
            } else {
                config.velocity_weight
            };
            mean[d / 4][d % 4] = average as f32;
            factor[d / 4][d % 4] = weight.sqrt() / (variance.sqrt() as f32).max(0.01);
        }
        let mut features = vec![Vec4::ZERO; rows.len() * groups];
        for (r, raw) in rows.iter().enumerate() {
            for d in 0..dimensions {
                features[r * groups + d / 4][d % 4] =
                    (raw[d] - mean[d / 4][d % 4]) * factor[d / 4][d % 4];
            }
        }
        ensure(
            features.iter().all(|v| v.is_finite()),
            "motion features overflow during normalization",
        )?;
        let mut bytes = postcard::to_allocvec(config).map_err(|e| Error(e.to_string()))?;
        for (_, clip) in clips {
            bytes.extend(clip.signature.to_le_bytes());
        }
        Ok(Self {
            skeleton: skeleton.signature(),
            signature: signature(&bytes),
            references: clips.iter().map(|(r, _)| r.clone()).collect(),
            clips: clips.iter().map(|(_, c)| c.signature).collect(),
            frequency,
            config: config.clone(),
            frames,
            mean,
            factor,
            features,
        })
    }
    /// Raw schema used at cook time.
    pub fn config(&self) -> &MatchConfig {
        &self.config
    }
    /// Canonical referenced clips, in row binding order.
    pub fn clip_references(&self) -> &[AssetRef] {
        &self.references
    }
    /// Indexed source frame; none for an invalid row.
    pub fn frame(&self, row: usize) -> Option<MatchFrame> {
        self.frames.get(row).copied()
    }
    /// Number of searchable sampled frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    /// Whether the database contains no frames (never true after cooking).
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    /// Number of SIMD vectors required for normalized query storage.
    pub fn query_groups(&self) -> usize {
        self.mean.len()
    }
    /// Resident feature payload in bytes, including zero SIMD padding.
    pub fn feature_bytes(&self) -> usize {
        self.features.len() * std::mem::size_of::<Vec4>()
    }
    /// Normalizes a raw query into caller-owned SIMD storage.
    pub fn normalize(&self, raw: &[f32], out: &mut [Vec4]) -> Result<(), Error> {
        ensure(
            raw.len() == self.config.dimensions()
                && out.len() == self.mean.len()
                && raw.iter().all(|v| v.is_finite()),
            "invalid motion query",
        )?;
        out.fill(Vec4::ZERO);
        for (d, value) in raw.iter().enumerate() {
            out[d / 4][d % 4] = (*value - self.mean[d / 4][d % 4]) * self.factor[d / 4][d % 4];
        }
        ensure(
            out.iter()
                .all(|v| v.is_finite() && v.abs().max_element() < 1e15),
            "normalized motion query exceeds finite cost range",
        )
    }
    /// Bounded SIMD brute-force search. A partial budget samples evenly across all clips.
    /// Zero means full search; exactness and examined count are returned explicitly.
    pub fn search(&self, normalized: &[Vec4], budget: usize) -> Result<SearchResult, Error> {
        ensure(
            normalized.len() == self.mean.len()
                && normalized
                    .iter()
                    .all(|v| v.is_finite() && v.abs().max_element() < 1e15),
            "invalid normalized query",
        )?;
        let count = if budget == 0 {
            self.len()
        } else {
            budget.min(self.len())
        };
        let mut best = SearchResult {
            row: 0,
            cost: f32::INFINITY,
            visited: count,
            exact: count == self.len(),
        };
        let groups = self.mean.len();
        for i in 0..count {
            let row = i * self.len() / count;
            let feature = &self.features[row * groups..(row + 1) * groups];
            let mut cost = 0.0;
            for (a, b) in feature.iter().zip(normalized) {
                let delta = *a - *b;
                cost += delta.dot(delta);
            }
            if cost < best.cost {
                best.row = row;
                best.cost = cost;
            }
        }
        Ok(best)
    }
    /// Borrows a normalized row for tooling and exact-search validation.
    pub fn normalized_row(&self, row: usize) -> Option<&[Vec4]> {
        let n = self.mean.len();
        self.features
            .get(row.checked_mul(n)?..row.checked_add(1)?.checked_mul(n)?)
    }
}
fn extended_root(clip: &CookedClip, skeleton: &Skeleton, tick: u64) -> RootMotion {
    let duration = u64::from(clip.duration);
    let (start, cycle) = clip.root_cache;
    start
        .compose(cycle.pow(tick / duration))
        .compose(RootMotion::between(
            start,
            clip.root_at(skeleton, (tick % duration) as f64),
        ))
}
