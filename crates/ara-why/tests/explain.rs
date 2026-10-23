//! Every built-in explainer on in-memory fixtures: a small scene, assets, a bus
//! store and a profile summary.

use std::collections::BTreeMap;

use ara_diag::bus::Meta;
use ara_diag::store::{Filter, Record, Store};
use ara_diag::{Code, Diagnostic, FixAction, Subject, codes};
use ara_why::{Asset, Entity, Facts, Likelihood, Report, Status, Why};
use serde_json::{Value, json};

const DOC: &str = "levels/test.scene";

/// A scene, some assets and files, a bus store and an optional profile.
#[derive(Default)]
struct Fixture {
    entities: BTreeMap<String, Entity>,
    assets: BTreeMap<String, Asset>,
    files: BTreeMap<String, String>,
    bus: Store,
    profile: Option<Value>,
}

impl Fixture {
    /// The base scene: a camera and a crate stack, all healthy.
    fn scene() -> Fixture {
        let mut f = Fixture::default();
        f.add(
            "e_cam01",
            "Main Camera",
            None,
            json!({
                "Transform": { "position": [0, 2, 10], "rotation": [0, 0, 0], "scale": [1, 1, 1] },
                "Camera": { "cullingMask": ["default", "ui"], "far": 100 },
            }),
        );
        f.add(
            "e_stack1",
            "Stack",
            None,
            json!({
                "Transform": { "position": [0, 0, 0], "rotation": [0, 0, 0], "scale": [1, 1, 1] },
            }),
        );
        f.add(
            "e_crate1",
            "Crate",
            Some("e_stack1"),
            json!({
                "Transform": { "position": [0, 1, 0], "rotation": [0, 0, 0], "scale": [1, 1, 1] },
                "MeshRenderer": { "mesh": "@meshes/crate.glb#ms_crate001", "tint": [1, 1, 1, 1] },
            }),
        );
        f.add_asset(
            "@meshes/crate.glb#ms_crate001",
            "ms_crate001",
            "meshes/crate.glb",
            None,
            Some(true),
        );
        f.add_asset(
            "@meshes/lid.glb#ms_lid0001",
            "ms_lid0001",
            "meshes/lid.glb",
            None,
            Some(true),
        );
        f
    }

    fn add(&mut self, address: &str, name: &str, parent: Option<&str>, components: Value) {
        let components = components
            .as_object()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        self.entities.insert(
            address.into(),
            Entity {
                doc: DOC.into(),
                address: address.into(),
                name: Some(name.into()),
                parent: parent.map(str::to_owned),
                components,
                flags: Default::default(),
            },
        );
    }

    fn set(&mut self, address: &str, component: &str, field: &str, value: Value) {
        let e = self.entities.get_mut(address).expect("entity");
        let c = e
            .components
            .entry(component.into())
            .or_insert_with(|| json!({}));
        c[field] = value;
    }

    fn add_asset(
        &mut self,
        reference: &str,
        id: &str,
        path: &str,
        kind: Option<&str>,
        source: Option<bool>,
    ) {
        let a = Asset {
            reference: reference.into(),
            id: Some(id.into()),
            path: Some(path.into()),
            kind: kind.map(str::to_owned),
            source_exists: source,
            problem: None,
            closest: Vec::new(),
        };
        for key in [
            reference.to_owned(),
            format!("@{path}"),
            path.to_owned(),
            id.to_owned(),
        ] {
            self.assets.insert(key, a.clone());
        }
    }

    fn why(&self, question: &str) -> Report {
        Why::builtin()
            .explain(question, self)
            .unwrap_or_else(|d| panic!("{question}: {d:?}"))
    }
}

impl Facts for Fixture {
    fn entity(&self, selector: &str, _doc: Option<&str>) -> Result<Entity, Vec<Diagnostic>> {
        let found = match selector.strip_prefix("name:") {
            Some(name) => self
                .entities
                .values()
                .find(|e| e.name.as_deref() == Some(name)),
            None => self.entities.get(selector),
        };
        found.cloned().ok_or_else(|| {
            vec![Diagnostic::error(
                codes::RECORD_NOT_FOUND,
                format!("no entity `{selector}`"),
            )]
        })
    }

    fn parent(&self, e: &Entity) -> Option<Entity> {
        self.entities.get(e.parent.as_deref()?).cloned()
    }

    fn with_component(&self, _doc: &str, component: &str) -> Vec<Entity> {
        self.entities
            .values()
            .filter(|e| e.components.contains_key(component))
            .cloned()
            .collect()
    }

