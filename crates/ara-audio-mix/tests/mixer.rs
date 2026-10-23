#![allow(clippy::unwrap_used)] // Tests may panic on unexpected failures.
//! Mixer documents: validation, engine build, ducking, limits and mix states.

use ara_audio::{
    AudioEngine, AuxEffect, BusId, Clip, EngineConfig, Frame, Playback, VoiceStatus,
    capture::capture_audio,
};
use ara_audio_mix::*;
use ara_doc::{AssetId, Document, RecordId};
use ara_schema::SchemaDb;
use std::collections::BTreeMap;

fn bus(name: &str, parent: &str) -> BusDesc {
    BusDesc {
        name: name.into(),
        output: parent.into(),
        ..BusDesc::default()
    }
}

fn document() -> Document {
    let mut doc = Document::new("mixer", AssetId::parse("mx_main").unwrap());
    let mut put = |id: &str, value: serde_json::Value| {
        let serde_json::Value::Object(record) = value else {
            panic!()
        };
        doc.insert_unchecked(RecordId::parse(id).unwrap(), record);
    };
    put(
        "b_music",
        ara_json::to_value(&BusDesc {
            loudness: Some(-16.0),
            duck: Some(DuckDesc {
                by: "dialogue".into(),
                ..DuckDesc::default()
            }),
            ..bus("music", "")
        })
        .unwrap(),
    );
    put(
        "b_dialogue",
        ara_json::to_value(&BusDesc {
            loudness: Some(-16.0),
            ..bus("dialogue", "")
        })
        .unwrap(),
    );
    put(
        "b_world",
        ara_json::to_value(&BusDesc {
            reverb: true,
            ..bus("world", "")
        })
        .unwrap(),
    );
    put(
        "b_steps",
        ara_json::to_value(&BusDesc {
            voice_limit: 2,
            ..bus("footsteps", "world")
        })
        .unwrap(),
    );
    put(
        "s_combat",
        ara_json::to_value(&MixStateDesc {
            name: "combat".into(),
            buses: BTreeMap::from([("music".into(), 6.0), ("footsteps".into(), -96.0)]),
            fade: 0.5,
        })
        .unwrap(),
    );
    put(
        "m_settings",
        ara_json::to_value(&MixerSettings {
            initial_state: String::new(),
        })
        .unwrap(),
    );
    doc
}

#[test]
fn schema_registers_and_documents_round_trip() {
    let mut db = SchemaDb::new();
    register(&mut db).unwrap();
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    for (name, value) in [
        ("AudioBus", ara_json::to_value(&BusDesc::default()).unwrap()),
        (
            "AudioMixState",
            ara_json::to_value(&MixStateDesc::default()).unwrap(),
        ),
        (
            "AudioDuck",
            ara_json::to_value(&DuckDesc::default()).unwrap(),
        ),
    ] {
        let desc = db.get(name).unwrap();
        for field in desc.fields() {
            assert!(!field.doc.is_empty(), "{name}.{}", field.name);
            assert!(
                ara_json::json_eq(
                    &db.canonical_value(&field.ty, &value[&field.name]),
                    &db.canonical_value(&field.ty, &db.field_default(field))
                ),
                "{name}.{}",
                field.name
            );
        }
    }
    let text = ara_doc::write(&document(), &db);
    let read = ara_doc::read(&text, "audio/main.mixer", &db, None);
    assert!(!ara_diag::has_errors(&read.diags), "{:?}", read.diags);
    let desc = MixerDesc::from_document(&read.doc.unwrap()).unwrap();
    let order: Vec<&str> = desc.buses.iter().map(|b| b.name.as_str()).collect();
    let position = |n: &str| order.iter().position(|o| *o == n).unwrap();
    assert!(
        position("dialogue") < position("music"),
        "duck source first: {order:?}"
    );
    assert!(
        position("world") < position("footsteps"),
        "parent first: {order:?}"
    );
}

