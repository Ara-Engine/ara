//! Workflows: schema discovery, undoing another agent's turn, docs lookup, one
//! batch with symbolic IDs, a dry run before a delete, and multi-step requests.

use std::path::Path;

use ara_doc::Op;
use ara_ops::Author;
use serde_json::{Value, json};

use super::prefabs::instances;
use super::{OUTPOST, at, num_is, one, parent_is, red, vec_is, with_why};
use crate::solve::Solver;
use crate::task::{Area, Budget, Checks, Fixture, Task};
use crate::world::World;

/// Field names the prompt only describes in words.
pub const DOOR_SCHEMA: Task = Task {
    id: "schema-door-fields",
    area: Area::Schema,
    prompt: "In the outpost level, make the gate door open by itself when someone comes within 3 m, and have it slide at half its current speed.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        let Some(d) = at(c, w, OUTPOST, "e_door01") else {
            return;
        };
        c.check(
            "the door opens by itself",
            d.get("Door.autoOpen") == Some(json!(true)),
            format!("{:?}", d.get("Door.autoOpen")),
        );
        num_is(c, "trigger radius is 3 m", d.num("Door.triggerRadius"), 3.0);
        num_is(c, "speed halved to 0.6 m/s", d.num("Door.speed"), 0.6);
        with_why(c, w, OUTPOST, &["e_door01"]);
    },
    solve: |s: &mut Solver| {
        s.call("schema.describe", json!({ "names": ["Door"] }))?;
        s.apply(
            OUTPOST,
            "Auto-open the gate door at 3 m and slow it down",
            json!([
                { "cmd": "property.set", "args": { "entity": "e_door01", "path": "Door.autoOpen", "value": true } },
                { "cmd": "property.set", "args": { "entity": "e_door01", "path": "Door.triggerRadius", "value": 3 } },
                { "cmd": "property.set", "args": { "entity": "e_door01", "path": "Door.speed", "value": 0.6 } },
            ]),
        )
        .map(drop)
    },
};

const BARRELS: [(&str, [f64; 3]); 3] = [
    ("e_brl001", [-3.0, 0.0, 2.0]),
    ("e_brl002", [-3.0, 0.0, 3.2]),
    ("e_brl003", [-3.0, 5.0, 4.4]),
];

/// Prior history: another agent moves the barrels, then the human brightens a light.
fn other_agent_moved_barrels(root: &Path) -> Result<(), String> {
    let mut helper = Solver::open(root, "agent:helper")?;
    helper.apply(
        OUTPOST,
        "Move the barrels next to the gate",
        json!([
            { "cmd": "transform.set", "args": { "entities": ["e_brl001"], "position": [12, 0, 4] } },
            { "cmd": "transform.set", "args": { "entities": ["e_brl002"], "position": [12, 0, 5] } },
            { "cmd": "transform.set", "args": { "entities": ["e_brl003"], "position": [12, 0, 6] } },
        ]),
    )?;
    helper.close();
    let mut human = Solver::open(root, "human")?;
    human.apply(
        OUTPOST,
        "Brighter work light",
        json!([{ "cmd": "property.set", "args": { "entity": "e_wlight1", "path": "Light.intensity", "value": 2000 } }]),
    )?;
    human.close();
    Ok(())
}

/// Reverting a specific earlier agent transaction, keeping a later human edit.
pub const UNDO_OTHER_AGENT: Task = Task {
    id: "undo-other-agent",
    area: Area::Undo,
    prompt: "Earlier, another agent moved the barrels in the outpost level over to the gate. That was a mistake: undo that change, but keep the work light change I made afterwards.",
    fixture: Fixture::Outpost,
    setup: Some(other_agent_moved_barrels),
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        for (addr, pos) in BARRELS {
            if let Some(e) = at(c, w, OUTPOST, addr) {
                vec_is(c, &format!("`{addr}` is back"), Some(e.position()), &pos);
            }
        }
        if let Some(e) = at(c, w, OUTPOST, "e_wlight1") {
            num_is(
                c,
                "the human's light change is kept",
                e.num("Light.intensity"),
                2000.0,
            );
        }
        let helper = Author::Agent("helper".into());
        let txn = w.history().iter().find(|t| t.author == helper);
        let reverted = txn.is_some_and(|t| w.reverted(t.id.0.as_str()));
        c.check(
            "the other agent's transaction is reverted",
            reverted,
            "still applied",
        );
    },
    solve: |s: &mut Solver| {
        let list = s.call("history.list", json!({ "author": "agent:helper" }))?;
        let txn = list["items"][0]["txn"]
            .as_str()
            .ok_or_else(|| format!("no helper transaction in {list}"))?
            .to_owned();
        s.call("review.revert", json!({ "txn": txn })).map(drop)
    },
};

