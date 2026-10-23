//! Scene assembly: create, name, parent and place entities, with units.

use serde_json::json;

use super::{MAIN, OUTPOST, close, num_is, one, parent_is, vec_is, with_why};
use crate::solve::Solver;
use crate::task::{Area, Budget, Checks, Fixture, Task};
use crate::world::World;

/// A floor with a box collider and a crate parented to it, in the empty scene.
pub const FLOOR_CRATE: Task = Task {
    id: "scene-floor-crate",
    area: Area::Assembly,
    prompt: "Start building the main level (levels/main.scene). Add a floor entity called Floor with a 20 x 0.5 x 20 m box collider, and a Crate entity that sits on it: make the crate a child of the floor, at local position (2, 0.5, 3).",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        let Some(floor) = one(c, w, MAIN, "Floor") else {
            return;
        };
        c.check(
            "Floor has a box collider",
            floor.get("Collider.shape") == Some(json!("box")),
            format!("{:?}", floor.get("Collider.shape")),
        );
        vec_is(
            c,
            "Floor collider is 20 x 0.5 x 20 m",
            floor.vec("Collider.size"),
            &[20.0, 0.5, 20.0],
        );
        let Some(crate_) = one(c, w, MAIN, "Crate") else {
            return;
        };
        parent_is(c, &crate_, Some(floor.addr), "Floor");
        vec_is(
            c,
            "Crate local position",
            Some(crate_.position()),
            &[2.0, 0.5, 3.0],
        );
        with_why(c, w, MAIN, &[floor.addr, crate_.addr]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            MAIN,
            "Start the main level with a floor and a crate on it",
            json!([
                { "cmd": "entity.create", "as": "$floor", "args": { "name": "Floor", "components": { "Transform": {}, "Collider": { "size": [20, 0.5, 20] } } } },
                { "cmd": "entity.create", "args": { "name": "Crate", "parent": "$floor", "components": { "Transform": { "position": [2, 0.5, 3] } } } },
            ]),
        )
        .map(drop)
    },
};

/// Rotation in degrees about Y and a relative move.
pub const TURN_AND_RAISE: Task = Task {
    id: "scene-turn-and-raise",
    area: Area::Assembly,
    prompt: "In the outpost level, turn the Generator a quarter turn around the vertical axis and raise it 1.5 m from where it stands now.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        let Some(g) = one(c, w, OUTPOST, "Generator") else {
            return;
        };
        let rot = g.vec("Transform.rotation").unwrap_or_default();
        let quarter = rot.len() == 3
            && close(rot[0], 0.0)
            && close(rot[2], 0.0)
            && [90.0, -90.0, 270.0, -270.0]
                .iter()
                .any(|q| close(rot[1], *q));
        c.check(
            "Generator turned 90 degrees about Y",
            quarter,
            format!("rotation {rot:?}"),
        );
        vec_is(
            c,
            "Generator raised 1.5 m",
            Some(g.position()),
            &[6.0, 1.5, -4.0],
        );
        with_why(c, w, OUTPOST, &[g.addr]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Turn the generator a quarter turn and raise it 1.5 m",
            json!([{ "cmd": "transform.set", "args": { "entities": ["e_gen001"], "position": [6, 1.5, -4], "rotation": [0, 90, 0] } }]),
        )
        .map(drop)
    },
};

/// A new group entity, three reparents in a given order, world positions kept.
pub const GROUP_BARRELS: Task = Task {
    id: "scene-group-barrels",
    area: Area::Assembly,
    prompt: "Tidy up the outpost hierarchy: put the three barrels (Barrel, Barrel.001 and Barrel.002) under a new empty entity called Barrels at the origin, in that order, without moving them in the world.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        let Some(group) = one(c, w, OUTPOST, "Barrels") else {
            return;
        };
        parent_is(c, &group, None, "the scene root");
        let kids: Vec<&str> = w
            .children(OUTPOST, Some(group.addr))
            .iter()
            .map(|e| e.name())
            .collect();
        c.check(
            "Barrels holds Barrel, Barrel.001, Barrel.002 in order",
            kids == ["Barrel", "Barrel.001", "Barrel.002"],
            format!("children {kids:?}"),
        );
        for (addr, pos) in [
            ("e_brl001", [-3.0, 0.0, 2.0]),
            ("e_brl002", [-3.0, 0.0, 3.2]),
            ("e_brl003", [-3.0, 5.0, 4.4]),
        ] {
            vec_is(
                c,
                &format!("`{addr}` keeps its world position"),
                w.world_pos(OUTPOST, addr),
                &pos,
            );
        }
        num_is(
            c,
            "Barrels sits at the origin",
            w.world_pos(OUTPOST, group.addr)
                .map(|p| p.iter().map(|x| x.abs()).sum()),
            0.0,
        );
        with_why(c, w, OUTPOST, &["e_brl001", "e_brl002", "e_brl003"]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Group the barrels under one entity",
            json!([
                { "cmd": "entity.create", "as": "$group", "args": { "name": "Barrels", "components": { "Transform": {} } } },
                { "cmd": "entity.reparent", "args": { "entity": "e_brl001", "parent": "$group" } },
                { "cmd": "entity.reparent", "args": { "entity": "e_brl002", "parent": "$group" } },
                { "cmd": "entity.reparent", "args": { "entity": "e_brl003", "parent": "$group" } },
            ]),
        )
        .map(drop)
    },
};
