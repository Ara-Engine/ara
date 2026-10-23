//! A segment's musical grid: bar and beat positions in output frames from its tempo map.

use crate::desc::{MusicSync, SegmentDesc, TempoChange};

#[derive(Debug, Clone, Copy)]
struct Section {
    bar: u32,
    start: f64,
    beat: f64,
    beats_per_bar: u32,
}

/// Bar/beat ↔ frame offsets within one segment (offset 0 is bar 0, beat 0).
#[derive(Debug, Clone)]
pub struct Grid {
    sections: Vec<Section>,
    bars: u32,
}

/// A musical position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    /// Bar (0-based).
    pub bar: u32,
    /// Beat within the bar (0-based, fractional).
    pub beat: f64,
}

impl Grid {
    /// Builds the grid for `segment` at `sample_rate`.
    pub fn new(segment: &SegmentDesc, sample_rate: u32) -> Self {
        let default = [TempoChange::default()];
        let tempo = if segment.tempo.is_empty() {
            &default[..]
        } else {
            &segment.tempo
        };
        let mut sections = Vec::with_capacity(tempo.len());
        let mut start = 0.0;
        for t in tempo {
            if let Some(previous) = sections.last() {
                let Section {
                    bar,
                    beat,
                    beats_per_bar,
                    ..
                } = *previous;
                start += f64::from(t.bar - bar) * beat * f64::from(beats_per_bar);
            }
            sections.push(Section {
                bar: t.bar,
                start,
                beat: 60.0 / f64::from(t.bpm) * f64::from(sample_rate),
                beats_per_bar: t.beats_per_bar,
            });
        }
        Self {
            sections,
            bars: segment.bars,
        }
    }

    fn section(&self, bar: u32) -> &Section {
        let i = self
            .sections
            .partition_point(|s| s.bar <= bar)
            .saturating_sub(1);
        &self.sections[i]
    }

    /// Frame offset of a bar line (bars beyond the segment extend the last tempo).
    pub fn bar(&self, bar: u32) -> f64 {
        let s = self.section(bar);
        s.start + f64::from(bar - s.bar) * s.beat * f64::from(s.beats_per_bar)
    }

    /// Segment length in frames.
    pub fn length(&self) -> f64 {
        self.bar(self.bars)
    }

    /// Bar and beat at a frame offset.
    pub fn position(&self, offset: f64) -> Position {
        let i = self
            .sections
            .partition_point(|s| s.start <= offset)
            .saturating_sub(1);
        let s = &self.sections[i];
        let beats = ((offset - s.start) / s.beat).max(0.0);
        let bars = (beats / f64::from(s.beats_per_bar)).floor();
        Position {
            bar: s.bar + bars as u32,
            beat: beats - bars * f64::from(s.beats_per_bar),
        }
    }

    /// First boundary at or after `offset` for `sync` (`End` is the segment length).
    pub fn next(&self, offset: f64, sync: MusicSync) -> f64 {
        match sync {
            MusicSync::Immediate => offset,
            MusicSync::End => self.length().max(offset),
            MusicSync::Bar => {
                let at = self.position(offset);
                let line = self.bar(at.bar);
                if line >= offset - 0.5 {
                    line
                } else {
                    self.bar(at.bar + 1)
                }
            }
            MusicSync::Beat => {
                let at = self.position(offset);
                let s = self.section(at.bar);
                let bar = self.bar(at.bar);
                let beat = ((offset - bar) / s.beat - 1e-9).ceil().max(0.0);
                bar + beat * s.beat
            }
        }
    }
}
