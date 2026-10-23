//! Spatial geometry is evaluated off the render path, then smoothly applied by a voice.

use crate::{
    AudioError, AudioListener, AudioSource, Frame, Result, dsp::BandSplit, schema::Attenuation,
};

/// Acoustic simulation result. Physics/Steam workers publish this at a frame boundary.
#[derive(
    Debug, Clone, Copy, PartialEq, ara_schema::Schema, serde::Serialize, serde::Deserialize,
)]
#[ara(module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct AcousticPath {
    /// Direct path visibility: zero is fully occluded, one is clear.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub visibility: f32,
    /// Low/mid/high band transmission through occluders.
    #[ara(range(min = 0, max = 1))]
    pub transmission: [f32; 3],
    /// Per-voice room reverb send amplitude into the bus's routed aux return.
    #[ara(range(min = 0, max = 1))]
    pub reverb_send: f32,
    /// Low/mid/high air absorption gain over the propagation distance; one is none.
    #[ara(default = [1, 1, 1], range(min = 0, max = 1))]
    #[serde(default = "unit_bands")]
    pub air: [f32; 3],
}

fn unit_bands() -> [f32; 3] {
    [1.0; 3]
}

impl Default for AcousticPath {
    fn default() -> Self {
        Self {
            visibility: 1.0,
            transmission: [0.0; 3],
            reverb_send: 0.0,
            air: unit_bands(),
        }
    }
}

impl AcousticPath {
    /// Final low/mid/high direct-path gains: air × (visibility + occluded × transmission).
    pub fn band_gains(&self) -> [f32; 3] {
        std::array::from_fn(|b| {
            self.air[b] * (self.visibility + (1.0 - self.visibility) * self.transmission[b])
        })
    }
}

/// Worker-side geometry seam; implementations must never run ray tracing on the audio thread.
pub trait AcousticScene {
    /// Computes occlusion, transmission and reflection send for a source/listener pair.
    fn query(&mut self, source: [f32; 3], listener: [f32; 3]) -> Result<AcousticPath>;
}

/// A prepared spatial update, in the listener's right/up/back coordinate system.
#[derive(Debug, Clone, Copy)]
pub struct SpatialParameters {
    /// Listener-local unit direction. Forward has negative Z.
    pub direction: [f32; 3],
    /// Gain including distance and source/listener cones, excluding source gain.
    pub attenuation: f32,
    /// Doppler playback-rate multiplier, clamped to [0.25, 4].
    pub doppler: f32,
}

/// Computes equal-power panning, finite-range rolloff, both cones and Doppler.
pub fn evaluate(source: &AudioSource, listener: &AudioListener) -> Result<SpatialParameters> {
    let scalars = [
        source.gain,
        source.pitch,
        source.reference_distance,
        source.max_distance,
        source.cone_inner,
        source.cone_outer,
        source.cone_outer_gain,
        listener.cone_inner,
        listener.cone_outer,
        listener.cone_outer_gain,
    ];
    if scalars.iter().any(|v| !v.is_finite())
        || source.reference_distance <= 0.0
        || source.max_distance <= source.reference_distance
        || source.pitch <= 0.0
        || source.gain < 0.0
        || !valid_cone(source.cone_inner, source.cone_outer, source.cone_outer_gain)
        || !valid_cone(
            listener.cone_inner,
            listener.cone_outer,
            listener.cone_outer_gain,
        )
    {
        return Err(AudioError::Invalid("spatial ranges, pitch or cone"));
    }
    for vector in [
        source.position,
        source.velocity,
        source.forward,
        listener.position,
        listener.velocity,
        listener.forward,
        listener.up,
    ] {
        if vector.iter().any(|v| !v.is_finite()) {
            return Err(AudioError::Invalid("finite spatial vectors required"));
        }
    }
    let forward = normalize(listener.forward)?;
    let right = normalize(cross(forward, listener.up))?;
    let up = cross(right, forward);
    let delta = sub(source.position, listener.position);
    let distance = dot(delta, delta).sqrt();
    if !distance.is_finite() {
        return Err(AudioError::Invalid(
            "spatial coordinates exceed supported magnitude",
        ));
    }
    let direction = if distance > 0.00001 {
        scale(delta, 1.0 / distance)
    } else {
        forward
    };
    let mut attenuation = if distance >= source.max_distance {
        0.0
    } else {
        let d = distance.max(source.reference_distance);
        let fade = ((source.max_distance - d) / (source.max_distance - source.reference_distance))
            .clamp(0.0, 1.0);
        match source.attenuation {
            Attenuation::Linear => fade,
            Attenuation::Inverse => source.reference_distance / d * fade,
            Attenuation::InverseSquare => (source.reference_distance / d).powi(2) * fade,
        }
    };
    attenuation *= cone(
        normalize(source.forward)?,
        scale(direction, -1.0),
        source.cone_inner,
        source.cone_outer,
        source.cone_outer_gain,
    );
    attenuation *= cone(
        forward,
        direction,
        listener.cone_inner,
        listener.cone_outer,
        listener.cone_outer_gain,
    );
    let speed = 343.0;
    let listener_toward = dot(listener.velocity, direction).clamp(-speed * 0.9, speed * 0.9);
    let source_away = dot(source.velocity, direction).clamp(-speed * 0.9, speed * 0.9);
    Ok(SpatialParameters {
        direction: [
            dot(direction, right),
            dot(direction, up),
            -dot(direction, forward),
        ],
        attenuation: if source.spatial { attenuation } else { 1.0 },
        doppler: if source.spatial {
            ((speed + listener_toward) / (speed + source_away)).clamp(0.25, 4.0)
        } else {
            1.0
        },
    })
}

