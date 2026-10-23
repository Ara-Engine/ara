//! Contiguous resident blocks retain the sample path's exact look-ahead history.
use super::{Input, Voice};
use crate::{Frame, QUANTUM};
use std::sync::atomic::Ordering;

impl Voice {
    // Unit-rate resident playback is a contiguous multiply, not 128 cubic resamplings.
    // Keep the four history samples identical so rate changes rejoin the general path.
    pub(super) fn resident_block(&mut self, out: &mut [Frame], dt: f64, target: f32) -> bool {
        let n = out.len();
        if n < 4
            || self.time < self.start
            || self.dry_ended
            || self.stop.is_some()
            || self
                .pending
                .is_some_and(|c| c.at_frame() < self.time + n as u64)
            || self.gain.remaining != 0
            || self.rate.remaining != 0
            || self.virtual_gain.remaining != 0
            || self.fraction != 0.0
            || self.rate.value as f64 * self.source_rate as f64 * dt != 1.0
        {
            return false;
        }
        let Input::Resident {
            clip,
            cursor,
            looping,
        } = &mut self.input
        else {
            return false;
        };
        // The general path owns end padding and stops, including the four-frame tail.
        if !*looping && cursor.saturating_add(n) >= clip.len() {
            return false;
        }
        if target == 0.0 {
            *cursor = (*cursor + n - 1) % clip.len() + 1;
            for (i, sample) in self.history.iter_mut().enumerate() {
                *sample = clip.frames[(*cursor + clip.len() - (4 - i) % clip.len()) % clip.len()];
            }
            out.fill(Frame::ZERO);
            self.time += n as u64;
            self.shared.position.fetch_add(n as u64, Ordering::Relaxed);
            self.shared
                .audibility
                .store(self.gain.value.to_bits(), Ordering::Relaxed);
            return true;
        }
        let gain = self.gain.value * self.virtual_gain.value;
        let mut samples = [Frame::ZERO; QUANTUM];
        samples[..3].copy_from_slice(&self.history[1..]);
        let mut written = 3;
        while written < n {
            if *cursor == clip.len() {
                *cursor = 0;
            }
            let count = (n - written).min(clip.len() - *cursor);
            samples[written..written + count]
                .copy_from_slice(&clip.frames[*cursor..*cursor + count]);
            *cursor += count;
            written += count;
        }
        // Three look-ahead frames are consumed even for virtual voices.
        self.history[0] = samples[n - 1];
        for sample in &mut self.history[1..] {
            if *cursor == clip.len() {
                *cursor = 0;
            }
            *sample = clip.frames[*cursor];
            *cursor += 1;
        }
        if self.spatial {
            for (mono, sample) in self.mono.iter_mut().zip(&samples[..n]) {
                *mono = (sample.left + sample.right) * 0.5 * gain;
            }
            self.spatializer.process(
                &self.mono[..n],
                out,
                self.parameters.direction,
                self.parameters.path,
            );
        } else {
            for (frame, sample) in out.iter_mut().zip(&samples[..n]) {
                *frame = *sample * gain;
            }
        }
        self.time += n as u64;
        self.shared.position.fetch_add(n as u64, Ordering::Relaxed);
        self.shared
            .audibility
            .store(self.gain.value.to_bits(), Ordering::Relaxed);
        true
    }
}
