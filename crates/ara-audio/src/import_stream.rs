//! Bounded PCM staging for music and speech cooks, independent of track duration.
use crate::{AudioError, Frame, Result, codec::AudioDecoder};
use ebur128::{EbuR128, Mode};
use std::collections::VecDeque;

/// Source-rate measurement and windowed-sinc resampling with one packet of lookahead.
pub(crate) struct Source<'a> {
    decoder: &'a mut dyn AudioDecoder,
    rate: u32,
    frames: VecDeque<Frame>,
    start: u64,
    count: u64,
    output: u64,
    eof: bool,
    first: Frame,
    last: Frame,
    meter: EbuR128,
    peak: f32,
}

fn meter_error(error: ebur128::Error) -> AudioError {
    AudioError::Codec(error.to_string())
}

impl<'a> Source<'a> {
    pub(crate) fn new(decoder: &'a mut dyn AudioDecoder, budget: usize) -> Result<Self> {
        // Decoder, encoder and meter scratch are separate from the resident PCM budget.
        // This covers the queue, decode block, meter interleave and Opus input block.
        if budget < 65536 {
            return Err(AudioError::Capacity(
                "streamed cook needs at least 65536 PCM bytes",
            ));
        }
        let rate = decoder.sample_rate();
        let meter = EbuR128::new(2, rate, Mode::I | Mode::HISTOGRAM).map_err(meter_error)?;
        Ok(Self {
            decoder,
            rate,
            frames: VecDeque::with_capacity(2048),
            start: 0,
            count: 0,
            output: 0,
            eof: false,
            first: Frame::ZERO,
            last: Frame::ZERO,
            meter,
            peak: 0.0,
        })
    }

    fn decode(&mut self) -> Result<()> {
        let mut block = [Frame::ZERO; 1024];
        let n = self.decoder.read_frames(&mut block)?;
        if n == 0 {
            self.eof = true;
            return Ok(());
        }
        let mut stereo = [0.0; 2048];
        for (i, frame) in block[..n].iter().enumerate() {
            if !frame.left.is_finite() || !frame.right.is_finite() {
                return Err(AudioError::Invalid("non-finite source audio"));
            }
            stereo[i * 2] = frame.left;
            stereo[i * 2 + 1] = frame.right;
            self.peak = self.peak.max(frame.left.abs()).max(frame.right.abs());
        }
        self.meter
            .add_frames_f32(&stereo[..n * 2])
            .map_err(meter_error)?;
        if self.count == 0 {
            self.first = block[0];
        }
        self.last = block[n - 1];
        self.count += n as u64;
        self.frames.extend(&block[..n]);
        Ok(())
    }

    pub(crate) fn next(&mut self) -> Result<Option<Frame>> {
        let ratio = f64::from(self.rate) / 48000.0;
        let position = self.output as f64 * ratio;
        let center = position.floor() as i64;
        let halo = if self.rate == 48000 { 0 } else { 32 };
        while !self.eof && self.count <= (center + halo) as u64 {
            self.decode()?;
        }
        if self.eof && self.output >= (self.count * 48000).div_ceil(u64::from(self.rate)) {
            return Ok(None);
        }
        let at = |index: i64| {
            if index < 0 {
                self.first
            } else if index as u64 >= self.count {
                self.last
            } else {
                self.frames[(index as u64 - self.start) as usize]
            }
        };
        let value = if self.rate == 48000 {
            at(center)
        } else {
            let cutoff = (1.0 / ratio).min(1.0) * 0.95;
            let mut sum = Frame::ZERO;
            let mut weight = 0.0;
            for tap in -32..=32 {
                let index = center + tap;
                let x = index as f64 - position;
                let z = std::f64::consts::PI * x * cutoff;
                let sinc = if z.abs() < 1e-12 { 1.0 } else { z.sin() / z };
                let window = 0.5 + 0.5 * (std::f64::consts::PI * x / 33.0).cos();
                let w = sinc * window * cutoff;
                sum += at(index) * w as f32;
                weight += w;
            }
            sum * (1.0 / weight) as f32
        };
        self.output += 1;
        let keep = (center - 32).max(0) as u64;
        while self.start < keep {
            self.frames.pop_front();
            self.start += 1;
        }
        Ok(Some(value))
    }

    pub(crate) fn measurement(&self) -> Result<(Option<f32>, f32)> {
        let loudness = if self.count < u64::from(self.rate) * 4 / 10 {
            None
        } else {
            let value = self.meter.loudness_global().map_err(meter_error)?;
            value.is_finite().then_some(value as f32)
        };
        Ok((loudness, self.peak))
    }
}
