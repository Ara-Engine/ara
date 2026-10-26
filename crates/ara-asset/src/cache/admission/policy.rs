//! Scheduling feedback only; these bytes never participate in an import key.
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const MAX_WRITERS: u32 = 8;
const WINDOW: usize = 16;
const SIZE: usize = 132;

#[derive(Debug, Clone, Copy)]
pub(super) struct Policy {
    pub limit: u32,
    cursor: usize,
    count: usize,
    changed_ms: u64,
    sampled_ms: u64,
    samples: [u32; WINDOW],
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(u64::MAX as u128) as u64)
}

impl Policy {
    pub fn new(now: u64) -> Self {
        Self {
            limit: 4,
            cursor: 0,
            count: 0,
            changed_ms: now,
            sampled_ms: now,
            samples: [0; WINDOW],
        }
    }

    pub fn decode(bytes: &[u8], now: u64) -> Self {
        if bytes.len() != SIZE
            || &bytes[..8] != b"ARAIO001"
            || blake3::hash(&bytes[..100]).as_bytes() != &bytes[100..]
        {
            return Self::new(now);
        }
        let u32_at =
            |i| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("fixed policy field"));
        let u64_at =
            |i| u64::from_le_bytes(bytes[i..i + 8].try_into().expect("fixed policy field"));
        let state = Self {
            limit: u32_at(8),
            cursor: u32_at(12) as usize,
            count: u32_at(16) as usize,
            changed_ms: u64_at(20),
            sampled_ms: u64_at(28),
            samples: std::array::from_fn(|i| u32_at(36 + i * 4)),
        };
        if !(1..=MAX_WRITERS).contains(&state.limit)
            || state.cursor >= WINDOW
            || state.count > WINDOW
            || now < state.sampled_ms
        {
            Self::new(now)
        } else {
            state
        }
    }

    pub fn reset_if_idle(&mut self, now: u64) -> bool {
        if now.saturating_sub(self.sampled_ms) <= 15_000 {
            return false;
        }
        *self = Self::new(now);
        true
    }

    pub fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(SIZE);
        bytes.extend_from_slice(b"ARAIO001");
        for value in [self.limit, self.cursor as u32, self.count as u32] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&self.changed_ms.to_le_bytes());
        bytes.extend_from_slice(&self.sampled_ms.to_le_bytes());
        for sample in self.samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes.extend_from_slice(blake3::hash(&bytes).as_bytes());
        bytes
    }

    /// Returns a limit transition and its p90. Tiny action records cannot drown
    /// out payload-write pressure; admission wait is never a latency sample.
    pub fn observe(&mut self, bytes: usize, elapsed_ms: u32, now: u64) -> Option<(u32, u32)> {
        if bytes < 64 * 1024 {
            return None;
        }
        if now.saturating_sub(self.sampled_ms) > 15_000 {
            self.count = 0;
            self.cursor = 0;
        }
        self.sampled_ms = now;
        self.samples[self.cursor] = elapsed_ms;
        self.cursor = (self.cursor + 1) % WINDOW;
        self.count = (self.count + 1).min(WINDOW);
        let mut sorted = self.samples;
        sorted[..self.count].sort_unstable();
        let p90 = sorted[(self.count * 9).div_ceil(10) - 1];
        let age = now.saturating_sub(self.changed_ms);
        let before = self.limit;
        if self.count >= 8 && p90 >= 25 && age >= 250 {
            self.limit = (self.limit / 2).max(1);
        } else if self.count == WINDOW && p90 <= 4 && age >= 1_000 {
            self.limit = (self.limit + 1).min(MAX_WRITERS);
        }
        if before == self.limit {
            return None;
        }
        self.changed_ms = now;
        self.count = 0;
        self.cursor = 0;
        Some((before, p90))
    }
}

#[cfg(test)]
mod tests;