#[test]
fn invalid_mixers_explain_themselves() {
    let message = |buses: Vec<BusDesc>, states: Vec<MixStateDesc>| {
        MixerDesc::new(buses, states, MixerSettings::default())
            .unwrap_err()
            .into_iter()
            .map(|d| d.message)
            .collect::<Vec<_>>()
            .join("; ")
    };
    assert!(message(vec![bus("a", "nowhere")], vec![]).contains("unknown parent `nowhere`"));
    assert!(message(vec![bus("a", ""), bus("a", "")], vec![]).contains("duplicated"));
    let mut a = bus("a", "");
    a.duck = Some(DuckDesc {
        by: "b".into(),
        ..DuckDesc::default()
    });
    let mut b = bus("b", "");
    b.duck = Some(DuckDesc {
        by: "a".into(),
        ..DuckDesc::default()
    });
    assert!(message(vec![a, b], vec![]).contains("cycle"));
    let state = MixStateDesc {
        name: "s".into(),
        buses: BTreeMap::from([("ghost".into(), 0.0)]),
        fade: 1.0,
    };
    assert!(message(vec![], vec![state]).contains("unknown bus `ghost`"));
}

fn tone(freq: f32, amplitude: f32) -> Clip {
    let frames = (0..48000)
        .map(|i| {
            let s = (i as f32 / 48000.0 * freq * std::f32::consts::TAU).sin() * amplitude;
            Frame::new(s, s)
        })
        .collect();
    Clip::new(48000, frames, 1 << 24).unwrap()
}

struct Silence;
impl AuxEffect for Silence {
    fn process(&mut self, _: &[f32], out: &mut [Frame], _: u32) {
        out.fill(Frame::ZERO);
    }
}

fn engine() -> AudioEngine {
    AudioEngine::new(EngineConfig {
        real_voices: 8,
        total_voices: 16,
        buses: 16,
        limiter: None,
        ..EngineConfig::default()
    })
    .unwrap()
}

fn music_rms(with_dialogue: bool) -> f64 {
    let mut e = engine();
    let room = e.add_aux("room", Box::new(Silence)).unwrap();
    let mixer = Mixer::build(
        &mut e,
        MixerDesc::from_document(&document()).unwrap(),
        Some(room),
    )
    .unwrap();
    let flat = || Playback {
        looping: true,
        spatial: false,
        ..Playback::default()
    };
    let music = mixer.bus("music").unwrap();
    e.play(music, tone(220.0, 0.2), flat()).unwrap();
    if with_dialogue {
        e.play(mixer.bus("dialogue").unwrap(), tone(700.0, 0.5), flat())
            .unwrap();
    }
    let c = capture_audio(&mut e, 1.0).unwrap();
    c.analysis
        .buses
        .iter()
        .find(|b| b.name == "music")
        .unwrap()
        .rms_dbfs
        .unwrap()
}

#[test]
fn dialogue_ducks_music() {
    let alone = music_rms(false);
    let ducked = music_rms(true);
    assert!(alone - ducked > 6.0, "{alone} vs {ducked}");
}

#[test]
fn limits_routes_and_states_apply_to_the_engine() {
    let mut e = engine();
    let room = e.add_aux("room", Box::new(Silence)).unwrap();
    let mut mixer = Mixer::build(
        &mut e,
        MixerDesc::from_document(&document()).unwrap(),
        Some(room),
    )
    .unwrap();
    let steps = mixer.bus("footsteps").unwrap();
    let voices: Vec<_> = (0..3)
        .map(|i| {
            e.play(
                steps,
                tone(300.0 + i as f32 * 50.0, 0.1),
                Playback {
                    looping: true,
                    priority: 100 + i,
                    ..Playback::default()
                },
            )
            .unwrap()
        })
        .collect();
    let mut out = vec![0.0; 256];
    e.render(&mut out).unwrap();
    let statuses: Vec<_> = voices
        .iter()
        .map(|v| e.voice_state(*v).unwrap().status)
        .collect();
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == VoiceStatus::VirtualBusLimit)
            .count(),
        1
    );
    assert_eq!(
        statuses[0],
        VoiceStatus::VirtualBusLimit,
        "lowest priority yields"
    );
    mixer.set_state(&mut e, Some("combat"), None).unwrap();
    assert_eq!(mixer.state(), Some("combat"));
    assert_eq!(e.bus_gain(steps), Some(0.0));
    assert!((e.bus_gain(mixer.bus("music").unwrap()).unwrap() - linear(6.0)).abs() < 1e-6);
    assert_eq!(e.muted_ancestor(steps), Some(steps));
    mixer.set_state(&mut e, None, Some(0.0)).unwrap();
    assert_eq!(e.bus_gain(steps), Some(1.0));
    assert!(mixer.set_state(&mut e, Some("nope"), None).is_err());
    assert!(mixer.bus("world").is_some() && mixer.bus("master") == Some(BusId::MASTER));
}
