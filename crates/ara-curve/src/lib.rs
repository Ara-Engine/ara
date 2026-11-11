//! A curve value: keyed values over time with tangents (Unity's `AnimationCurve`, Unreal's
//! rich curves). Particle properties over lifetime, animation blends and anything else that
//! maps one number to another hold a [`Curve`] and call [`Curve::evaluate`].
//!
//! Each key has a value, slopes on either side, how those slopes are chosen ([`Tangent`])
//! and how the segment after it moves ([`Interpolation`]). Outside its keys a curve holds,
//! repeats or mirrors ([`Extrapolation`]). Evaluation is plain `f32` arithmetic with no
//! state, so the same curve and time give the same value on every run.
use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// How a key's slopes are chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "CurveTangent")]
#[serde(rename_all = "camelCase")]
pub enum Tangent {
    /// Smooth through the neighbours without overshooting them; flat at the ends and at
    /// peaks and dips.
    #[default]
    Auto,
    /// Level on both sides.
    Flat,
    /// One slope set by hand on both sides (`out_slope`).
    Free,
    /// Separate slopes set by hand on each side.
    Broken,
}

/// How the segment from a key to the next one moves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "CurveInterpolation")]
#[serde(rename_all = "camelCase")]
pub enum Interpolation {
    /// A cubic shaped by both keys' slopes.
    #[default]
    Cubic,
    /// A straight line to the next key.
    Linear,
    /// This key's value until the next key.
    Constant,
}

/// What a curve does before its first key or after its last.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "CurveExtrapolation")]
#[serde(rename_all = "camelCase")]
pub enum Extrapolation {
    /// Holds the end key's value.
    #[default]
    Clamp,
    /// Repeats the keyed span.
    Loop,
    /// Repeats the keyed span, mirrored every other time.
    PingPong,
}

/// One key of a curve.
#[derive(Debug, Clone, Copy, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "CurveKey")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Key {
    /// Where the key sits on the curve's time axis.
    pub time: f32,
    /// The curve's value at the key.
    pub value: f32,
    /// The slope arriving at the key (value per unit time), used when `tangent` is Broken.
    #[serde(skip_serializing_if = "is_zero")]
    pub in_slope: f32,
    /// The slope leaving the key (value per unit time), used when `tangent` is Free or Broken.
    #[serde(skip_serializing_if = "is_zero")]
    pub out_slope: f32,
    /// How the key's slopes are chosen.
    #[serde(skip_serializing_if = "is_default")]
    pub tangent: Tangent,
    /// How the segment from this key to the next moves.
    #[serde(skip_serializing_if = "is_default")]
    pub interpolation: Interpolation,
}

/// Saved files leave out what a reader fills in anyway.
fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// As [`is_zero`], for the modes.
fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

impl Key {
    /// A smooth key at `time` with `value`.
    pub fn new(time: f32, value: f32) -> Self {
        Self {
            time,
            value,
            ..Self::default()
        }
    }
}

/// Values over time: keys in time order, joined by cubic, linear or constant segments.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "ValueCurve")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Curve {
    /// The keys, in strictly increasing time order.
    pub keys: Vec<Key>,
    /// What the curve does before its first key.
    #[serde(skip_serializing_if = "is_default")]
    pub before: Extrapolation,
    /// What the curve does after its last key.
    #[serde(skip_serializing_if = "is_default")]
    pub after: Extrapolation,
}

impl Default for Curve {
    /// A straight line from 0 to 1 over 0 to 1.
    fn default() -> Self {
        Self::linear(&[(0.0, 0.0), (1.0, 1.0)])
    }
}

/// A named starting shape, as a curve editor offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// A straight line from 0 to 1.
    Linear,
    /// Slow at both ends.
    EaseInOut,
    /// Slow at the start.
    EaseIn,
    /// Slow at the end.
    EaseOut,
    /// 1 throughout.
    Constant,
}

impl Preset {
    /// Every preset, in the order an editor lists them.
    pub const ALL: [Preset; 5] = [
        Preset::Linear,
        Preset::EaseInOut,
        Preset::EaseIn,
        Preset::EaseOut,
        Preset::Constant,
    ];

