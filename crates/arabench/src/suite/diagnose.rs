//! Diagnosis: something in the outpost does not show up, and the agent has to find
//! out why with the tools before fixing it. Each task starts from a broken scene
//! the prompt only describes by its symptom: a material that does not compile, a
//! mesh lost in a re-export, a layer the camera does not draw, and a barrel the
//! play session reported as NaN (a runtime report in `.ara/diagnostics.jsonl`).
//!
//! Every task adds the game camera first, so "is there a camera" is never the
//! answer.

use std::fs;
use std::path::Path;

use ara_host::{Host, HostConfig, Project};
use ara_proto::ConnId;
use serde_json::{Value, json};

use super::{OUTPOST, at, clean, with_why};
use crate::fixture::{authored_path, patch};
use crate::solve::Solver;
use crate::task::{Area, Budget, Checks, Fixture, Task};
use crate::world::World;

const RADIO_MATERIAL: &str = "materials/radio.material";
const BROKEN_RADIO: &str = "material Radio : surface budget 1 {\n  param paint : ColorLinear = linear(0.1, 0.35, 0.12);\n  param roughness : float = 0.6;\n  output baseColor = pain;\n  output roughness = roughness;\n}\n";
const FIXED_RADIO: &str = "material Radio : surface budget 1 {\n  param paint : ColorLinear = linear(0.1, 0.35, 0.12);\n  param roughness : float = 0.6;\n  output baseColor = paint;\n  output roughness = roughness;\n}\n";

fn write(root: &Path, rel: &str, text: &str) -> Result<(), String> {
    let path = authored_path(root, rel);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&path, text).map_err(|e| format!("cannot write `{rel}`: {e}"))
}

/// The game camera, south of the outpost looking north (−Z), drawing two layers.
fn add_camera(root: &Path) -> Result<(), String> {
    patch(
        root,
        OUTPOST,
        "\"records\": {",
        "\"records\": {\n    \"e_cam001\": {\n      \"name\": \"MainCamera\",\n      \"order\": \"aY\",\n      \"Camera\": {\n        \"cullingMask\": [\"default\", \"props\"],\n        \"far\": 200\n      },\n      \"Transform\": {\n        \"position\": [0, 4, 24]\n      }\n    },",
    )
}

/// Whether a material source parses and checks.
fn compiles(text: &str, id: &str) -> Result<(), String> {
    let m = ara_material::dsl::parse(text, id).map_err(|d| format!("{d:?}"))?;
    ara_material::check(&m)
        .map(|_| ())
        .map_err(|d| format!("{d:?}"))
}

/// The radio draws with a material whose output names a parameter that does not
/// exist (`pain` for `paint`).
pub const BAD_MATERIAL: Task = Task {
    id: "why-bad-material",
    area: Area::Diagnose,
    prompt: "The radio on the shelf in the outpost level stopped showing up in game after someone edited its material. Find out why and fix it.",
    fixture: Fixture::Outpost,
    setup: Some(|root: &Path| {
        add_camera(root)?;
        write(root, RADIO_MATERIAL, BROKEN_RADIO)?;
        write(
            root,
            "materials/radio.material.import.json",
            "{\n  \"id\": \"gr_radio001\"\n}\n",
        )?;
        patch(
            root,
            OUTPOST,
            "\"mesh\": \"@meshes/radio.glb#ms_radio001\"",
            "\"material\": \"@materials/radio.material#gr_radio001\",\n        \"mesh\": \"@meshes/radio.glb#ms_radio001\"",
        )
    }),
    budget: Budget::new(20, 6),
    verify: |w: &World, c: &mut Checks| {
        clean(c, w);
        if let Some(e) = at(c, w, OUTPOST, "e_radio1") {
            let material = e.get("MeshRenderer.material");
            c.check(
                "the radio still draws with its material",
                material
                    .as_ref()
                    .and_then(Value::as_str)
                    .is_some_and(|m| m.contains("gr_radio001")),
                format!("{material:?}"),
            );
        }
        let text = w.authored_file(RADIO_MATERIAL).unwrap_or_default();
        let result = compiles(&text, "gr_radio001");
        c.check(
            "the radio material compiles",
            result.is_ok(),
            result.err().unwrap_or_default(),
        );
        c.check(
            "the radio material keeps its green paint",
            text.contains("linear(0.1, 0.35, 0.12)"),
            "the paint parameter changed",
        );
    },
    solve: |s: &mut Solver| {
        let why = s.call(
            "why",
            json!({ "question": "name:Radio not visible", "full": true }),
        )?;
        if why["causes"][0]["code"] != "ARA3306" {
            return Err(format!("unexpected answer:\n{}", why["text"]));
        }
        s.write_authored(RADIO_MATERIAL, FIXED_RADIO)
    },
};

