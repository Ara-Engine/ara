use crate::{
    Error, Skeleton,
    data::{ClipData, Interpolation, Notify},
    ensure,
    quantization::Packed,
    signature,
};
use ara_schema::FieldPath;
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Maximum local-space component errors accepted by the offline cooker.
#[derive(Debug, Clone, Copy)]
pub struct CompressionSettings {
    /// Translation Euclidean error, in meters.
    pub translation: f32,
    /// Rotation geodesic error, in radians.
    pub rotation: f32,
    /// Scale Euclidean error.
    pub scale: f32,
    /// Absolute scalar curve error.
    pub curve: f32,
}
impl Default for CompressionSettings {
    fn default() -> Self {
        Self {
            translation: 0.0001,
            rotation: 0.0005,
            scale: 0.0001,
            curve: 0.0001,
        }
    }
}

/// Offline compression measurements; bounds apply locally, not to accumulated tips.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CompressionReport {
    /// Float32 source key payload plus uint32 timestamps, before constant reduction.
    pub source_bytes: usize,
    /// Complete versioned cooked blob including times, ranges, curves and notifies.
    pub cooked_bytes: usize,
    /// Largest measured translation error over keys and 16 subdivisions per interval.
    pub translation_error: f32,
    /// Largest measured quaternion geodesic error in radians.
    pub rotation_error: f32,
    /// Largest measured scale error.
    pub scale_error: f32,
    /// Largest measured scalar error.
    pub curve_error: f32,
    /// Conservative all-time translation quantization bound.
    pub translation_bound: f32,
    /// Conservative all-time quaternion quantization bound in radians.
    pub rotation_bound: f32,
    /// Conservative all-time scale quantization bound.
    pub scale_bound: f32,
    /// Conservative all-time scalar quantization bound.
    pub curve_bound: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Track {
    pub translation: Option<Packed>,
    pub rotation: Option<Packed>,
    pub scale: Option<Packed>,
}
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PackedCurve {
    pub property: String,
    pub track: Packed,
}