    /// The preset's name.
    pub fn name(self) -> &'static str {
        match self {
            Preset::Linear => "Linear",
            Preset::EaseInOut => "Ease In-Out",
            Preset::EaseIn => "Ease In",
            Preset::EaseOut => "Ease Out",
            Preset::Constant => "Constant",
        }
    }

    /// The preset over 0 to 1, rising from 0 to 1 (Constant: 1 throughout).
    pub fn curve(self) -> Curve {
        let key = |time, value, tangent, slope| Key {
            time,
            value,
            in_slope: slope,
            out_slope: slope,
            tangent,
            interpolation: Interpolation::Cubic,
        };
        let keys = match self {
            Preset::Linear => return Curve::linear(&[(0.0, 0.0), (1.0, 1.0)]),
            Preset::Constant => return Curve::linear(&[(0.0, 1.0), (1.0, 1.0)]),
            Preset::EaseInOut => vec![
                key(0.0, 0.0, Tangent::Auto, 0.0),
                key(1.0, 1.0, Tangent::Auto, 0.0),
            ],
            Preset::EaseIn => vec![
                key(0.0, 0.0, Tangent::Flat, 0.0),
                key(1.0, 1.0, Tangent::Free, 2.0),
            ],
            Preset::EaseOut => vec![
                key(0.0, 0.0, Tangent::Free, 2.0),
                key(1.0, 1.0, Tangent::Flat, 0.0),
            ],
        };
        Curve {
            keys,
            ..Curve::empty()
        }
    }
}

impl Curve {
    /// No keys: evaluates to 0 until one is added.
    pub fn empty() -> Self {
        Self {
            keys: Vec::new(),
            before: Extrapolation::Clamp,
            after: Extrapolation::Clamp,
        }
    }

    /// Straight lines through `points` (time, value), which must be in time order.
    pub fn linear(points: &[(f32, f32)]) -> Self {
        Self {
            keys: points
                .iter()
                .map(|&(time, value)| Key {
                    interpolation: Interpolation::Linear,
                    ..Key::new(time, value)
                })
                .collect(),
            ..Self::empty()
        }
    }

    /// `value` throughout.
    pub fn constant(value: f32) -> Self {
        Self::linear(&[(0.0, value)])
    }

    /// Why the curve can't be used, if it can't: it needs a key, finite numbers, and keys in
    /// strictly increasing time.
    pub fn validate(&self) -> Result<(), String> {
        if self.keys.is_empty() {
            return Err("a curve needs at least one key".into());
        }
        for (i, k) in self.keys.iter().enumerate() {
            if ![k.time, k.value, k.in_slope, k.out_slope]
                .iter()
                .all(|v| v.is_finite())
            {
                return Err(format!("key {i} has a number that isn't finite"));
            }
            if i > 0 && k.time <= self.keys[i - 1].time {
                return Err(format!("key {i} isn't after key {}", i - 1));
            }
        }
        Ok(())
    }

    /// The first and last keys' times, or `None` without keys.
    pub fn span(&self) -> Option<(f32, f32)> {
        Some((self.keys.first()?.time, self.keys.last()?.time))
    }

    /// The slopes key `index` arrives and leaves with, as its [`Tangent`] chooses them.
    pub fn slopes(&self, index: usize) -> (f32, f32) {
        let k = &self.keys[index];
        match k.tangent {
            Tangent::Flat => (0.0, 0.0),
            Tangent::Free => (k.out_slope, k.out_slope),
            Tangent::Broken => (k.in_slope, k.out_slope),
            Tangent::Auto => {
                let auto = self.auto_slope(index);
                (auto, auto)
            }
        }
    }

    /// A smooth slope that doesn't overshoot either neighbour (monotone cubic): level at the
    /// ends and wherever the key is a peak or a dip.
    fn auto_slope(&self, i: usize) -> f32 {
        let (Some(prev), Some(next)) = (
            i.checked_sub(1).and_then(|p| self.keys.get(p)),
            self.keys.get(i + 1),
        ) else {
            return 0.0;
        };
        let k = &self.keys[i];
        let (before, after) = (
            (k.value - prev.value) / (k.time - prev.time),
            (next.value - k.value) / (next.time - k.time),
        );
        if before * after <= 0.0 {
            return 0.0;
        }
        let slope = (next.value - prev.value) / (next.time - prev.time);
        // Fritsch–Carlson: no more than three times either side's secant.
        let limit = 3.0 * before.abs().min(after.abs());
        slope.clamp(-limit, limit)
    }

    /// The curve's value at `time`.
    pub fn evaluate(&self, time: f32) -> f32 {
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else {
            return 0.0;
        };
        if self.keys.len() == 1 || !time.is_finite() {
            return first.value;
        }
        let time = self.wrap(time, first.time, last.time);
        // The segment starting at the last key at or before `time`.
        let i = self
            .keys
            .partition_point(|k| k.time <= time)
            .clamp(1, self.keys.len() - 1)
            - 1;
        self.segment(i, time)
    }

