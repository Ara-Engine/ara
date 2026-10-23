//! Sample sources a voice resamples: resident clips, bounded streams or procedural blocks.

use super::Clip;
use crate::{Frame, QUANTUM, source::SignalSource, streaming::StreamReader};

pub(crate) enum Input {
    Resident {
        clip: Clip,
        cursor: usize,
        looping: bool,
    },
    Stream(StreamReader),
    Procedural {
        source: Box<dyn SignalSource>,
        block: Box<[Frame; QUANTUM]>,
        cursor: usize,
    },
}

impl Input {
    pub(crate) fn procedural(source: Box<dyn SignalSource>) -> Self {
        Self::Procedural {
            source,
            block: Box::new([Frame::ZERO; QUANTUM]),
            cursor: QUANTUM,
        }
    }
    pub(crate) fn sample_rate(&self) -> u32 {
        match self {
            Self::Resident { clip, .. } => clip.sample_rate(),
            Self::Stream(s) => s.sample_rate(),
            Self::Procedural { source, .. } => source.sample_rate(),
        }
    }
    /// Next source frame. `time` is the voice's output frame, passed to procedural blocks.
    pub(crate) fn next(&mut self, time: u64) -> Frame {
        match self {
            Self::Resident {
                clip,
                cursor,
                looping,
            } => {
                if *cursor >= clip.len() {
                    if *looping {
                        *cursor = 0;
                    } else {
                        return Frame::ZERO;
                    }
                }
                let frame = clip.frames[*cursor];
                *cursor += 1;
                frame
            }
            Self::Stream(s) => s.next_frame(),
            Self::Procedural {
                source,
                block,
                cursor,
            } => {
                if *cursor == QUANTUM {
                    if source.ended() {
                        return Frame::ZERO;
                    }
                    source.render(block.as_mut_slice(), time);
                    *cursor = 0;
                }
                let frame = block[*cursor];
                *cursor += 1;
                frame
            }
        }
    }
    pub(crate) fn ended(&self) -> bool {
        match self {
            Self::Resident {
                clip,
                cursor,
                looping,
            } => !looping && *cursor >= clip.len(),
            Self::Stream(s) => s.ended(),
            Self::Procedural { source, cursor, .. } => *cursor == QUANTUM && source.ended(),
        }
    }
}
