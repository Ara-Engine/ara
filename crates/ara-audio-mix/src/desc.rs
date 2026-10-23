//! Reflected mixer records and the validated `MixerDesc` read from a `mixer` document.

use ara_audio::diagnose::codes;
use ara_diag::Diagnostic;
use ara_doc::Document;
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Sidechain ducking: another bus's level pulls this bus down (dialogue over music).
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "audio", name = "AudioDuck")]
#[serde(default, rename_all = "camelCase")]
pub struct DuckDesc {
    /// Name of the bus whose level triggers the duck.
    pub by: String,
    /// Trigger level of that bus's block peak.
    #[ara(unit = Decibels, default = -30, range(min = -96, max = 0))]
    pub threshold_db: f32,
    /// Attenuation while ducked.
    #[ara(unit = Decibels, default = -9, range(min = -60, max = 0))]
    pub amount_db: f32,
    /// Seconds to reach the ducked level.
    #[ara(unit = Seconds, default = 0.05, range(min = 0.001, max = 10))]
    pub attack: f32,
    /// Seconds to recover.
    #[ara(unit = Seconds, default = 0.4, range(min = 0.001, max = 10))]
    pub release: f32,
}

impl Default for DuckDesc {
    fn default() -> Self {
        Self {
            by: String::new(),
            threshold_db: -30.0,
            amount_db: -9.0,
            attack: 0.05,
            release: 0.4,
        }
    }
}

/// One bus (category) in the mixer tree. `AudioSource.bus` names it.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "b"), module = "audio", name = "AudioBus")]
#[serde(default, rename_all = "camelCase")]
pub struct BusDesc {
    /// Unique bus name; `master` configures the root bus.
    pub name: String,
    /// Bus this one mixes into; empty means `master`. (`parent` is reserved by the kernel.)
    pub output: String,
    /// Base gain.
    #[ara(unit = Decibels, default = 0, range(min = -96, max = 24))]
    pub gain_db: f32,
    /// Silences the bus and its reverb sends.
    pub mute: bool,
    /// Real-voice cap for this category and its children; 0 is unlimited.
    pub voice_limit: u32,
    /// Integrated loudness target for sources on this bus (music and dialogue: -16 LUFS).
    #[ara(unit = Decibels)]
    pub loudness: Option<f32>,
    /// Route spatial voices on this bus to the listener's room reverb.
    pub reverb: bool,
    /// Sidechain ducking driven by another bus.
    pub duck: Option<DuckDesc>,
}

impl Default for BusDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            output: String::new(),
            gain_db: 0.0,
            mute: false,
            voice_limit: 0,
            loudness: None,
            reverb: false,
            duck: None,
        }
    }
}

/// A named mix (combat, explore, menu): per-bus gain offsets reached over `fade` seconds.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "s"), module = "audio", name = "AudioMixState")]
#[serde(default, rename_all = "camelCase")]
pub struct MixStateDesc {
    /// Unique state name.
    pub name: String,
    /// Gain offsets in dB added to each named bus's base gain; -96 or below mutes.
    pub buses: BTreeMap<String, f32>,
    /// Transition time into this state.
    #[ara(unit = Seconds, default = 1, range(min = 0, max = 60))]
    pub fade: f32,
}

impl Default for MixStateDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            buses: BTreeMap::new(),
            fade: 1.0,
        }
    }
}

/// Mixer-wide settings (one record per mixer document).
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "m"), module = "audio", name = "AudioMixerSettings")]
#[serde(default, rename_all = "camelCase")]
pub struct MixerSettings {
    /// State applied at startup; empty is the base mix.
    pub initial_state: String,
}

/// A validated mixer: buses in creation order (parents and duck sources first).
#[derive(Debug, Clone, PartialEq)]
pub struct MixerDesc {
    /// Buses, master first if configured, then dependency order.
    pub buses: Vec<BusDesc>,
    /// Mix states by name.
    pub states: BTreeMap<String, MixStateDesc>,
    /// Settings.
    pub settings: MixerSettings,
}

fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::MIXER, message)
}

