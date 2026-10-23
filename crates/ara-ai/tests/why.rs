#![allow(clippy::unwrap_used)]
//! `ara why` for NPCs: live diagnoses from a running sim, explained over facts.
mod common;
use ara_ai::{why::NpcExplainer, *};
use ara_diag::bus::Meta;
use ara_diag::store::{Filter, Record, Store};
use ara_diag::{Diagnostic, Subject};
use ara_nav::{Geometry, NavAgentState};
use ara_perception::{Perceiver, Perception};
use ara_why::{Asset, Entity, Facts, Likelihood, Status, Why};
use common::*;
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;

/// Map with a sealed room (x 50..58, z 2..10) and a long wall hiding the east.
fn map() -> ara_nav::NavMesh {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, 60.0], [0.0, 40.0], 0.0, 0);
    solid(&mut g, [28.0, 30.0], [0.0, 40.0]);
    solid(&mut g, [48.0, 50.0], [0.0, 12.0]);
    solid(&mut g, [48.0, 60.0], [10.0, 12.0]);
    bake(g, [60.0, 40.0])
}

fn diagnosis(sim: &mut Sim, npc: bevy_ecs::prelude::Entity) -> Option<Diagnostic> {
    let state = sim.world.get::<BrainState>(npc).unwrap().clone();
    let ai = sim.world.resource::<AiWorld>();
    let program = ai.program(&state.program).unwrap().clone();
    let stuck = (ai.config().stuck_seconds / DT).round() as u32;
    why::diagnose(
        &program,
        &state,
        sim.world.get::<NavAgentState>(npc),
        sim.world.get::<Perception>(npc),
        stuck,
        DT,
    )
}

fn live_diagnoses() -> Vec<Diagnostic> {
    let mut sim = Sim::new(map());
    let text = |b: &str| b.to_owned();
    sim.world
        .resource_mut::<AiWorld>()
        .load(
            "go",
            &text(
                "behavior Go { var goal: vec3 = vec3(55, 0, 5); state Walk { do moveTo(goal); } }",
            ),
        )
        .unwrap();
    sim.world
        .resource_mut::<AiWorld>()
        .load(
            "nap",
            "behavior Nap { state Rest { do wait(0.5); } state Other { do wait(1); } }",
        )
        .unwrap();
    sim.world
        .resource_mut::<AiWorld>()
        .load("watch", "behavior Watch { state Wait { do wait(0.1); on sees(nearest(\"player\")) -> Alarm; } state Alarm { do wait(1); } }")
        .unwrap();
    let walker = sim.npc(1, "go", [5.0, 0.1, 5.0], Perceiver::default(), &[]);
    let napper = sim.npc(2, "nap", [5.0, 0.1, 20.0], Perceiver::default(), &[]);
    let watcher = sim.npc(3, "watch", [20.0, 0.1, 20.0], Perceiver::default(), &[]);
    sim.world
        .get_mut::<ara_perception::PerceptionPose>(watcher)
        .unwrap()
        .forward = [1.0, 0.0, 0.0];
    sim.actor(99, "player", [40.0, 0.1, 20.0]); // Facing it, but behind the wall.
    for _ in 0..1300 {
        sim.step();
    }
    let walk = diagnosis(&mut sim, walker).expect("unreachable goal is diagnosed");
    assert_eq!(walk.code, codes::NO_PATH, "{}", walk.message);
    let nap = diagnosis(&mut sim, napper).expect("finished tasks without a transition");
    assert_eq!(nap.code, codes::STUCK, "{}", nap.message);
    assert!(nap.message.contains("`Rest`"), "{}", nap.message);
    let watch = diagnosis(&mut sim, watcher).expect("perception never fired");
    assert_eq!(watch.code, codes::UNSEEN, "{}", watch.message);
    let checks = &watch.data.as_ref().unwrap()["checks"];
    assert_eq!(checks[0]["outcome"], "Occluded", "{checks}");
    vec![walk, nap, watch]
}

#[derive(Default)]
struct Fixture {
    entities: BTreeMap<String, Entity>,
    bus: Store,
}

impl Fixture {
    fn add(&mut self, address: &str, name: &str, components: Json) {
        self.entities.insert(
            address.into(),
            Entity {
                doc: "levels/yard.scene".into(),
                address: address.into(),
                name: Some(name.into()),
                parent: None,
                components: components
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                flags: Default::default(),
            },
        );
    }
}

