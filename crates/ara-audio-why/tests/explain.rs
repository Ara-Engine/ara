#![allow(clippy::unwrap_used)] // Tests may panic on unexpected failures.
//! The audio explainer on an in-memory scene and bus.

use ara_audio::{
    AudioSource, BusId, VoiceState, VoiceStatus,
    diagnose::{Audibility, codes},
    spatial::AcousticPath,
};
use ara_diag::{
    Diagnostic,
    bus::Meta,
    store::{Filter, Record, Store},
};
use ara_why::{Asset, Entity, Facts, Likelihood, Why};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
struct Scene {
    entities: BTreeMap<String, Entity>,
    bus: Store,
}

impl Scene {
    fn add(&mut self, address: &str, name: &str, components: Value) {
        self.entities.insert(
            address.into(),
            Entity {
                doc: "levels/test.scene".into(),
                address: address.into(),
                name: Some(name.into()),
                parent: None,
                components: components
                    .as_object()
                    .unwrap()
                    .clone()
                    .into_iter()
                    .collect(),
                flags: Default::default(),
            },
        );
    }
}

impl Facts for Scene {
    fn entity(&self, selector: &str, _: Option<&str>) -> Result<Entity, Vec<Diagnostic>> {
        let found = match selector.strip_prefix("name:") {
            Some(name) => self
                .entities
                .values()
                .find(|e| e.name.as_deref() == Some(name)),
            None => self.entities.get(selector),
        };
        found.cloned().ok_or_else(|| {
            vec![Diagnostic::error(
                ara_diag::codes::RECORD_NOT_FOUND,
                "no entity",
            )]
        })
    }
    fn parent(&self, _: &Entity) -> Option<Entity> {
        None
    }
    fn with_component(&self, _: &str, component: &str) -> Vec<Entity> {
        self.entities
            .values()
            .filter(|e| e.components.contains_key(component))
            .cloned()
            .collect()
    }
    fn has_component_type(&self, _: &str) -> bool {
        true
    }
    fn asset(&self, reference: &str) -> Asset {
        if reference == "@sounds/radio.ogg#au_radio01" {
            Asset {
                reference: reference.into(),
                id: Some("au_radio01".into()),
                path: Some("sounds/radio.ogg".into()),
                ..Asset::default()
            }
        } else {
            Asset {
                reference: reference.into(),
                problem: Some(Diagnostic::error(
                    ara_diag::codes::UNRESOLVED_REF,
                    "missing",
                )),
                closest: vec!["@sounds/radio.ogg#au_radio01".into()],
                ..Asset::default()
            }
        }
    }
    fn read(&self, _: &str) -> Option<String> {
        None
    }
    fn users(&self, _: &str, _: usize) -> Vec<(String, String, String)> {
        Vec::new()
    }
    fn diagnostics(&self, filter: &Filter) -> Vec<Record> {
        self.bus.query(filter).into_iter().cloned().collect()
    }
    fn causes(&self, _: &Record) -> Vec<Record> {
        Vec::new()
    }
    fn profile(&self) -> Option<Value> {
        None
    }
}

fn scene(clip: &str, max_distance: f64) -> Scene {
    let mut s = Scene::default();
    s.add(
        "e_radio1",
        "Radio",
        json!({
            "Transform": { "position": [30, 0, 0] },
            "AudioSource": { "clip": clip, "gain": 1, "maxDistance": max_distance, "spatial": true, "priority": 128 },
        }),
    );
    s.add(
        "e_cam01",
        "Camera",
        json!({ "Transform": { "position": [0, 0, 0] }, "AudioListener": {} }),
    );
    s
}

fn why() -> Why {
    let mut why = Why::builtin();
    ara_audio_why::register(&mut why);
    why
}

#[test]
fn catalog_codes_match_the_engine() {
    for (name, code) in [
        ("audio::NOT_LOADED", codes::NOT_LOADED),
        ("audio::OUT_OF_RANGE", codes::OUT_OF_RANGE),
        ("audio::OCCLUDED", codes::OCCLUDED),
        ("audio::BUS_MUTED", codes::BUS_MUTED),
        ("audio::VOICE_LIMITED", codes::VOICE_LIMITED),
        ("audio::SILENT_SOURCE", codes::SILENT_SOURCE),
        ("audio::NOT_PLAYING", codes::NOT_PLAYING),
    ] {
        assert_eq!(
            ara_diag::catalog::lookup(name).unwrap().code,
            code,
            "{name}"
        );
    }
}

#[test]
fn out_of_range_sources_get_a_runnable_fix() {
    let report = why()
        .explain(
            "why can't I hear name:Radio",
            &scene("@sounds/radio.ogg#au_radio01", 20.0),
        )
        .unwrap();
    assert_eq!(report.explainer, "audio");
    let top = &report.causes[0];
    assert_eq!(
        (top.code, top.likelihood),
        (codes::OUT_OF_RANGE, Likelihood::High)
    );
    assert_eq!(
        top.fixes[0].action,
        ara_diag::FixAction::Command {
            command: "property.set".into(),
            params: json!({ "entity": "e_radio1", "path": "AudioSource.maxDistance", "value": 38.0 }),
        }
    );
    assert!(report.text().contains("30 m"), "{}", report.text());
}

#[test]
fn missing_clips_offer_the_closest_asset() {
    let report = why()
        .explain("radio is silent", &scene("@sounds/raido.ogg", 100.0))
        .unwrap();
    assert_eq!(report.causes[0].code, codes::NOT_LOADED);
    assert!(format!("{:?}", report.causes[0].fixes).contains("au_radio01"));
}

#[test]
fn runtime_reports_explain_occlusion_mutes_and_voice_limits() {
    let mut s = scene("@sounds/radio.ogg#au_radio01", 100.0);
    // What the runtime emits for this source: the engine's own audibility explanation.
    let facts = Audibility {
        entity: "e_radio1".into(),
        source: AudioSource::default(),
        loaded: true,
        voice: Some(VoiceState {
            bus: BusId::MASTER,
            status: VoiceStatus::VirtualBusLimit,
            gain: 0.2,
            trim: 1.0,
            position: 0,
        }),
        muted_bus: Some("sfx".into()),
        distance: 30.0,
        attenuation: 0.03,
        path: AcousticPath {
            visibility: 0.0,
            transmission: [0.015, 0.002, 0.001],
            ..AcousticPath::default()
        },
        occluders: vec!["e_wall01 (concrete)".into()],
    };
    for d in facts.explain() {
        s.bus.push(d, Meta::default());
    }
    let report = why().explain("why can't I hear e_radio1", &s).unwrap();
    let found: Vec<_> = report
        .causes
        .iter()
        .map(|c| (c.code, c.likelihood))
        .collect();
    assert!(
        found.contains(&(codes::BUS_MUTED, Likelihood::High)),
        "{found:?}"
    );
    assert!(
        found.contains(&(codes::VOICE_LIMITED, Likelihood::High)),
        "{found:?}"
    );
    assert!(
        found.contains(&(codes::OCCLUDED, Likelihood::Medium)),
        "{found:?}"
    );
    let limited = report
        .causes
        .iter()
        .find(|c| c.code == codes::VOICE_LIMITED)
        .unwrap();
    assert!(!limited.fixes.is_empty(), "runtime fixes are carried over");
    assert!(report.text().contains("e_wall01"));
}

#[test]
fn visibility_questions_still_go_to_visibility() {
    let report = why()
        .explain(
            "why is name:Radio not visible",
            &scene("@sounds/radio.ogg#au_radio01", 100.0),
        )
        .unwrap();
    assert_eq!(report.explainer, "visibility");
}
