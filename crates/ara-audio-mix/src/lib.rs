//! Mixer documents for `ara-audio`: the bus (category) tree with base gains and mutes,
//! room-reverb sends, sidechain ducking, per-category voice limits, loudness targets
//! (-16 LUFS for music and dialogue) and named mix states (combat, explore) with
//! transition times. Authoring uses typed `mixer.bus.set` commands producing ops;
//! `Mixer::build` applies a validated `MixerDesc` to an engine.

/// Typed mixer authoring through the engine command registry.
pub mod commands;
mod desc;
mod runtime;

pub use desc::{BusDesc, DuckDesc, MixStateDesc, MixerDesc, MixerSettings};
pub use runtime::{Mixer, linear};

use ara_diag::Diagnostic;
use ara_schema::{DocKindDesc, SchemaDb};

/// Registers the mixer records and the `mixer` document kind (`.mixer`, IDs `mx_...`).
pub fn register(db: &mut SchemaDb) -> Result<(), Diagnostic> {
    db.register::<BusDesc>()
        .register::<MixStateDesc>()
        .register::<MixerSettings>();
    db.add_kind(DocKindDesc {
        name: "mixer".into(),
        version: 1,
        doc:
            "Audio mixer: bus tree, sends, ducking, voice limits, loudness targets and mix states."
                .into(),
        id_prefix: "mx".into(),
        extension: "mixer".into(),
        records: vec![
            "AudioBus".into(),
            "AudioMixState".into(),
            "AudioMixerSettings".into(),
        ],
        single_root: false,
    })
}