    fn has_component_type(&self, name: &str) -> bool {
        matches!(name, "Camera" | "MeshRenderer" | "Transform")
    }

    fn asset(&self, reference: &str) -> Asset {
        self.assets
            .get(reference)
            .cloned()
            .unwrap_or_else(|| Asset {
                reference: reference.into(),
                problem: Some(Diagnostic::error(
                    codes::UNRESOLVED_REF,
                    format!("`{reference}` does not resolve"),
                )),
                closest: vec!["@meshes/crate.glb#ms_crate001".into()],
                ..Asset::default()
            })
    }

    fn read(&self, path: &str) -> Option<String> {
        self.files.get(path).cloned()
    }

    fn users(&self, id: &str, _limit: usize) -> Vec<(String, String, String)> {
        self.entities
            .values()
            .filter(|e| {
                e.get("MeshRenderer", "mesh")
                    .and_then(Value::as_str)
                    .is_some_and(|m| m.contains(id))
            })
            .map(|e| (e.doc.clone(), e.address.clone(), "MeshRenderer.mesh".into()))
            .collect()
    }

    fn diagnostics(&self, filter: &Filter) -> Vec<Record> {
        self.bus.query(filter).into_iter().cloned().collect()
    }

    fn causes(&self, record: &Record) -> Vec<Record> {
        self.bus.causes(record).into_iter().cloned().collect()
    }

    fn profile(&self) -> Option<Value> {
        self.profile.clone()
    }
}

fn top(r: &Report) -> (Code, Likelihood) {
    let c = r
        .causes
        .first()
        .unwrap_or_else(|| panic!("no cause:\n{}", r.text()));
    (c.code, c.likelihood)
}

fn command(r: &Report) -> Value {
    match &r.causes[0].fixes[0].action {
        FixAction::Command { command, params } => json!({ "cmd": command, "args": params }),
        other => panic!("not a command: {other:?}"),
    }
}

#[test]
fn a_healthy_entity_has_no_causes_and_every_check_passes() {
    let r = Fixture::scene().why("why is name:Crate not visible");
    assert_eq!(r.explainer, "visibility");
    assert!(r.causes.is_empty(), "{}", r.text());
    assert!(
        r.checks.iter().all(|c| c.status != Status::Fail),
        "{}",
        r.text()
    );
    assert!(
        r.text().contains("passes ? (needs the running renderer)"),
        "{}",
        r.text()
    );
}

#[test]
fn an_inherited_zero_scale_points_at_the_parent() {
    let mut f = Fixture::scene();
    f.set("e_stack1", "Transform", "scale", json!([1, 0, 1]));
    let r = f.why("why is name:Crate not visible");
    assert_eq!(
        top(&r),
        (codes::RENDER_DEGENERATE_TRANSFORM, Likelihood::High)
    );
    assert_eq!(
        command(&r),
        json!({ "cmd": "property.set", "args": { "entity": "e_stack1", "path": "Transform.scale", "value": [1, 1, 1] } })
    );
    assert!(
        r.text()
            .contains("Its ancestor `Stack` (e_stack1)'s `Transform.scale` is 0 on y"),
        "{}",
        r.text()
    );
}

#[test]
fn a_layer_no_camera_renders_is_culled_with_two_fixes() {
    let mut f = Fixture::scene();
    f.set("e_crate1", "MeshRenderer", "layer", json!("debug"));
    let r = f.why("e_crate1 invisible");
    assert_eq!(top(&r), (codes::RENDER_LAYER_CULLED, Likelihood::High));
    let fixes: Vec<&str> = r.causes[0].fixes.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(
        fixes,
        [
            "Move it to layer \"default\"",
            "Add `debug` to the culling mask of `Main Camera` (e_cam01)"
        ]
    );
}

#[test]
fn a_material_that_does_not_compile_is_located() {
    let mut f = Fixture::scene();
    f.set(
        "e_crate1",
        "MeshRenderer",
        "material",
        json!("@materials/crate.material"),
    );
    f.add_asset(
        "@materials/crate.material",
        "gr_crate001",
        "materials/crate.material",
        Some("material"),
        None,
    );
    f.files.insert(
        "materials/crate.material".into(),
        "material Crate : surface budget 1 {\n  param tint : ColorLinear = linear(1, 1, 1);\n  output baseColor = tnit;\n}\n".into(),
    );
    let r = f.why("why can't I see name:Crate");
    assert_eq!(top(&r), (codes::RENDER_MATERIAL, Likelihood::High));
    assert!(
        r.causes[0].evidence[0].contains("materials/crate.material"),
        "{}",
        r.text()
    );

    // An opacity of 0 compiles but draws nothing.
    f.files.insert(
        "materials/crate.material".into(),
        "material Crate : unlit budget 1 {\n  param tint : ColorLinear = linear(1, 1, 1);\n  let alpha : float = 0.0;\n  output color = tint;\n  output opacity = alpha;\n}\n".into(),
    );
    let r = f.why("why can't I see name:Crate");
    assert_eq!(
        top(&r),
        (codes::RENDER_TRANSPARENT, Likelihood::High),
        "{}",
        r.text()
    );
}

