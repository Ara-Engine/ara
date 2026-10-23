//! Reflected music records (`music` documents) and the validated `MusicDesc`.

use ara_audio::diagnose::codes;
use ara_diag::Diagnostic;
use ara_doc::Document;
use ara_schema::{AssetRef, Schema};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Where a transition or stinger may land.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "audio", name = "MusicSync")]
#[serde(rename_all = "camelCase")]
pub enum MusicSync {
    /// On the next bar line.
    #[default]
    Bar,
    /// On the next beat.
    Beat,
    /// When the current segment ends.
    End,
    /// At the next control update.
    Immediate,
}

/// A tempo and meter that applies from `bar` until the next change.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "audio", name = "MusicTempo")]
#[serde(default, rename_all = "camelCase")]
pub struct TempoChange {
    /// First bar (0-based) at this tempo.
    pub bar: u32,
    /// Beats per minute.
    #[ara(default = 120, range(min = 20, max = 400))]
    pub bpm: f32,
    /// Beats in each bar.
    #[ara(default = 4, range(min = 1, max = 32))]
    pub beats_per_bar: u32,
}

impl Default for TempoChange {
    fn default() -> Self {
        Self {
            bar: 0,
            bpm: 120.0,
            beats_per_bar: 4,
        }
    }
}

/// A synchronized layer (stem) of a segment, faded in by intensity.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "audio", name = "MusicStem")]
#[serde(default, rename_all = "camelCase")]
pub struct StemDesc {
    /// Stem name (`drums`, `strings`).
    pub name: String,
    /// The stem's audio clip.
    #[ara(asset_kind = "audio")]
    pub clip: Option<AssetRef>,
    /// Intensity window: silent at or below `[0]`, full at or above `[1]`; equal values
    /// make a hard switch, `[0, 0]` is always on.
    pub intensity: [f32; 2],
    /// Stem gain.
    #[ara(unit = Decibels, range(min = -96, max = 24))]
    pub gain_db: f32,
}

impl Default for StemDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            clip: None,
            intensity: [0.0, 0.0],
            gain_db: 0.0,
        }
    }
}

/// A weighted follow-up for horizontal resequencing.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "audio", name = "MusicNext")]
#[serde(default, rename_all = "camelCase")]
pub struct NextDesc {
    /// Segment to continue with.
    pub segment: String,
    /// Relative probability.
    #[ara(default = 1, range(min = 0, max = 1000))]
    pub weight: f32,
}

impl Default for NextDesc {
    fn default() -> Self {
        Self {
            segment: String::new(),
            weight: 1.0,
        }
    }
}

/// A playable section with synchronized stems on one tempo map.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "sg"), module = "audio", name = "MusicSegment")]
#[serde(default, rename_all = "camelCase")]
pub struct SegmentDesc {
    /// Unique segment name.
    pub name: String,
    /// Length in bars; the follow-up starts on the last bar line.
    #[ara(default = 4, range(min = 1, max = 4096))]
    pub bars: u32,
    /// Tempo map; empty is 120 BPM in 4/4. The first change must be at bar 0.
    pub tempo: Vec<TempoChange>,
    /// Layers played together.
    pub stems: Vec<StemDesc>,
    /// Weighted follow-ups; empty repeats this segment.
    pub next: Vec<NextDesc>,
    /// Cooked integrated loudness of the full mix (all stems), so a loudness-targeted
    /// bus trims every stem equally. Measure with `measure_mix_loudness`.
    #[ara(unit = Decibels)]
    pub loudness: Option<f32>,
}

impl Default for SegmentDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            bars: 4,
            tempo: Vec::new(),
            stems: Vec::new(),
            next: Vec::new(),
            loudness: None,
        }
    }
}

/// How a requested change of segment lands. An empty `from` matches any segment.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "tn"), module = "audio", name = "MusicTransition")]
#[serde(default, rename_all = "camelCase")]
pub struct TransitionDesc {
    /// Segment being left; empty matches any.
    pub from: String,
    /// Segment requested.
    pub to: String,
    /// Musical boundary to land on.
    pub sync: MusicSync,
    /// Crossfade length in seconds; zero cuts.
    #[ara(unit = Seconds, default = 0.5, range(min = 0, max = 30))]
    pub fade: f32,
    /// Optional bridge segment played between them.
    pub bridge: String,
}

impl Default for TransitionDesc {
    fn default() -> Self {
        Self {
            from: String::new(),
            to: String::new(),
            sync: MusicSync::Bar,
            fade: 0.5,
            bridge: String::new(),
        }
    }
}

/// A one-shot played on top of the music, aligned to the grid.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "st"), module = "audio", name = "MusicStinger")]
#[serde(default, rename_all = "camelCase")]
pub struct StingerDesc {
    /// Unique stinger name.
    pub name: String,
    /// The stinger's clip.
    #[ara(asset_kind = "audio")]
    pub clip: Option<AssetRef>,
    /// Grid alignment.
    pub sync: MusicSync,
    /// Stinger gain.
    #[ara(unit = Decibels, range(min = -96, max = 24))]
    pub gain_db: f32,
}

impl Default for StingerDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            clip: None,
            sync: MusicSync::Bar,
            gain_db: 0.0,
        }
    }
}

