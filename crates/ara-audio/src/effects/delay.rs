//! Stereo delay. Feedback controls subsequent repeats, never the first echo.

use crate::{AudioError, Result};
use kira::{
    Frame,
    effect::{Effect, EffectBuilder},
    info::Info,
};
use std::time::Duration;

/// Prepared stereo delay configuration. Construction is fallible; no invalid DSP values reach rendering.
pub struct DelayBuilder {
    duration: Duration,
    feedback: f32,
    mix: f32,
}
impl DelayBuilder {
    /// Creates a delay of (0,10] seconds, linear feedback in (-1,1), and equal-power wet mix in [0,1].
    pub fn new(duration: Duration, feedback: f32, mix: f32) -> Result<Self> {
        if duration.is_zero()
            || duration > Duration::from_secs(10)
            || !feedback.is_finite()
            || feedback.abs() >= 1.0
            || !mix.is_finite()
            || !(0.0..=1.0).contains(&mix)
        {
            return Err(AudioError::Invalid("delay duration, feedback or mix"));
        }
        Ok(Self {
            duration,
            feedback,
            mix,
        })
    }
}
impl EffectBuilder for DelayBuilder {
    type Handle = ();
    fn build(self) -> (Box<dyn Effect>, ()) {
        (
            Box::new(Delay {
                duration: self.duration,
                feedback: self.feedback,
                dry: (1.0 - self.mix).sqrt(),
                wet: self.mix.sqrt(),
                buffer: Vec::new(),
                cursor: 0,
            }),
            (),
        )
    }
}
struct Delay {
    duration: Duration,
    feedback: f32,
    dry: f32,
    wet: f32,
    buffer: Vec<Frame>,
    cursor: usize,
}
impl Effect for Delay {
    fn init(&mut self, sample_rate: u32, _: usize) {
        self.on_change_sample_rate(sample_rate);
    }
    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.buffer = vec![
            Frame::ZERO;
            (self.duration.as_secs_f64() * sample_rate as f64)
                .round()
                .max(1.0) as usize
        ];
        self.cursor = 0;
    }
    fn process(&mut self, input: &mut [Frame], _: f64, _: &Info) {
        for frame in input {
            let echo = self.buffer[self.cursor];
            self.buffer[self.cursor] = *frame + echo * self.feedback;
            *frame = *frame * self.dry + echo * self.wet;
            self.cursor += 1;
            if self.cursor == self.buffer.len() {
                self.cursor = 0;
            }
        }
    }
}
