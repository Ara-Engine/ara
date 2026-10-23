//! "Why can't I hear X?": audio diagnostic codes and the audibility checks behind them.
//!
//! The runtime gathers an [`Audibility`] for a source (component values, the voice's
//! budget state, its bus path and the acoustics result) and emits [`Audibility::explain`]
//! on the diagnostics bus, keyed by the entity. `ara-audio-why` reads those records
//! plus the documents to answer questions; agents get runnable `property.set` fixes.

use crate::{AudioSource, VoiceState, VoiceStatus, spatial::AcousticPath};
use ara_diag::{Diagnostic, FixAction, Subject};
use serde_json::json;

/// Audio diagnostic codes, ARA3400–3499 (catalogued in ara-diag `codes/subsystems.md`).
pub mod codes {
    use ara_diag::Code;
    /// The source's clip or graph is missing, unresolved or not yet loaded.
    pub const NOT_LOADED: Code = Code(3400);
    /// The listener is beyond the source's `maxDistance`.
    pub const OUT_OF_RANGE: Code = Code(3401);
    /// Geometry between source and listener blocks most of the direct sound.
    pub const OCCLUDED: Code = Code(3402);
    /// The source's bus, or a bus above it, has zero gain.
    pub const BUS_MUTED: Code = Code(3403);
    /// The voice is virtual: higher-priority voices fill the global or category budget.
    pub const VOICE_LIMITED: Code = Code(3404);
    /// The source's own gain (or its distance/cone attenuation) is zero.
    pub const SILENT_SOURCE: Code = Code(3405);
    /// A procedural audio graph failed to parse, check or compile.
    pub const GRAPH: Code = Code(3406);
    /// A mixer document is invalid (bus tree, sends, ducking, states).
    pub const MIXER: Code = Code(3407);
    /// A music document is invalid (segments, stems, tempo, transitions).
    pub const MUSIC: Code = Code(3408);
    /// Acoustic geometry, bake or cooked cell data is invalid.
    pub const ACOUSTICS: Code = Code(3409);
    /// The source has no playing voice (never started, stopped or finished).
    pub const NOT_PLAYING: Code = Code(3410);
}

/// Visibility below which a direct path counts as occluded for diagnostics.
pub const OCCLUDED_BELOW: f32 = 0.25;

/// Everything needed to decide whether one source is audible, gathered by the runtime.
#[derive(Debug, Clone, PartialEq)]
pub struct Audibility {
    /// Entity subject (`e_8b21` or `@levels/a.scene#e_8b21`).
    pub entity: String,
    /// The source component as authored.
    pub source: AudioSource,
    /// Whether its clip/graph is resolved and prepared.
    pub loaded: bool,
    /// The voice's state; None when no voice exists.
    pub voice: Option<VoiceState>,
    /// Name of the nearest muted bus on the source's path to master.
    pub muted_bus: Option<String>,
    /// Source-listener distance in meters.
    pub distance: f32,
    /// Distance/cone attenuation from `spatial::evaluate`, excluding source gain.
    pub attenuation: f32,
    /// The acoustics result applied to the voice.
    pub path: AcousticPath,
    /// What blocks the direct path (`concrete wall e_wall01`), nearest first.
    pub occluders: Vec<String>,
}

fn set(entity: &str, path: &str, value: serde_json::Value) -> FixAction {
    FixAction::Command {
        command: "property.set".into(),
        params: json!({ "entity": entity, "path": path, "value": value }),
    }
}

impl Audibility {
    /// Diagnostics explaining why the source is (partly) inaudible; empty when it is audible.
    /// Ordered by how completely each cause silences the source.
    pub fn explain(&self) -> Vec<Diagnostic> {
        let subject = || Subject::entity(self.entity.clone());
        let mut out = Vec::new();
        if !self.loaded {
            let clip = self.source.clip.as_ref().map_or("none", |c| c.0.as_str());
            out.push(
                Diagnostic::error(
                    codes::NOT_LOADED,
                    format!("`AudioSource.clip` (`{clip}`) is not loaded"),
                )
                .subject(subject()),
            );
        }
        if self.source.gain == 0.0 {
            out.push(
                Diagnostic::warning(codes::SILENT_SOURCE, "`AudioSource.gain` is 0")
                    .subject(subject())
                    .with_fix(
                        "Set `AudioSource.gain` to 1",
                        set(&self.entity, "AudioSource.gain", json!(1)),
                    ),
            );
        }
        if self.source.spatial && self.distance >= self.source.max_distance {
            let reach = (self.distance * 1.25).ceil();
            out.push(
                Diagnostic::warning(
                    codes::OUT_OF_RANGE,
                    format!(
                        "listener is {:.1} m away, beyond `AudioSource.maxDistance` ({} m)",
                        self.distance, self.source.max_distance
                    ),
                )
                .subject(subject())
                .data(json!({ "distance": self.distance, "maxDistance": self.source.max_distance }))
                .with_fix(
                    format!("Set `AudioSource.maxDistance` to {reach}"),
                    set(&self.entity, "AudioSource.maxDistance", json!(reach)),
                ),
            );
        } else if self.source.gain > 0.0 && self.attenuation == 0.0 {
            out.push(
                Diagnostic::warning(
                    codes::SILENT_SOURCE,
                    "distance or cone attenuation is 0 (check `coneOuterGain`)",
                )
                .subject(subject()),
            );
        }
        if let Some(bus) = &self.muted_bus {
            out.push(
                Diagnostic::warning(
                    codes::BUS_MUTED,
                    format!("bus `{bus}` on the source's path to master is muted"),
                )
                .subject(subject())
                .data(json!({ "bus": bus })),
            );
        }
        let band = self.path.band_gains();
        if self.path.visibility < OCCLUDED_BELOW {
            let by = if self.occluders.is_empty() {
                String::new()
            } else {
                format!(" by {}", self.occluders.join(", "))
            };
            out.push(
                Diagnostic::info(
                    codes::OCCLUDED,
                    format!(
                        "direct path is occluded{by}: visibility {:.2}, band gains {:.2}/{:.2}/{:.2}",
                        self.path.visibility, band[0], band[1], band[2]
                    ),
                )
                .subject(subject())
                .data(json!({ "visibility": self.path.visibility, "occluders": self.occluders })),
            );
        }
        match self.voice.map(|v| v.status) {
            Some(VoiceStatus::VirtualGlobalLimit) | Some(VoiceStatus::VirtualBusLimit) => {
                let category = matches!(
                    self.voice.map(|v| v.status),
                    Some(VoiceStatus::VirtualBusLimit)
                );
                let priority = self.source.priority.saturating_add(32);
                out.push(
                    Diagnostic::warning(
                        codes::VOICE_LIMITED,
                        if category {
                            "voice is virtual: its bus category voice limit is full"
                        } else {
                            "voice is virtual: the engine's real-voice limit is full"
                        },
                    )
                    .subject(subject())
                    .with_fix(
                        format!("Raise `AudioSource.priority` to {priority}"),
                        set(&self.entity, "AudioSource.priority", json!(priority)),
                    ),
                );
            }
            None if self.loaded => out.push(
                Diagnostic::info(codes::NOT_PLAYING, "no voice is playing for this source")
                    .subject(subject()),
            ),
            _ => (),
        }
        out
    }
}