/// The generator's mesh was re-exported under a new name and ID, so its
/// reference resolves to nothing.
pub const MISSING_MESH: Task = Task {
    id: "why-missing-mesh",
    area: Area::Diagnose,
    prompt: "The generator in the outpost level has been invisible since the art team re-exported its mesh. Find out why and fix it.",
    fixture: Fixture::Outpost,
    setup: Some(|root: &Path| {
        add_camera(root)?;
        fs::remove_file(authored_path(root, "meshes/generator.glb.import.json"))
            .map_err(|e| e.to_string())?;
        write(
            root,
            "meshes/generator_v2.glb.import.json",
            "{\n  \"id\": \"ms_genr0002\"\n}\n",
        )
    }),
    budget: Budget::new(20, 6),
    verify: |w: &World, c: &mut Checks| {
        clean(c, w);
        if let Some(e) = at(c, w, OUTPOST, "e_gen001") {
            let mesh = e.get("MeshRenderer.mesh");
            c.check(
                "the generator draws the re-exported mesh",
                mesh.as_ref()
                    .and_then(Value::as_str)
                    .is_some_and(|m| m.contains("ms_genr0002")),
                format!("{mesh:?}"),
            );
        }
        with_why(c, w, OUTPOST, &["e_gen001"]);
    },
    solve: |s: &mut Solver| {
        let why = s.call(
            "why",
            json!({ "question": "name:Generator not visible", "full": true }),
        )?;
        let fix = &why["causes"][0]["fixes"][0];
        if fix["command"] != "property.set" {
            return Err(format!("no runnable fix:\n{}", why["text"]));
        }
        s.apply(
            OUTPOST,
            "Point the generator at its re-exported mesh",
            json!([{ "cmd": fix["command"], "args": fix["params"] }]),
        )
        .map(|_| ())
    },
};

/// The workbench sits on a layer the game camera does not draw.
pub const CULLING_MASK: Task = Task {
    id: "why-culling-mask",
    area: Area::Diagnose,
    prompt: "The workbench doesn't render in game anymore, but it's still in the outpost level. Find out why and fix it. Don't change the camera.",
    fixture: Fixture::Outpost,
    setup: Some(|root: &Path| {
        add_camera(root)?;
        patch(
            root,
            OUTPOST,
            "\"mesh\": \"@meshes/bench.glb#ms_bench001\"",
            "\"layer\": \"editor\",\n        \"mesh\": \"@meshes/bench.glb#ms_bench001\"",
        )
    }),
    budget: Budget::new(20, 6),
    verify: |w: &World, c: &mut Checks| {
        clean(c, w);
        if let Some(e) = at(c, w, OUTPOST, "e_bench1") {
            let layer = e.get("MeshRenderer.layer");
            c.check(
                "the workbench is on a layer the camera draws",
                matches!(
                    layer.as_ref().and_then(Value::as_str),
                    Some("default" | "props")
                ),
                format!("{layer:?}"),
            );
        }
        if let Some(cam) = at(c, w, OUTPOST, "e_cam001") {
            let mask = cam.get("Camera.cullingMask");
            c.check(
                "the camera is unchanged",
                mask == Some(json!(["default", "props"])) && cam.num("Camera.far") == Some(200.0),
                format!("{mask:?}"),
            );
        }
        with_why(c, w, OUTPOST, &["e_bench1"]);
    },
    solve: |s: &mut Solver| {
        let why = s.call(
            "why",
            json!({ "question": "name:Workbench not visible", "full": true }),
        )?;
        let cause = &why["causes"][0];
        if cause["code"] != "ARA3308" {
            return Err(format!("unexpected answer:\n{}", why["text"]));
        }
        let fix = &cause["fixes"][0];
        s.apply(
            OUTPOST,
            "Put the workbench back on a layer the camera draws",
            json!([{ "cmd": fix["command"], "args": fix["params"] }]),
        )
        .map(|_| ())
    },
};