/// An immutable, checked, bit-packed clip. Decode only with `from_bytes`.
#[derive(Debug)]
pub struct CookedClip {
    pub(crate) root_cache: (crate::RootMotion, crate::RootMotion),
    pub(crate) plan: crate::sampling_plan::SamplingPlan,
    pub(crate) duration: u32,
    pub(crate) frequency: u32,
    pub(crate) skeleton: u64,
    pub(crate) joints: usize,
    pub(crate) root_track: Track,
    encoded: Vec<u8>,
    pub(crate) curves: Vec<PackedCurve>,
    pub(crate) notifies: Vec<Notify>,
    pub(crate) signature: u64,
}
#[derive(Serialize, Deserialize)]
struct Blob {
    duration: u32,
    frequency: u32,
    skeleton: u64,
    tracks: Vec<Track>,
    curves: Vec<PackedCurve>,
    notifies: Vec<Notify>,
}
impl CookedClip {
    /// Validates and compresses source keys. Run in an importer worker.
    pub fn cook(
        data: &ClipData,
        skeleton: &Skeleton,
        settings: CompressionSettings,
    ) -> Result<(Self, CompressionReport), Error> {
        ensure(
            data.duration > 0 && data.ticks_per_second > 0 && data.tracks.len() == skeleton.len(),
            "invalid clip duration, frequency or skeleton track count",
        )?;
        ensure(
            [
                settings.translation,
                settings.rotation,
                settings.scale,
                settings.curve,
            ]
            .iter()
            .all(|e| e.is_finite() && *e > 0.0),
            "compression tolerances must be positive and finite",
        )?;
        let mut report = CompressionReport::default();
        let mut tracks = Vec::with_capacity(skeleton.len());
        for (joint, raw) in data.tracks.iter().enumerate() {
            let translation = pack(
                raw.translations
                    .iter()
                    .map(|k| (k.tick, [k.value[0], k.value[1], k.value[2], 0.0]))
                    .collect(),
                3,
                raw.translation_interpolation,
                settings.translation,
                data.duration,
                false,
                &mut report.translation_error,
                &mut report.translation_bound,
                &mut report.source_bytes,
            )?;
            for key in &raw.rotations {
                ensure(
                    Quat::from_array(key.value).is_normalized(),
                    "rotation keys must be unit quaternions",
                )?;
            }
            let mut keys = Vec::with_capacity(raw.rotations.len());
            let mut previous = Quat::IDENTITY;
            let mut near_half_turn = false;
            for (i, key) in raw.rotations.iter().enumerate() {
                let mut q = Quat::from_array(key.value).normalize();
                if i != 0 {
                    near_half_turn |= previous.dot(q).abs() < 0.001;
                    if previous.dot(q) < 0.0 {
                        q = -q;
                    }
                }
                keys.push((key.tick, q.to_array()));
                previous = q;
            }
            // Half-turn ties must not change interpolation hemisphere after quantizing.
            let budget = if near_half_turn {
                f32::MIN_POSITIVE
            } else {
                settings.rotation / 32.0
            };
            let rotation = pack(
                keys,
                4,
                raw.rotation_interpolation,
                budget,
                data.duration,
                true,
                &mut report.rotation_error,
                &mut report.rotation_bound,
                &mut report.source_bytes,
            )?;
            ensure(
                raw.scales.iter().all(|k| k.value.iter().all(|v| *v > 0.0)),
                "scale keys must be positive",
            )?;
            let scale = pack(
                raw.scales
                    .iter()
                    .map(|k| (k.tick, [k.value[0], k.value[1], k.value[2], 0.0]))
                    .collect(),
                3,
                raw.scale_interpolation,
                settings.scale,
                data.duration,
                false,
                &mut report.scale_error,
                &mut report.scale_bound,
                &mut report.source_bytes,
            )?;
            let mut track = Track {
                translation,
                rotation,
                scale,
            };
            let bind = skeleton.bind.joint(joint).expect("source tracks match rig");
            let defaults = [
                [
                    bind.translation.x,
                    bind.translation.y,
                    bind.translation.z,
                    0.0,
                ],
                bind.rotation.to_array(),
                [bind.scale.x, bind.scale.y, bind.scale.z, 0.0],
            ];
            for (channel, value) in [
                &mut track.translation,
                &mut track.rotation,
                &mut track.scale,
            ]
            .into_iter()
            .zip(defaults)
            {
                if channel
                    .as_ref()
                    .is_some_and(|t| t.times.len() == 1 && t.decode(0) == value)
                {
                    *channel = None;
                }
            }
            tracks.push(track);
        }
        let mut properties = BTreeSet::new();
        let mut curves = Vec::new();
        for curve in &data.curves {
            ensure(
                curve.property.parse::<FieldPath>().is_ok() && properties.insert(&curve.property),
                "curve property paths must be valid and unique",
            )?;
            let track = pack(
                curve
                    .keys
                    .iter()
                    .map(|k| (k.tick, [k.value, 0.0, 0.0, 0.0]))
                    .collect(),
                1,
                curve.interpolation,
                settings.curve,
                data.duration,
                false,
                &mut report.curve_error,
                &mut report.curve_bound,
                &mut report.source_bytes,
            )?
            .ok_or_else(|| Error("curve needs at least one key".into()))?;
            curves.push(PackedCurve {
                property: curve.property.clone(),
                track,
            });
        }
        let blob = Blob {
            duration: data.duration,
            frequency: data.ticks_per_second,
            skeleton: skeleton.signature(),
            tracks,
            curves,
            notifies: data.notifies.clone(),
        };
        Self::validate(&blob, skeleton)?;
        let mut bytes = b"ARAANM\x02\0".to_vec();
        bytes.extend(postcard::to_allocvec(&blob).map_err(|e| Error(e.to_string()))?);
        ensure(bytes.len() <= 64 * 1024 * 1024, "oversized animation cache")?;
        report.cooked_bytes = bytes.len();
        let result = Self::from_blob(blob, bytes, skeleton);
        crate::measure::runtime(data, &result, skeleton, &mut report)?;
        ensure(
            report.translation_error <= settings.translation
                && report.rotation_error <= settings.rotation
                && report.scale_error <= settings.scale
                && report.curve_error <= settings.curve,
            "measured compression error exceeds requested tolerance",
        )?;
        ensure(
            report.translation_bound <= settings.translation
                && report.rotation_bound <= settings.rotation
                && report.scale_bound <= settings.scale
                && report.curve_bound <= settings.curve,
            "compression tolerance is below the floating-point error bound",
        )?;
        Ok((result, report))
    }
    /// Encodes cache format version 2; never used as an authoring document.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        Ok(self.encoded.to_vec())
    }
    /// Decodes and checks all packed metadata before any hot-path access.
    pub fn from_bytes(bytes: &[u8], skeleton: &Skeleton) -> Result<Self, Error> {
        ensure(
            bytes.starts_with(b"ARAANM\x02\0") && bytes.len() <= 64 * 1024 * 1024,
            "unsupported or oversized animation cache",
        )?;
        let (blob, trailing): (Blob, _) =
            postcard::take_from_bytes(&bytes[8..]).map_err(|e| Error(e.to_string()))?;
        ensure(trailing.is_empty(), "trailing animation cache data")?;
        Self::validate(&blob, skeleton)?;
        Ok(Self::from_blob(blob, bytes.to_vec(), skeleton))
    }
    /// Builds derived packets, then releases the track-major cooker representation.
    fn from_blob(mut blob: Blob, encoded: Vec<u8>, skeleton: &Skeleton) -> Self {
        let plan = crate::sampling_plan::SamplingPlan::new(&blob.tracks, skeleton);
        let joints = blob.tracks.len();
        let root_track = blob.tracks.swap_remove(skeleton.data.motion_root as usize);
        let mut clip = Self {
            root_cache: Default::default(),
            plan,
            duration: blob.duration,
            frequency: blob.frequency,
            skeleton: blob.skeleton,
            joints,
            root_track,
            signature: signature(&encoded),
            encoded,
            curves: blob.curves,
            notifies: blob.notifies,
        };
        clip.prepare_root(skeleton);
        clip
    }
    fn validate(blob: &Blob, skeleton: &Skeleton) -> Result<(), Error> {
        ensure(
            blob.duration > 0
                && blob.frequency > 0
                && blob.skeleton == skeleton.signature()
                && blob.tracks.len() == skeleton.len(),
            "cooked clip skeleton or timing mismatch",
        )?;
        for track in &blob.tracks {
            if let Some(t) = &track.translation {
                t.validate(blob.duration, 3)?;
            }
            if let Some(t) = &track.rotation {
                t.validate(blob.duration, 4)?;
                for i in 0..t.times.len() {
                    ensure(
                        (0.99..=1.01).contains(&Quat::from_array(t.decode(i)).length_squared()),
                        "invalid cooked quaternion",
                    )?;
                }
            }
            if let Some(t) = &track.scale {
                t.validate(blob.duration, 3)?;
                for i in 0..t.times.len() {
                    ensure(
                        t.decode(i)[..3].iter().all(|v| *v > 0.0),
                        "invalid cooked scale",
                    )?;
                }
            }
        }
        let mut properties = BTreeSet::new();
        for curve in &blob.curves {
            ensure(
                curve.property.parse::<FieldPath>().is_ok() && properties.insert(&curve.property),
                "invalid cooked property path",
            )?;
            curve.track.validate(blob.duration, 1)?;
        }
        let mut ids = BTreeSet::new();
        ensure(
            blob.notifies.windows(2).all(|w| w[0].tick <= w[1].tick)
                && blob
                    .notifies
                    .iter()
                    .all(|n| n.tick < blob.duration && !n.id.is_empty() && ids.insert(&n.id)),
            "invalid or duplicate clip notifies",
        )
    }
    fn prepare_root(&mut self, skeleton: &Skeleton) {
        let start = self.root_at(skeleton, 0.0);
        self.root_cache = (
            start,
            crate::RootMotion::between(start, self.root_at(skeleton, f64::from(self.duration))),
        );
    }
    /// Clip duration in integer ticks.
    pub fn duration(&self) -> u32 {
        self.duration
    }
    /// Required fixed-clock frequency.
    pub fn ticks_per_second(&self) -> u32 {
        self.frequency
    }
    /// Number of scalar property curves.
    pub fn curve_count(&self) -> usize {
        self.curves.len()
    }
    /// Canonical property path for a scalar curve.
    pub fn curve_property(&self, index: usize) -> Option<&str> {
        self.curves.get(index).map(|c| c.property.as_str())
    }
    /// Immutable clip notifies in delivery order.
    pub fn notifies(&self) -> &[Notify] {
        &self.notifies
    }
    /// Owned runtime storage, including packet caches and metadata, excluding
    /// allocator bookkeeping and the shared skeleton. No float key expansion.
    pub fn runtime_bytes(&self) -> usize {
        let tracks = [
            &self.root_track.translation,
            &self.root_track.rotation,
            &self.root_track.scale,
        ]
        .into_iter()
        .flatten()
        .map(Packed::storage_bytes)
        .sum::<usize>();
        let curves = self
            .curves
            .iter()
            .map(|c| c.property.capacity() + c.track.storage_bytes())
            .sum::<usize>();
        let notifies = self
            .notifies
            .iter()
            .map(|n| n.id.capacity() + n.name.capacity() + n.payload.capacity())
            .sum::<usize>();
        std::mem::size_of::<Self>()
            + self.encoded.capacity()
            + self.curves.capacity() * std::mem::size_of::<PackedCurve>()
            + self.notifies.capacity() * std::mem::size_of::<Notify>()
            + tracks
            + curves
            + notifies
            + self.plan.storage_bytes()
    }
}