/// Answers that live in the generated docs.
pub const DOCS_CHEATSHEET: Task = Task {
    id: "docs-cheatsheet",
    area: Area::Docs,
    prompt: "I'm writing a cheat sheet for new level designers. Using the engine's own documentation, create CHEATSHEET.md in the project root that answers: which axis points up, which unit `Transform.rotation` uses, which unit `Light.range` uses, and the exact name of the command that reverts a prefab override.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        let Some(text) = c.some("CHEATSHEET.md exists", w.file("CHEATSHEET.md")) else {
            return;
        };
        let t = text.to_lowercase();
        let any = |words: &[&str]| words.iter().any(|x| t.contains(x));
        c.check(
            "says Y is up",
            any(&[
                "y-up", "y up", "y is up", "+y", "y axis", "y-axis", "`y`", "**y**",
            ]),
            "",
        );
        c.check(
            "says rotations are in degrees",
            any(&["degree", "`deg`"]),
            "",
        );
        c.check(
            "says Light.range is in meters",
            any(&["meter", "metre", "`m`", "(m)"]),
            "",
        );
        c.check(
            "names `prefab.override.revert`",
            t.contains("prefab.override.revert"),
            "",
        );
    },
    solve: |s: &mut Solver| {
        s.call("docs.search", json!({ "query": "conventions axes units" }))?;
        s.call("docs.search", json!({ "query": "Light range" }))?;
        s.call("docs.search", json!({ "query": "revert prefab override" }))?;
        s.write(
            "CHEATSHEET.md",
            "# Cheat sheet\n\n- Up axis: Y (Y-up, right-handed).\n- `Transform.rotation`: XYZ Euler degrees.\n- `Light.range`: meters (`m`).\n- Revert a prefab override: `prefab.override.revert`.\n",
        )
    },
};

/// Four new records, nested, in one transaction through symbolic IDs.
pub const WATCHTOWER_BATCH: Task = Task {
    id: "batch-watchtower",
    area: Area::Batch,
    prompt: "Build a watchtower near the gate in the outpost level, as one single change: a root entity Watchtower at (10, 0, -6); under it a Platform 4 m up and a Ladder at (1, 2, 0); and a lamp from the lamp prefab standing on the Platform.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        let Some(tower) = one(c, w, OUTPOST, "Watchtower") else {
            return;
        };
        parent_is(c, &tower, None, "the scene root");
        vec_is(
            c,
            "Watchtower at (10, 0, -6)",
            Some(tower.position()),
            &[10.0, 0.0, -6.0],
        );
        let mut ids = vec![tower.addr];
        for (name, pos) in [("Platform", [0.0, 4.0, 0.0]), ("Ladder", [1.0, 2.0, 0.0])] {
            if let Some(e) = one(c, w, OUTPOST, name) {
                parent_is(c, &e, Some(tower.addr), "Watchtower");
                vec_is(
                    c,
                    &format!("{name} local position"),
                    Some(e.position()),
                    &pos,
                );
                ids.push(e.addr);
            }
        }
        let platform = w.named(OUTPOST, "Platform").first().map(|e| e.addr);
        let lamp = instances(w, OUTPOST, "pf_lamp0001")
            .into_iter()
            .find(|a| platform.is_some() && w.get(OUTPOST, a).and_then(|e| e.parent()) == platform);
        if c.some("a lamp instance on the Platform", lamp).is_some() {
            ids.extend(lamp);
        }
        let creators: Vec<&str> = ids
            .iter()
            .filter_map(|id| {
                w.history().iter().find(|t| {
                    t.ops
                        .iter()
                        .any(|op| matches!(op, Op::Create { .. }) && op.id().as_str() == *id)
                })
            })
            .map(|t| t.id.0.as_str())
            .collect();
        let single = creators.len() == ids.len() && creators.windows(2).all(|p| p[0] == p[1]);
        c.check(
            "all of it was created in one transaction",
            single,
            format!("created by {creators:?}"),
        );
        with_why(c, w, OUTPOST, &ids);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Add a watchtower by the gate",
            json!([
                { "cmd": "entity.create", "as": "$tower", "args": { "name": "Watchtower", "components": { "Transform": { "position": [10, 0, -6] } } } },
                { "cmd": "entity.create", "as": "$platform", "args": { "name": "Platform", "parent": "$tower", "components": { "Transform": { "position": [0, 4, 0] } } } },
                { "cmd": "entity.create", "args": { "name": "Ladder", "parent": "$tower", "components": { "Transform": { "position": [1, 2, 0] } } } },
                { "cmd": "prefab.instantiate", "args": { "prefab": "@prefabs/lamp.prefab", "parent": "$platform" } },
            ]),
        )
        .map(drop)
    },
};