/// A barrel with zero mass blew up in the last play session: physics divided by
/// zero and its position went NaN. The play process's report is on the bus.
pub const NAN_BARREL: Task = Task {
    id: "why-nan-barrel",
    area: Area::Diagnose,
    prompt: "In the last playtest one of the barrels in the outpost vanished a second after the game started. Find out why and fix it so it stops happening.",
    fixture: Fixture::Outpost,
    setup: Some(|root: &Path| {
        add_camera(root)?;
        let barrel = "\"name\": \"Barrel.002\",";
        let text = fs::read_to_string(authored_path(root, OUTPOST)).map_err(|e| e.to_string())?;
        let at = text.find(barrel).ok_or("no Barrel.002")?;
        let mass = text[at..].find("\"mass\": 30").ok_or("no barrel mass")? + at;
        let text = format!(
            "{}\"mass\": 0{}",
            &text[..mass],
            &text[mass + "\"mass\": 30".len()..]
        );
        fs::write(authored_path(root, OUTPOST), text).map_err(|e| e.to_string())?;
        report_playtest(root)
    }),
    budget: Budget::new(20, 6),
    verify: |w: &World, c: &mut Checks| {
        clean(c, w);
        if let Some(e) = at(c, w, OUTPOST, "e_brl003") {
            let mass = e.num("RigidBody.mass");
            c.check(
                "Barrel.002 has a mass again",
                mass.is_some_and(|m| m > 0.0),
                format!("{mass:?}"),
            );
        }
        for other in ["e_brl001", "e_brl002"] {
            if let Some(e) = at(c, w, OUTPOST, other) {
                let mass = e.num("RigidBody.mass");
                c.check(
                    format!("{other} keeps its mass"),
                    mass == Some(30.0),
                    format!("{mass:?}"),
                );
            }
        }
        with_why(c, w, OUTPOST, &["e_brl003"]);
    },
    solve: |s: &mut Solver| {
        let logs = s.call("logs", json!({ "severity": "error" }))?;
        if !logs["text"]
            .as_str()
            .is_some_and(|t| t.contains("entity:e_brl003"))
        {
            return Err(format!("the playtest report is missing:\n{}", logs["text"]));
        }
        let why = s.call("why", json!({ "question": "e_brl003 not visible" }))?;
        if !why["text"]
            .as_str()
            .is_some_and(|t| t.contains("zero mass"))
        {
            return Err(format!("the cause is missing:\n{}", why["text"]));
        }
        s.apply(
            OUTPOST,
            "A zero mass made physics divide by zero; give the barrel its mass back",
            json!([{ "cmd": "property.set", "args": { "entity": "e_brl003", "path": "RigidBody.mass", "value": 30 } }]),
        )
        .map(|_| ())
    },
};

/// What the play process reported during the playtest, as the host keeps it.
fn report_playtest(root: &Path) -> Result<(), String> {
    let project = Project::open(root).map_err(|d| d.to_string())?;
    let mut host = Host::open(project, HostConfig::default()).map_err(|d| d.to_string())?;
    let call = |host: &mut Host, method: &str, params: Value| {
        host.call(ConnId::LOCAL, method, params)
            .map_err(|e| e.to_string())
    };
    let begun = call(
        &mut host,
        "session.begin",
        json!({ "client": "ara-player", "author": "human" }),
    )?;
    let session = begun["session"].clone();
    call(
        &mut host,
        "diag.report",
        json!({
            "session": session,
            "engine": true,
            "tick": 61,
            "diagnostics": [
                {
                    "code": "ARA7001",
                    "severity": "error",
                    "message": "rigid body of `Barrel.002` has zero mass; integrating its forces divided by zero",
                    "subjects": ["entity:e_brl003", "system:physics.step"],
                },
                {
                    "code": "ARA3304",
                    "severity": "error",
                    "message": "`Barrel.002` position became NaN; it is no longer drawn",
                    "subjects": ["entity:e_brl003"],
                    "causedBy": 0,
                },
            ],
        }),
    )?;
    call(&mut host, "session.end", json!({ "session": session }))?;
    drop(host);
    Ok(())
}