#[allow(clippy::too_many_arguments)] // Shared cooker for vector/quaternion/property tracks.
fn pack(
    keys: Vec<(u32, [f32; 4])>,
    dimensions: u8,
    interpolation: Interpolation,
    budget: f32,
    duration: u32,
    rotation: bool,
    measured: &mut f32,
    bound: &mut f32,
    source_bytes: &mut usize,
) -> Result<Option<Packed>, Error> {
    if keys.is_empty() {
        return Ok(None);
    }
    ensure(
        keys.iter()
            .all(|k| k.0 <= duration && k.1.iter().all(|v| v.is_finite()))
            && keys.windows(2).all(|w| w[0].0 < w[1].0),
        "keys must be finite, ordered and within clip duration",
    )?;
    *source_bytes += keys.len() * (4 + usize::from(dimensions) * 4);
    let track = Packed::cook(
        keys.clone(),
        dimensions,
        interpolation,
        budget,
        rotation && budget > f32::MIN_POSITIVE,
    );
    let e = track.bound();
    // For unit quaternions, nlerp's denominator is >= 1/sqrt(2). Endpoint
    // perturbation and final normalization give a conservative angular bound.
    *bound = bound.max(if rotation {
        (16.0 * (2.0 * e).min(1.0).asin()).max(2e-6)
    } else {
        e
    });
    for index in 0..keys.len() {
        let next = (index + 1).min(keys.len() - 1);
        for sub in 0..=16 {
            let t = sub as f32 / 16.0;
            let tick =
                keys[index].0 as f64 + f64::from(t) * f64::from(keys[next].0 - keys[index].0);
            let a = reference(
                &keys[index].1,
                &keys[next].1,
                if interpolation == Interpolation::Step && sub < 16 {
                    0.0
                } else {
                    t
                },
                rotation,
            );
            let b = crate::sampling::sample_uncached(&track, tick, rotation);
            let error = if rotation {
                rotation_distance(Quat::from_array(a), Quat::from_array(b))
            } else if dimensions == 1 {
                (a[0] - b[0]).abs()
            } else {
                Vec3::from_slice(&a).distance(Vec3::from_slice(&b))
            };
            *measured = measured.max(error);
        }
    }
    Ok(Some(track))
}
pub(crate) fn reference(a: &[f32; 4], b: &[f32; 4], t: f32, rotation: bool) -> [f32; 4] {
    if rotation {
        crate::pose::nlerp(
            Quat::from_array(*a).normalize(),
            Quat::from_array(*b).normalize(),
            t,
        )
        .to_array()
    } else {
        std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
    }
}
pub(crate) fn rotation_distance(a: Quat, b: Quat) -> f32 {
    4.0 * (((a - b).length().min((a + b).length()) * 0.5).min(1.0)).asin()
}