    /// `time` brought into the keyed span by the extrapolation on its side.
    fn wrap(&self, time: f32, start: f32, end: f32) -> f32 {
        let length = end - start;
        let mode = match time {
            t if t < start => self.before,
            t if t > end => self.after,
            t => return t,
        };
        match mode {
            Extrapolation::Clamp => time.clamp(start, end),
            Extrapolation::Loop => start + (time - start).rem_euclid(length),
            Extrapolation::PingPong => {
                let phase = (time - start).rem_euclid(2.0 * length);
                start
                    + if phase > length {
                        2.0 * length - phase
                    } else {
                        phase
                    }
            }
        }
    }

    /// The value at `time` on the segment from key `i` to key `i + 1`.
    fn segment(&self, i: usize, time: f32) -> f32 {
        let (a, b) = (&self.keys[i], &self.keys[i + 1]);
        let span = b.time - a.time;
        let u = ((time - a.time) / span).clamp(0.0, 1.0);
        match a.interpolation {
            Interpolation::Constant => {
                if u >= 1.0 {
                    b.value
                } else {
                    a.value
                }
            }
            Interpolation::Linear => a.value + (b.value - a.value) * u,
            Interpolation::Cubic => {
                let (_, out_slope) = self.slopes(i);
                let (in_slope, _) = self.slopes(i + 1);
                let (u2, u3) = (u * u, u * u * u);
                (2.0 * u3 - 3.0 * u2 + 1.0) * a.value
                    + (u3 - 2.0 * u2 + u) * span * out_slope
                    + (-2.0 * u3 + 3.0 * u2) * b.value
                    + (u3 - u2) * span * in_slope
            }
        }
    }

    /// Segment `i` (key `i` to `i + 1`) as `c0 + c1·u + c2·u² + c3·u³` over its local
    /// `u` in [0, 1): what a shader lowering evaluates (a Constant segment is just `c0`).
    pub fn polynomial(&self, i: usize) -> [f32; 4] {
        let (a, b) = (&self.keys[i], &self.keys[i + 1]);
        match a.interpolation {
            Interpolation::Constant => [a.value, 0.0, 0.0, 0.0],
            Interpolation::Linear => [a.value, b.value - a.value, 0.0, 0.0],
            Interpolation::Cubic => {
                let span = b.time - a.time;
                let (m0, m1) = (span * self.slopes(i).1, span * self.slopes(i + 1).0);
                [
                    a.value,
                    m0,
                    -3.0 * a.value - 2.0 * m0 + 3.0 * b.value - m1,
                    2.0 * a.value + m0 - 2.0 * b.value + m1,
                ]
            }
        }
    }

    /// Adds a key at `time` with `value`, or sets the value of the key already there; its
    /// index. A new key is smooth, and moves as the segment it splits did.
    pub fn insert(&mut self, time: f32, value: f32) -> usize {
        let at = self.keys.partition_point(|k| k.time < time);
        if let Some(k) = self.keys.get_mut(at).filter(|k| k.time == time) {
            k.value = value;
            return at;
        }
        let interpolation = at
            .checked_sub(1)
            .and_then(|p| self.keys.get(p))
            .or(self.keys.get(at))
            .map_or(Interpolation::Cubic, |k| k.interpolation);
        self.keys.insert(
            at,
            Key {
                interpolation,
                ..Key::new(time, value)
            },
        );
        at
    }

    /// Moves key `index` to `time` and `value`, kept between its neighbours (a small gap
    /// short of them), so the keys stay in order; the time it got.
    pub fn move_key(&mut self, index: usize, time: f32, value: f32) -> f32 {
        let gap = |a: f32, b: f32| ((b - a).abs() * 1e-3).max(1e-6);
        let low = index
            .checked_sub(1)
            .map(|p| self.keys[p].time)
            .map_or(f32::NEG_INFINITY, |t| t + gap(t, self.keys[index].time));
        let high = self
            .keys
            .get(index + 1)
            .map(|n| n.time)
            .map_or(f32::INFINITY, |t| t - gap(self.keys[index].time, t));
        let k = &mut self.keys[index];
        k.time = time.clamp(low, high.max(low));
        k.value = value;
        k.time
    }

    /// The lowest and highest values over the keyed span, sampled `per_segment` times along
    /// each segment (cubic segments can pass their keys), or `None` without keys.
    pub fn value_range(&self, per_segment: usize) -> Option<(f32, f32)> {
        let (start, end) = self.span()?;
        let samples = per_segment.max(1) * self.keys.len().max(2);
        let mut range = (f32::INFINITY, f32::NEG_INFINITY);
        for i in 0..=samples {
            let v = self.evaluate(start + (end - start) * i as f32 / samples as f32);
            range = (range.0.min(v), range.1.max(v));
        }
        for k in &self.keys {
            range = (range.0.min(k.value), range.1.max(k.value));
        }
        Some(range)
    }
}

#[cfg(test)]
mod tests;
