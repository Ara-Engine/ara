//! Spatial geometry is evaluated off the render path, then smoothly applied by a voice.

use crate::{AudioError, AudioListener, AudioSource, Frame, Result, schema::Attenuation};

/// Acoustic simulation result. Physics/Steam workers publish this at a frame boundary.
#[derive(Debug, Clone, Copy, ara_schema::Schema, serde::Serialize, serde::Deserialize)]
#[ara(module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct AcousticPath {
    /// Direct path visibility: zero is fully occluded, one is clear.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub visibility: f32,
    /// Low/mid/high band transmission through occluders.
    #[ara(range(min = 0, max = 1))]
    pub transmission: [f32; 3],
    /// Late reverberation send amplitude, consumed by the mixer bus bridge.
    #[ara(range(min = 0, max = 1))]
    pub reverb_send: f32,
}

impl Default for AcousticPath {
    fn default() -> Self {
        Self {
            visibility: 1.0,
            transmission: [0.0; 3],
            reverb_send: 0.0,
        }
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
    /// Converts a mono quantum to stereo. Directions may change once per quantum.
    fn process(&mut self, mono: &[f32], out: &mut [Frame], direction: [f32; 3], path: AcousticPath);
    /// Renders the next tail quantum after dry input ends. True means the tail is complete.
    fn drain(&mut self, out: &mut [Frame]) -> bool {
        out.fill(Frame::ZERO);
        true
    }
}

/// Equal-power stereo fallback, with smoothed occlusion and a low-pass shadow filter.
#[derive(Default)]
pub struct StereoPanner {
    left: f32,
    right: f32,
    low: f32,
    initialized: bool,
    visibility: f32,
    transmission: f32,
}

impl Spatializer for StereoPanner {
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
        if !self.initialized {
            self.left = target_l;
            self.right = target_r;
            self.initialized = true;
            self.visibility = path.visibility;
            self.transmission = path.transmission[0];
        }
        // Static clear paths need no recursive filters; LLVM vectorizes this independent loop.
        // Keep the low-pass history current for a subsequent occlusion transition.
        if self.left == target_l
            && self.right == target_r
            && self.visibility == 1.0
            && path.visibility == 1.0
            && self.transmission == path.transmission[0]
        {
            for input in mono {
                self.low += (*input - self.low) * 0.08;
            }
            for (input, frame) in mono.iter().zip(out) {
                *frame = Frame::new(input * target_l, input * target_r);
            }
            return;
        }
        for (input, frame) in mono.iter().zip(out) {
            self.left += (target_l - self.left) * 0.02;
            self.right += (target_r - self.right) * 0.02;
            self.low += (*input - self.low) * 0.08;
            self.visibility += (path.visibility - self.visibility) * 0.02;
            self.transmission += (path.transmission[0] - self.transmission) * 0.02;
            let transmitted = self.low * self.transmission * (1.0 - self.visibility);
            let sample = input * self.visibility + transmitted;
            *frame = Frame::new(sample * self.left, sample * self.right);
        }
    }
}