#[test]
fn a_missing_mesh_offers_the_closest_asset() {
    let mut f = Fixture::scene();
    f.set(
        "e_crate1",
        "MeshRenderer",
        "mesh",
        json!("@meshes/crat.glb#ms_crate002"),
    );
    let r = f.why("name:Crate not showing");
    assert_eq!(top(&r), (codes::RENDER_NO_MESH, Likelihood::High));
    assert_eq!(
        command(&r)["args"]["value"],
        "@meshes/crate.glb#ms_crate001"
    );
}

#[test]
fn a_nan_reported_at_runtime_comes_with_its_cause() {
    let mut f = Fixture::scene();
    let physics = f
        .bus
        .push(
            Diagnostic::error(
                Code(7001),
                "rigid body `Crate` has zero mass; its integration divided by zero",
            )
            .subject(Subject::entity("e_crate1")),
            Meta {
                tick: Some(1200),
                ..Meta::default()
            },
        )
        .id;
    f.bus.push(
        Diagnostic::error(
            codes::RENDER_NON_FINITE_TRANSFORM,
            "`Crate` has a NaN position",
        )
        .subject(Subject::entity("e_crate1")),
        Meta {
            cause: Some(physics),
            tick: Some(1201),
            ..Meta::default()
        },
    );
    let r = f.why("why is name:Crate not visible");
    assert_eq!(
        top(&r),
        (codes::RENDER_NON_FINITE_TRANSFORM, Likelihood::High)
    );
    let text = r.text();
    assert!(
        text.contains("reported a non-finite transform at tick 1201"),
        "{text}"
    );
    assert!(
        text.contains("caused by: error ARA7001 rigid body `Crate` has zero mass"),
        "{text}"
    );
}

#[test]
fn disabled_ancestors_and_far_planes_are_found() {
    let mut f = Fixture::scene();
    f.entities
        .get_mut("e_stack1")
        .expect("stack")
        .flags
        .insert("enabled".into(), json!(false));
    let r = f.why("name:Crate not visible");
    assert_eq!(top(&r), (codes::RENDER_DISABLED, Likelihood::High));

    let mut f = Fixture::scene();
    f.set("e_crate1", "Transform", "position", json!([0, 1, -500]));
    let r = f.why("name:Crate not visible");
    assert_eq!(top(&r), (codes::RENDER_DISTANCE_CULLED, Likelihood::High));
    assert_eq!(command(&r)["args"]["path"], "Camera.far");
}

#[test]
fn reload_escalations_are_explained_newest_reason_first() {
    let mut f = Fixture::scene();
    for (reason, msg) in [
        ("cold", "first edit of a cold module"),
        ("closure", "Door.cs: OnTick has a lambda"),
    ] {
        f.bus.push(
            Diagnostic::info(
                codes::RELOAD_ESCALATED,
                format!("`Gameplay` reloaded at L3 instead of L1: {msg} ({reason})"),
            )
            .subject(Subject::module("Gameplay"))
            .data(json!({ "module": "Gameplay", "level": "L3", "reason": reason })),
            Meta::default(),
        );
    }
    let r = f.why("why did reload fall back to L3");
    assert_eq!(r.explainer, "reload");
    assert_eq!(top(&r), (codes::RELOAD_ESCALATED, Likelihood::High));
    assert!(r.causes[0].title.contains("lambda"), "{}", r.text());
    assert!(r.causes[0].advice[0].contains("named method"));
    assert_eq!(r.causes[1].likelihood, Likelihood::Medium);

    let none = Fixture::scene().why("why did reload fall back to L3");
    assert!(none.answer.starts_with("no C# reload has been reported"));
}

