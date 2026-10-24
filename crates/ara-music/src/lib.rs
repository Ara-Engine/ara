//! Interactive music on `ara-audio`'s sample clock.
//!
//! A `music` document holds segments (tempo map, synchronized stems with intensity
//! windows, weighted follow-ups), transition rules (bar/beat/end/immediate, crossfade,
//! optional bridge), stingers and settings. `MusicPlayer` schedules every stem as a voice
//! at an exact output frame, so segment changes, layers and stingers land on the grid in
//! live play and in deterministic offline captures alike.

pub mod commands;
mod desc;
mod grid;
mod player;

pub use desc::{
    MusicDesc, MusicSettings, MusicSync, NextDesc, SegmentDesc, StemDesc, StingerDesc, TempoChange,
    TransitionDesc,
};
pub use grid::{Grid, Position};
pub use player::{HISTORY, MusicPlayer, MusicPosition, Scheduled, StemLoader};

use ara_audio::{AudioError, Clip, Frame};
use ara_diag::Diagnostic;
use ara_schema::{DocKindDesc, SchemaDb};

/// Registers music records and the `music` document kind (`.music`, IDs `mu_...`).
pub fn register(db: &mut SchemaDb) -> Result<(), Diagnostic> {
    db.register::<SegmentDesc>()
        .register::<TransitionDesc>()
        .register::<StingerDesc>()
        .register::<MusicSettings>();
    db.add_kind(DocKindDesc {
        name: "music".into(),
        version: 1,
        doc: "Interactive music: segments with stems and tempo maps, transitions, stingers.".into(),
        id_prefix: "mu".into(),
        extension: "music".into(),
        records: vec![
            "MusicSegment".into(),
            "MusicTransition".into(),
            "MusicStinger".into(),
            "MusicSettings".into(),
        ],
        single_root: false,
    })
}

/// Integrated loudness of a segment's full mix (stems summed at their linear gains),
/// for `SegmentDesc::loudness`. Run at cook time; stems must share a sample rate.
pub fn measure_mix_loudness(stems: &[(&Clip, f32)]) -> ara_audio::Result<Option<f32>> {
    let Some((first, _)) = stems.first() else {
        return Ok(None);
    };
    if stems
        .iter()
        .any(|(c, g)| c.sample_rate() != first.sample_rate() || !g.is_finite())
    {
        return Err(AudioError::Invalid(
            "stems need one sample rate and finite gains",
        ));
    }
    let len = stems.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    let mut mix = vec![Frame::ZERO; len];
    for (clip, gain) in stems {
        for (out, f) in mix.iter_mut().zip(clip.frames()) {
            *out += *f * *gain;
        }
    }
    ara_audio::loudness::integrated(first.sample_rate(), &mix)
}