fn valid_cone(inner: f32, outer: f32, gain: f32) -> bool {
    (0.0..=360.0).contains(&inner)
        && (inner..=360.0).contains(&outer)
        && (0.0..=1.0).contains(&gain)
}

fn cone(forward: [f32; 3], direction: [f32; 3], inner: f32, outer: f32, gain: f32) -> f32 {
    let angle = dot(forward, direction).clamp(-1.0, 1.0).acos().to_degrees() * 2.0;
    if angle <= inner {
        1.0
    } else if angle >= outer {
        gain
    } else {
        1.0 + (gain - 1.0) * ((angle - inner) / (outer - inner))
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(a: [f32; 3]) -> Result<[f32; 3]> {
    let len = dot(a, a).sqrt();
    if !len.is_finite() || len < 0.00001 {
        return Err(AudioError::Invalid("zero or parallel spatial axes"));
    }
    Ok(scale(a, 1.0 / len))
}

/// Replaceable per-voice renderer. Construction may allocate; processing must not.
pub trait Spatializer: Send {
    /// Called once at installation with the engine's sample rate, on the control path.
    fn prepare(&mut self, _sample_rate: u32) {}
    /// Converts a mono quantum to stereo. Directions may change once per quantum.
    fn process(&mut self, mono: &[f32], out: &mut [Frame], direction: [f32; 3], path: AcousticPath);
    /// Renders the next tail quantum after dry input ends. True means the tail is complete.
    fn drain(&mut self, out: &mut [Frame]) -> bool {
        out.fill(Frame::ZERO);
        true
    }
}

/// Equal-power stereo fallback with three-band occlusion, transmission and air absorption.
///
/// Clear paths take a vectorized multiply. Otherwise critically damped SVF crossovers
/// (800 Hz and 8 kHz, `dsp::BandSplit`) split the signal into low/mid/high bands whose
/// gains ramp linearly per quantum to `AcousticPath::band_gains`. Equal band gains sum back to the input exactly, so
/// entering the filtered path from a clear path never clicks.
pub struct StereoPanner {
    left: f32,
    right: f32,
    bands: [f32; 3],
    split: BandSplit,
    initialized: bool,
}

impl Default for StereoPanner {
    fn default() -> Self {
        Self {
            left: 0.0,
            right: 0.0,
            bands: [1.0; 3],
            split: BandSplit::new(48000),
            initialized: false,
        }
    }
}

impl Spatializer for StereoPanner {
    fn prepare(&mut self, sample_rate: u32) {
        self.split = BandSplit::new(sample_rate);
    }
    fn process(
        &mut self,
        mono: &[f32],
        out: &mut [Frame],
        direction: [f32; 3],
        path: AcousticPath,
    ) {
        let pan = direction[0].clamp(-1.0, 1.0);
        let target_l = ((1.0 - pan) * 0.5).sqrt();
        let target_r = ((1.0 + pan) * 0.5).sqrt();
        let target_bands = path.band_gains();
        if !self.initialized {
            self.left = target_l;
            self.right = target_r;
            self.bands = target_bands;
            self.initialized = true;
        }
        // Static clear paths need no recursive filters; LLVM vectorizes this independent loop.
        if self.left == target_l
            && self.right == target_r
            && self.bands == [1.0; 3]
            && target_bands == [1.0; 3]
        {
            for (input, frame) in mono.iter().zip(out) {
                *frame = Frame::new(input * target_l, input * target_r);
            }
            return;
        }
        let n = mono.len().max(1) as f32;
        let step = |from: f32, to: f32| (to - from) / n;
        let (dl, dr) = (step(self.left, target_l), step(self.right, target_r));
        let db: [f32; 3] = std::array::from_fn(|b| step(self.bands[b], target_bands[b]));
        let (mut l, mut r, mut g) = (self.left, self.right, self.bands);
        for (input, frame) in mono.iter().zip(out) {
            l += dl;
            r += dr;
            for (gain, delta) in g.iter_mut().zip(db) {
                *gain += delta;
            }
            let [low, mid, high] = self.split.split(*input);
            let sample = g[0] * low + g[1] * mid + g[2] * high;
            *frame = Frame::new(sample * l, sample * r);
        }
        self.left = target_l;
        self.right = target_r;
        self.bands = target_bands;
    }
}