#[test]
fn slow_frames_name_the_top_scope_or_ask_for_a_capture() {
    let mut f = Fixture::scene();
    let r = f.why("why is the frame slow");
    assert_eq!(r.explainer, "frame");
    assert!(
        matches!(r.causes[0].fixes[0].action, FixAction::Call { ref method, .. } if method == "profile.start")
    );

    f.profile = Some(json!({
        "version": 1, "frames": 120, "budget_ns": 16_666_667,
        "cpu_frame": { "p50": 15_000_000, "p95": 24_000_000, "p99": 31_000_000 },
        "metrics": [
            { "key": "cpu/physics.step", "total_ns": 1, "time": { "p50": 1, "p95": 9_000_000, "p99": 1 }, "inclusive": { "p50": 1, "p95": 1, "p99": 1 }, "allocated_bytes": 0 },
            { "key": "gpu/graphics/shadows", "total_ns": 1, "time": { "p50": 1, "p95": 4_000_000, "p99": 1 }, "inclusive": { "p50": 1, "p95": 1, "p99": 1 }, "allocated_bytes": 0 },
        ],
        "hitches": [{ "frame": 77, "duration_ns": 41_000_000, "budget_ns": 16_666_667, "contributors": [["cpu/script.reload", 30_000_000]], "hints": ["a C# module swapped at this tick boundary"] }],
        "hitch_count": 1, "dropped": 0, "truncated": false, "metrics_truncated": false,
    }));
    let r = f.why("why is the frame slow");
    assert_eq!(top(&r), (codes::FRAME_OVER_BUDGET, Likelihood::High));
    assert!(
        r.causes[0]
            .title
            .starts_with("`cpu/physics.step` takes 9.00 ms at p95"),
        "{}",
        r.text()
    );
    assert!(
        r.text()
            .contains("contributors: cpu/script.reload 30.00 ms"),
        "{}",
        r.text()
    );
}

#[test]
fn codes_explain_their_instances_with_fixes() {
    let mut f = Fixture::scene();
    f.bus.push(
        Diagnostic::error(
            codes::UNRESOLVED_REF,
            "`@meshes/genrator.glb#ms_genr0002` does not resolve",
        )
        .file("levels/test.scene")
        .record("e_gen001")
        .path("MeshRenderer.mesh")
        .with_fix(
            "Use `@meshes/generator.glb#ms_genr0001`",
            FixAction::Set {
                record: Some("e_gen001".into()),
                path: "MeshRenderer.mesh".into(),
                value: json!("@meshes/generator.glb#ms_genr0001"),
            },
        ),
        Meta::default(),
    );
    let r = f.why("ARA1302");
    assert_eq!(r.explainer, "code");
    assert_eq!(top(&r), (codes::UNRESOLVED_REF, Likelihood::High));
    assert_eq!(
        command(&r)["args"]["entity"],
        "e_gen001",
        "file fixes become runnable commands"
    );
    assert!(
        r.doc
            .as_deref()
            .is_some_and(|d| d.contains("ara://docs/errors/ARA1302"))
    );

    let quiet = Fixture::scene().why("why ARA1302");
    assert!(quiet.causes.iter().all(|c| c.likelihood == Likelihood::Low));
}

#[test]
fn assets_report_compile_errors_and_users() {
    let mut f = Fixture::scene();
    f.add_asset(
        "@materials/crate.material",
        "gr_crate001",
        "materials/crate.material",
        Some("material"),
        None,
    );
    f.files.insert(
        "materials/crate.material".into(),
        "material Crate : surface budget 1 {\n  output baseColor = 1.0;\n}\n".into(),
    );
    let r = f.why("@materials/crate.material");
    assert_eq!(r.explainer, "asset");
    assert_eq!(r.causes[0].likelihood, Likelihood::High, "{}", r.text());
    assert!(
        r.checks
            .iter()
            .any(|c| c.name == "compiles" && c.status == Status::Fail)
    );

    let r = f.why("@meshes/crate.glb");
    assert!(r.causes.is_empty(), "{}", r.text());
    assert_eq!(
        r.related,
        ["levels/test.scene · e_crate1 · MeshRenderer.mesh"]
    );
}

#[test]
fn unclear_questions_say_what_can_be_asked() {
    let d = Why::builtin()
        .explain("why", &Fixture::scene())
        .expect_err("unanswerable");
    assert_eq!(d[0].code, codes::WHY_UNANSWERED);
    let r = Fixture::scene().why("why is name:Crate not opening");
    assert_eq!(r.explainer, "subject");
    assert!(
        r.answer.contains("no explainer covers `opening` yet"),
        "{}",
        r.answer
    );
}