impl Facts for Fixture {
    fn entity(&self, selector: &str, _: Option<&str>) -> Result<Entity, Vec<Diagnostic>> {
        let name = selector.strip_prefix("name:").unwrap_or(selector);
        self.entities
            .values()
            .find(|e| e.name.as_deref() == Some(name) || e.address == selector)
            .cloned()
            .ok_or_else(|| {
                vec![Diagnostic::error(
                    ara_diag::codes::RECORD_NOT_FOUND,
                    format!("no `{selector}`"),
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
        Asset {
            reference: reference.into(),
            id: Some("gr_guard001".into()),
            path: Some("ai/guard.behavior".into()),
            kind: Some("graph".into()),
            ..Asset::default()
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
    fn profile(&self) -> Option<Json> {
        None
    }
}

#[test]
fn npc_explainer_ranks_live_causes_and_setup_problems() {
    let diagnoses = live_diagnoses();
    let mut f = Fixture::default();
    f.add(
        "e_guard01",
        "Guard",
        json!({
            "Brain": {"behavior": "@ai/guard.behavior#gr_guard001", "overrides": [], "enabled": true},
            "NavAgent": {"maxSpeed": 3.0},
        }),
    );
    f.add(
        "e_ground1",
        "Ground",
        json!({"NavMeshSurface": {"profiles": [0]}}),
    );
    f.add(
        "e_statue1",
        "Statue",
        json!({"NavAgent": {"maxSpeed": 0.0}}),
    );
    for d in diagnoses {
        f.bus
            .push(d.subject(Subject::entity("e_guard01")), Meta::default());
    }
    let mut why = Why::builtin();
    why.register(Box::new(NpcExplainer));
    let report = why.explain("why isn't name:Guard moving", &f).unwrap();
    assert_eq!(report.explainer, "npc");
    let codes_found: Vec<_> = report.causes.iter().map(|c| c.code).collect();
    assert!(codes_found.contains(&codes::NO_PATH), "{}", report.text());
    assert!(codes_found.contains(&codes::STUCK), "{}", report.text());
    assert!(codes_found.contains(&codes::UNSEEN), "{}", report.text());
    let stuck = report
        .causes
        .iter()
        .find(|c| c.code == codes::STUCK)
        .unwrap();
    assert_eq!(stuck.likelihood, Likelihood::High);
    assert!(
        stuck.evidence.iter().any(|e| e.contains("active states")),
        "{:?}",
        stuck.evidence
    );
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.name == "navmesh" && c.status == Status::Ok)
    );

    // Setup problems are reported without any runtime data.
    let report = why.explain("why is name:Statue standing", &f).unwrap();
    let text = report.text();
    assert!(
        report.causes.iter().any(|c| c.code == codes::NO_BEHAVIOR),
        "{text}"
    );
    assert!(
        report
            .causes
            .iter()
            .any(|c| c.title.contains("maxSpeed is 0")),
        "{text}"
    );
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.name == "runtime" && c.status == Status::Unknown),
        "{text}"
    );
}

#[test]
fn overlays_capture_paths_cones_sight_checks_and_eqs_points() {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, 40.0], [0.0, 40.0], 0.0, 0);
    for (x, z) in [(18.0, 10.0), (18.0, 28.0), (12.0, 15.0), (12.0, 23.0)] {
        solid(&mut g, [x, x + 2.0], [z, z + 2.0]);
    }
    let mut sim = Sim::new(bake(g, [40.0, 40.0]));
    sim.load("soldier", "soldier");
    let soldier = sim.npc(
        1,
        "soldier",
        [6.0, 0.1, 20.0],
        Perceiver {
            sight_range: 40.0,
            ..Default::default()
        },
        &[],
    );
    sim.world
        .get_mut::<ara_perception::PerceptionPose>(soldier)
        .unwrap()
        .forward = [1.0, 0.0, 0.0];
    sim.actor(99, "player", [34.0, 0.1, 20.0]);
    sim.step();
    let items = overlay::collect(&mut sim.world, true);
    let has = |f: &dyn Fn(&overlay::Overlay) -> bool| items.iter().any(f);
    assert!(has(&|o| matches!(o, overlay::Overlay::Cone { .. })));
    assert!(has(&|o| matches!(
        o,
        overlay::Overlay::Sight {
            outcome: ara_perception::SightOutcome::Seen,
            ..
        }
    )));
    assert!(has(&|o| matches!(
        o,
        overlay::Overlay::Eqs { best: true, .. }
    )));
    assert!(has(
        &|o| matches!(o, overlay::Overlay::Agent { label, .. } if label == "Peek")
    ));
    for _ in 0..30 {
        sim.step();
    }
    let items = overlay::collect(&mut sim.world, true);
    assert!(
        items
            .iter()
            .any(|o| matches!(o, overlay::Overlay::Path { .. }))
    );
    let svg = overlay::svg(&items, 640);
    assert!(svg.starts_with("<svg") && svg.contains("<polyline") && svg.contains("<path"));
    let view = view(&sim.world, soldier).unwrap();
    assert_eq!(view.path, ["Fight", "Engage", "StepOut"]);
    assert!(
        serde_json::to_string(&view)
            .unwrap()
            .contains("\"behavior\":\"Soldier\"")
    );
    if std::env::var("ARA_WRITE_CAPTURES").is_ok_and(|v| v == "1") {
        let dir = format!("{}/captures", env!("CARGO_MANIFEST_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(format!("{dir}/cover-fight.svg"), svg).unwrap();
    }
}