/// A destructive change previewed first.
pub const DRY_RUN_RUINS: Task = Task {
    id: "dryrun-clear-ruins",
    area: Area::DryRun,
    prompt: "Clear out the Ruins in the outpost level: delete everything inside the Ruins group but keep the group itself. It's destructive, so preview exactly what would be deleted first, and only go ahead if nothing outside the Ruins is affected.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        if at(c, w, OUTPOST, "e_ruins1").is_some() {
            let left = w.children(OUTPOST, Some("e_ruins1")).len();
            c.check("Ruins is empty", left == 0, format!("{left} children left"));
        }
        for addr in ["e_rwall1", "e_rubbl1", "e_stone1"] {
            c.check(
                format!("`{addr}` is deleted"),
                w.get(OUTPOST, addr).is_none(),
                "still there",
            );
        }
        at(c, w, OUTPOST, "e_sign01");
        let dry = w.calls.iter().position(|t| t.is_dry_run());
        let delete = w.calls.iter().position(|t| t.applies("entity.delete"));
        c.check(
            "a dry run came before the delete",
            matches!((dry, delete), (Some(d), Some(x)) if d < x),
            format!("dry run at {dry:?}, delete at {delete:?}"),
        );
        with_why(c, w, OUTPOST, &["e_rwall1", "e_rubbl1", "e_stone1"]);
    },
    solve: |s: &mut Solver| {
        let delete =
            json!([{ "cmd": "entity.delete", "args": { "entities": ["e_rwall1", "e_rubbl1"] } }]);
        let preview = s.dry_run(OUTPOST, "Clear out the ruins", delete.clone())?;
        if preview["diff"]["deleted"].as_array().map_or(0, Vec::len) != 3 {
            return Err(format!("unexpected preview {}", preview["diff"]));
        }
        s.apply(OUTPOST, "Clear out the ruins", delete).map(drop)
    },
};

