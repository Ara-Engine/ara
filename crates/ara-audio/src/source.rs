//! Procedural voice input: synthesized blocks instead of decoded PCM.

use crate::Frame;

/// Block-rendered synthesized audio, such as a compiled procedural audio graph.
///
/// Built and fully allocated on a loader thread, then moved into a voice with
/// `PreparedVoice::procedural`. `render` runs on the audio thread and must not
/// allocate, lock, read clocks or perform IO.
pub trait SignalSource: Send {
    /// Frames per second the source renders at, normally the engine's sample rate.
    fn sample_rate(&self) -> u32;
    /// Fills `out` (at most one quantum) with the next frames. `frame` is the voice's
    /// output-clock frame when the block was requested; schedule control changes against it.
    fn render(&mut self, out: &mut [Frame], frame: u64);
    /// True once every remaining frame is silence and the voice may finish.
    fn ended(&self) -> bool;
}