impl MixerDesc {
    /// Reads and validates a `mixer` document.
    pub fn from_document(doc: &Document) -> Result<Self, Vec<Diagnostic>> {
        if doc.kind != "mixer" {
            return Err(vec![error("expected a mixer document")]);
        }
        let mut buses = Vec::new();
        let mut states = Vec::new();
        let mut settings = MixerSettings::default();
        let mut diags = Vec::new();
        for (id, record) in doc.records() {
            let value = serde_json::Value::Object(record.clone());
            let decoded = match id.prefix() {
                "b" => serde_json::from_value(value).map(|b| buses.push(b)),
                "s" => serde_json::from_value(value).map(|s| states.push(s)),
                "m" => serde_json::from_value(value).map(|m| settings = m),
                _ => {
                    diags
                        .push(error(format!("unexpected mixer record `{id}`")).record(id.as_str()));
                    continue;
                }
            };
            if let Err(e) = decoded {
                diags.push(error(e.to_string()).record(id.as_str()));
            }
        }
        if !diags.is_empty() {
            return Err(diags);
        }
        Self::new(buses, states, settings)
    }

    /// Validates and orders buses: unique names, known parents and duck sources, no cycles,
    /// finite values, states naming real buses.
    pub fn new(
        buses: Vec<BusDesc>,
        states: Vec<MixStateDesc>,
        settings: MixerSettings,
    ) -> Result<Self, Vec<Diagnostic>> {
        let mut diags = Vec::new();
        let mut names = BTreeSet::from(["master"]);
        for b in &buses {
            if b.name.is_empty() || (b.name != "master" && !names.insert(b.name.as_str())) {
                diags.push(error(format!(
                    "bus name `{}` is empty or duplicated",
                    b.name
                )));
            }
            if b.name == "master" && !b.output.is_empty() {
                diags.push(error("`master` has no parent"));
            }
            let finite = b.gain_db.is_finite()
                && b.loudness
                    .is_none_or(|l| l.is_finite() && (-70.0..=0.0).contains(&l))
                && b.duck.as_ref().is_none_or(|d| {
                    [d.threshold_db, d.amount_db, d.attack, d.release]
                        .iter()
                        .all(|v| v.is_finite())
                        && d.attack > 0.0
                        && d.release > 0.0
                        && d.amount_db <= 0.0
                });
            if !finite {
                diags.push(error(format!(
                    "bus `{}` has invalid gain, loudness or ducking",
                    b.name
                )));
            }
        }
        for b in &buses {
            let parent = if b.output.is_empty() {
                "master"
            } else {
                &b.output
            };
            if b.name != "master" && !names.contains(parent) {
                diags.push(error(format!(
                    "bus `{}` has unknown parent `{parent}`",
                    b.name
                )));
            }
            if let Some(d) = &b.duck
                && (!names.contains(d.by.as_str()) || d.by == b.name)
            {
                diags.push(error(format!(
                    "bus `{}` ducks by unknown or own bus `{}`",
                    b.name, d.by
                )));
            }
        }
        // Creation order: a bus after its parent and its duck source.
        let mut ordered: Vec<BusDesc> = Vec::with_capacity(buses.len());
        let mut created = BTreeSet::from(["master".to_owned()]);
        let mut remaining: Vec<BusDesc> = buses
            .iter()
            .filter(|b| b.name != "master")
            .cloned()
            .collect();
        if let Some(master) = buses.iter().find(|b| b.name == "master") {
            ordered.push(master.clone());
        }
        while !remaining.is_empty() {
            let ready = remaining.iter().position(|b| {
                let parent = if b.output.is_empty() {
                    "master"
                } else {
                    &b.output
                };
                created.contains(parent) && b.duck.as_ref().is_none_or(|d| created.contains(&d.by))
            });
            let Some(i) = ready else {
                if diags.is_empty() {
                    diags.push(error("bus parents or duck sources form a cycle"));
                }
                break;
            };
            let bus = remaining.remove(i);
            created.insert(bus.name.clone());
            ordered.push(bus);
        }
        let mut by_name = BTreeMap::new();
        for s in states {
            if s.name.is_empty() || !s.fade.is_finite() || s.fade < 0.0 {
                diags.push(error(format!(
                    "state `{}` needs a name and a fade >= 0",
                    s.name
                )));
            }
            for (bus, offset) in &s.buses {
                if !names.contains(bus.as_str()) || !offset.is_finite() {
                    diags.push(error(format!(
                        "state `{}` sets unknown bus `{bus}`",
                        s.name
                    )));
                }
            }
            if by_name.insert(s.name.clone(), s).is_some() {
                diags.push(error("duplicate mix state name"));
            }
        }
        if !settings.initial_state.is_empty() && !by_name.contains_key(&settings.initial_state) {
            diags.push(error(format!(
                "initial state `{}` does not exist",
                settings.initial_state
            )));
        }
        if diags.is_empty() {
            Ok(Self {
                buses: ordered,
                states: by_name,
                settings,
            })
        } else {
            Err(diags)
        }
    }
}