/// Music-wide settings (one record).
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "ms"), module = "audio", name = "MusicSettings")]
#[serde(default, rename_all = "camelCase")]
pub struct MusicSettings {
    /// Segment to start with.
    pub initial: String,
    /// Starting intensity 0..1.
    #[ara(default = 0.5, range(min = 0, max = 1))]
    pub intensity: f32,
    /// Seconds for layers to follow an intensity change.
    #[ara(unit = Seconds, default = 1, range(min = 0, max = 30))]
    pub layer_fade: f32,
    /// Seed for weighted follow-ups (deterministic resequencing).
    pub seed: u32,
}

impl Default for MusicSettings {
    fn default() -> Self {
        Self {
            initial: String::new(),
            intensity: 0.5,
            layer_fade: 1.0,
            seed: 0,
        }
    }
}

/// A validated music description.
#[derive(Debug, Clone, PartialEq)]
pub struct MusicDesc {
    /// Segments in name order.
    pub segments: Vec<SegmentDesc>,
    /// Transition rules: specific `from` rules win over wildcards.
    pub transitions: Vec<TransitionDesc>,
    /// Stingers by name.
    pub stingers: BTreeMap<String, StingerDesc>,
    /// Settings.
    pub settings: MusicSettings,
}

fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::MUSIC, message)
}

impl MusicDesc {
    /// Reads and validates a `music` document.
    pub fn from_document(doc: &Document) -> Result<Self, Vec<Diagnostic>> {
        if doc.kind != "music" {
            return Err(vec![error("expected a music document")]);
        }
        let (mut segments, mut transitions, mut stingers) = (Vec::new(), Vec::new(), Vec::new());
        let mut settings = MusicSettings::default();
        let mut diags = Vec::new();
        for (id, record) in doc.records() {
            let value = serde_json::Value::Object(record.clone());
            let decoded = match id.prefix() {
                "sg" => serde_json::from_value(value).map(|s| segments.push(s)),
                "tn" => serde_json::from_value(value).map(|t| transitions.push(t)),
                "st" => serde_json::from_value(value).map(|s| stingers.push(s)),
                "ms" => serde_json::from_value(value).map(|s| settings = s),
                _ => {
                    diags
                        .push(error(format!("unexpected music record `{id}`")).record(id.as_str()));
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
        Self::new(segments, transitions, stingers, settings)
    }

    /// Validates names, tempo maps, references and numbers.
    pub fn new(
        mut segments: Vec<SegmentDesc>,
        transitions: Vec<TransitionDesc>,
        stingers: Vec<StingerDesc>,
        settings: MusicSettings,
    ) -> Result<Self, Vec<Diagnostic>> {
        let mut diags = Vec::new();
        segments.sort_by(|a, b| a.name.cmp(&b.name));
        let names: BTreeSet<&str> = segments.iter().map(|s| s.name.as_str()).collect();
        if names.len() != segments.len() || names.contains("") {
            diags.push(error("segment names must be unique and non-empty"));
        }
        for s in &segments {
            let tempo_ok = s.tempo.first().is_none_or(|t| t.bar == 0)
                && s.tempo.windows(2).all(|w| w[0].bar < w[1].bar)
                && s.tempo.iter().all(|t| {
                    t.bpm.is_finite()
                        && (20.0..=400.0).contains(&t.bpm)
                        && (1..=32).contains(&t.beats_per_bar)
                });
            if !tempo_ok || s.bars == 0 {
                diags.push(error(format!(
                    "segment `{}` needs bars >= 1 and a tempo map starting at bar 0 (20-400 BPM)",
                    s.name
                )));
            }
            for stem in &s.stems {
                if stem.clip.is_none()
                    || !stem.gain_db.is_finite()
                    || stem.intensity.iter().any(|v| !v.is_finite())
                {
                    diags.push(error(format!(
                        "stem `{}` of `{}` needs a clip and finite values",
                        stem.name, s.name
                    )));
                }
            }
            for next in &s.next {
                if !names.contains(next.segment.as_str())
                    || !next.weight.is_finite()
                    || next.weight < 0.0
                {
                    diags.push(error(format!(
                        "segment `{}` continues to unknown `{}`",
                        s.name, next.segment
                    )));
                }
            }
            if s.loudness.is_some_and(|l| !l.is_finite()) {
                diags.push(error(format!(
                    "segment `{}` loudness must be finite",
                    s.name
                )));
            }
        }
        for t in &transitions {
            let known = |n: &str| n.is_empty() || names.contains(n);
            if !names.contains(t.to.as_str())
                || !known(&t.from)
                || !known(&t.bridge)
                || !t.fade.is_finite()
                || t.fade < 0.0
            {
                diags.push(error(format!(
                    "transition `{}` -> `{}` names unknown segments",
                    t.from, t.to
                )));
            }
        }
        let mut by_name = BTreeMap::new();
        for s in stingers {
            if s.name.is_empty() || s.clip.is_none() || !s.gain_db.is_finite() {
                diags.push(error(format!(
                    "stinger `{}` needs a name, a clip and a finite gain",
                    s.name
                )));
            }
            if by_name.insert(s.name.clone(), s).is_some() {
                diags.push(error("duplicate stinger name"));
            }
        }
        if !settings.initial.is_empty() && !names.contains(settings.initial.as_str()) {
            diags.push(error(format!(
                "initial segment `{}` does not exist",
                settings.initial
            )));
        }
        if diags.is_empty() {
            Ok(Self {
                segments,
                transitions,
                stingers: by_name,
                settings,
            })
        } else {
            Err(diags)
        }
    }

    /// Index of a segment by name.
    pub fn segment(&self, name: &str) -> Option<usize> {
        self.segments.iter().position(|s| s.name == name)
    }
}
