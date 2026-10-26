//! Outpost's game layer, authored through the same typed commands as an agent.
use crate::{build::{Ctx, anim, place::create}, play::Mcp};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const SCENE: &str = "levels/outpost.scene";
const ROBOT: &str = "assets/third_party/outpost_robot/RobotExpressive.glb";
const SPACE: &str = "assets/third_party/outpost_space/Models/GLTF format";

/// Publishes C# types through headless Play, then augments the real compound in place.
pub(crate) fn author(ctx: &Ctx) -> Result<()> {
    std::fs::create_dir_all(ctx.root.join(".scratch"))?;
    {
        let mut mcp = Mcp::start(&ctx.root)?;
        let started = mcp.play("start", json!({"scene":SCENE,"startPaused":true,"window":false}))?;
        let deadline = Instant::now() + Duration::from_secs(90);
        let types = ctx.root.join("scripts/Salvage/Salvage.types.json");
        while !types.is_file() {
            anyhow::ensure!(Instant::now() < deadline, "C# types were not published");
            mcp.play("status", json!({}))?;
            std::thread::sleep(Duration::from_millis(20));
        }
        mcp.play("stop", json!({"world":started["world"]}))?;
    }
    ctx.sidecar(ROBOT, "as_outpostrobot", json!({}))?;
    let drone = format!("{SPACE}/craft_speederA.glb");
    ctx.sidecar(&drone, "as_outpostdrone", json!({}))?;
    let toolbox = "assets/third_party/metal_toolbox/metal_toolbox_1k.gltf";
    ctx.sidecar(toolbox, "as_metaltoolbox", json!({}))?;
    let animation = "anim/scavenger.animation";
    let existing: Value = serde_json::from_slice(&std::fs::read(ctx.root.join(SCENE))?)?;
    if ctx.root.join(animation).is_file() {
        ctx.apply("ara.project.json", "Reauthor the scavenger animation controller", vec![
            ("doc.delete",json!({"document":"@anim/scavenger.animation#ac_scavenger"}))])?;
    }
    anim::locomotion(ctx, animation, "ac_scavenger", "an_scavenger", &ctx.root.join(ROBOT), "Idle", &[
        anim::Gait {clip:"Walking",speed:2.4}, anim::Gait {clip:"Running",speed:5.0},
    ])?;
    let mut calls = Vec::new();
    if existing["records"]["e_player"].get("Survivor").is_some() {
        calls.push(("component.remove",json!({"entity":"e_player","component":"Survivor"})));
    }
    for id in existing["records"].as_object().context("scene records")?.keys().filter(|id| id.starts_with("e_game") || *id == "e_run") {
        calls.push(("entity.delete", json!({"entity":id})));
    }
    calls.push(("property.set",json!({"entity":"e_player","path":"MeshRenderer","value":{"model":format!("@{ROBOT}#as_outpostrobot")}})));
    calls.push(("property.set",json!({"entity":"e_player","path":"Transform","value":{"position":[96,0.05,85],"rotation":[0,0,0],"scale":[0.45,0.45,0.45]}})));
    calls.push(("property.set",json!({"entity":"e_player","path":"PlayerControl","value":{"walkSpeed":2.4,"runSpeed":5.0,"yaw":0,"camera":"e_camera"}})));
    calls.push(("property.set",json!({"entity":"e_camera","path":"FollowCamera","value":{"target":"e_player","distance":4.8,"height":1.6,"yaw":0,"pitch":15}})));
    calls.push(("property.set",json!({"entity":"e_player","path":"Animator.controller","value":"@anim/scavenger.animation#an_scavenger"})));
    calls.push(("component.add",json!({"entity":"e_player","component":"Survivor","value":{"health":100}})));
    calls.push(create("e_run","Outpost salvage run",json!({"SalvageRun":{"goal":8,"ticksLeft":18000,"rescues":3}})));
    calls.push(create("e_gameextract","Blue extraction beacon",json!({
        "Transform":{"position":[96,0.1,85]},"ExtractionBeacon":{"radius":4},
        "MeshRenderer":{"shape":"plane","size":[6,0.08,6],"color":[0.1,0.6,0.8,1],"emissive":[0.05,0.4,0.7]},
        "Light":{"color":[0.15,0.7,1,1],"intensity":400,"range":8}
    })));
    // The route has a close first cache, two shared doors, then branches into the hangar.
    for (index, position) in [[94.,0.6,91.],[99.,0.6,94.],[102.,0.6,99.],[98.,0.6,106.],
        [92.,0.6,108.],[101.,0.6,113.],[90.,0.6,119.],[100.,0.6,124.],
        [81.,0.6,94.],[111.,0.6,92.],[110.,0.6,111.],[80.,0.6,119.]].into_iter().enumerate() {
        calls.push(create(&format!("e_gamecache{index:02}"), &format!("Signal cache {}",index+1), json!({
            "Transform":{"position":position,"scale":[1.2,1.2,1.2]},
            "MeshRenderer":{"model":format!("@{toolbox}#as_metaltoolbox"),"emissive":[0.12,0.24,0.06]},
            "SalvageCache":{"kind":index%3},
            "Light":{"color":[0.6,1,0.4,1],"intensity":65,"range":3}
        })));
    }
    // Art doors are selected from the existing pinned industrial shutter prefab.
    for (index, position) in [[98.0,0.0,103.8],[104.0,0.0,93.0]].into_iter().enumerate() {
        calls.push(create(&format!("e_gamedoor{index}"),"Shared salvage hatch",json!({
            "Transform":{"position":position},"SharedDoor":{},
            "MeshRenderer":{"model":"@assets/third_party/rollershutter_door/rollershutter_door_1k.gltf#as_rollershutterdoor","node":"rollershutter_door_graffiti"},
            "RigidBody":{"motion":"kinematic"},"Collider":{"center":[1.0,1.3,0],"halfExtents":[1,1.3,0.12]}
        })));
    }
    for (index, position) in [[104.,0.1,116.],[82.,0.1,111.]].into_iter().enumerate() {
        calls.push(create(&format!("e_gamedrone{index}"),"Security drone",json!({
            "Transform":{"position":position,"scale":[0.6,0.6,0.6]},
            "MeshRenderer":{"model":format!("@{drone}#as_outpostdrone")},
            "NavAgent":{"maxSpeed":2.0,"radius":0.3,"height":1.0},
            "SecurityDrone":{"home":position}
        })));
    }
    ctx.apply(SCENE,"Turn the Outpost art scene into a co-op salvage run",calls)?;
    ctx.check()
}