/// Creation, a collider, three instances, an override and a light in one request.
pub const STORAGE_ROOM: Task = Task {
    id: "multi-storage-room",
    area: Area::MultiStep,
    prompt: "Set up a storage room in the outpost level: an entity StorageRoom at (-8, 0, 4) with a 6 x 3 x 6 m box collider. Inside it, put three crates from the crate prefab in a row along X, 1.5 m apart, starting at local (-1.5, 0, 0). Paint the middle crate's body red. Then add a point light called StorageLight 2.5 m above the room's center, at 600 lm.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(40, 10),
    verify: |w: &World, c: &mut Checks| {
        let Some(room) = one(c, w, OUTPOST, "StorageRoom") else {
            return;
        };
        vec_is(
            c,
            "StorageRoom at (-8, 0, 4)",
            w.world_pos(OUTPOST, room.addr),
            &[-8.0, 0.0, 4.0],
        );
        c.check(
            "StorageRoom has a box collider",
            room.get("Collider.shape") == Some(json!("box")),
            format!("{:?}", room.get("Collider.shape")),
        );
        vec_is(
            c,
            "collider is 6 x 3 x 6 m",
            room.vec("Collider.size"),
            &[6.0, 3.0, 6.0],
        );
        let mut crates: Vec<(Vec<f64>, bool)> = instances(w, OUTPOST, "pf_crate001")
            .into_iter()
            .filter_map(|a| w.get(OUTPOST, a))
            .filter(|e| e.parent() == Some(room.addr))
            .map(|e| {
                (
                    e.position(),
                    red(&e.vec("MeshRenderer.tint").unwrap_or_default()),
                )
            })
            .collect();
        crates.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
        if c.check(
            "three crates inside the room",
            crates.len() == 3,
            format!("{} crates", crates.len()),
        ) {
            for ((pos, _), x) in crates.iter().zip([-1.5, 0.0, 1.5]) {
                vec_is(
                    c,
                    &format!("a crate at local ({x}, 0, 0)"),
                    Some(pos.clone()),
                    &[x, 0.0, 0.0],
                );
            }
            let reds: Vec<bool> = crates.iter().map(|(_, r)| *r).collect();
            c.check(
                "only the middle crate is red",
                reds == [false, true, false],
                format!("red {reds:?}"),
            );
        }
        if let Some(light) = one(c, w, OUTPOST, "StorageLight") {
            c.check(
                "StorageLight is a point light",
                light.get("Light.kind") == Some(json!("point")),
                format!("{:?}", light.get("Light.kind")),
            );
            num_is(
                c,
                "StorageLight is 600 lm",
                light.num("Light.intensity"),
                600.0,
            );
            vec_is(
                c,
                "StorageLight 2.5 m above the center",
                w.world_pos(OUTPOST, light.addr),
                &[-8.0, 2.5, 4.0],
            );
        }
        with_why(c, w, OUTPOST, &[room.addr]);
    },
    solve: |s: &mut Solver| {
        let crate_at = |x: f64| json!({ "cmd": "prefab.instantiate", "args": { "prefab": "@prefabs/crate.prefab", "parent": "$room", "position": [x, 0, 0] } });
        let mut calls = vec![
            json!({ "cmd": "entity.create", "as": "$room", "args": { "name": "StorageRoom", "components": { "Transform": { "position": [-8, 0, 4] }, "Collider": { "size": [6, 3, 6] } } } }),
        ];
        calls.push(crate_at(-1.5));
        let mut mid = crate_at(0.0);
        mid["as"] = json!("$mid");
        calls.push(mid);
        calls.push(crate_at(1.5));
        calls.push(json!({ "cmd": "property.set", "args": { "entity": "$mid", "path": "MeshRenderer.tint", "value": [1, 0, 0, 1] } }));
        calls.push(json!({ "cmd": "entity.create", "args": { "name": "StorageLight", "parent": "$room", "components": { "Transform": { "position": [0, 2.5, 0] }, "Light": { "kind": "point", "intensity": 600 } } } }));
        s.apply(OUTPOST, "Set up the storage room", Value::Array(calls))
            .map(drop)
    },
};

/// A property, a transform and an override revert, described as bugs.
pub const QA_FIXES: Task = Task {
    id: "multi-qa-fixes",
    area: Area::MultiStep,
    prompt: "QA found three problems in the outpost level: the Floodlight is switched off (it should be 1000 lm), Barrel.002 is floating in the air (it belongs on the ground), and the YardLamp's bulb has the wrong color; it should be whatever the lamp prefab says. Fix all three.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(30, 8),
    verify: |w: &World, c: &mut Checks| {
        if let Some(e) = at(c, w, OUTPOST, "e_flood1") {
            num_is(c, "Floodlight is 1000 lm", e.num("Light.intensity"), 1000.0);
        }
        if let Some(e) = at(c, w, OUTPOST, "e_brl003") {
            vec_is(
                c,
                "Barrel.002 is on the ground",
                Some(e.position()),
                &[-3.0, 0.0, 4.4],
            );
        }
        if let Some(e) = at(c, w, OUTPOST, "e_ylamp1/e_bulb01") {
            vec_is(
                c,
                "YardLamp bulb has the prefab color",
                e.vec("Light.color"),
                &[1.0; 4],
            );
        }
        if let Some(e) = at(c, w, OUTPOST, "e_ylamp1") {
            vec_is(
                c,
                "YardLamp stays at (0, 0, 8)",
                Some(e.position()),
                &[0.0, 0.0, 8.0],
            );
        }
        with_why(c, w, OUTPOST, &["e_flood1", "e_brl003", "e_ylamp1"]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Fix the QA issues: floodlight, floating barrel, yard lamp color",
            json!([
                { "cmd": "property.set", "args": { "entity": "e_flood1", "path": "Light.intensity", "value": 1000 } },
                { "cmd": "transform.set", "args": { "entities": ["e_brl003"], "position": [-3, 0, 4.4] } },
                { "cmd": "prefab.override.revert", "args": { "entity": "e_ylamp1/e_bulb01", "path": "Light.color" } },
            ]),
        )
        .map(drop)
    },
};
