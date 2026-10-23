//! Hierarchy: reparenting to a sibling position, reordering siblings.

use serde_json::json;

use super::{OUTPOST, at, parent_is, vec_is, with_why};
use crate::solve::Solver;
use crate::task::{Area, Budget, Checks, Fixture, Task};
use crate::world::World;

/// A reparent to the first child slot plus a local position.
pub const RADIO_TO_BENCH: Task = Task {
    id: "hierarchy-radio-to-bench",
    area: Area::Hierarchy,
    prompt: "In the outpost level, move the Radio off the shelf and onto the Workbench: it should be the workbench's first child, at local position (0, 1, 0).",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        let Some(radio) = at(c, w, OUTPOST, "e_radio1") else {
            return;
        };
        parent_is(c, &radio, Some("e_bench1"), "Workbench");
        let first = w
            .children(OUTPOST, Some("e_bench1"))
            .first()
            .map(|e| e.name());
        c.check(
            "Radio is the Workbench's first child",
            first == Some("Radio"),
            format!("first child {first:?}"),
        );
        vec_is(
            c,
            "Radio local position",
            Some(radio.position()),
            &[0.0, 1.0, 0.0],
        );
        with_why(c, w, OUTPOST, &["e_radio1"]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Put the radio on the workbench",
            json!([
                { "cmd": "entity.reparent", "args": { "entity": "e_radio1", "parent": "e_bench1", "index": 0 } },
                { "cmd": "transform.set", "args": { "entities": ["e_radio1"], "position": [0, 1, 0] } },
            ]),
        )
        .map(drop)
    },
};

/// Sibling order only; positions and parents stay.
pub const ORDER_WAYPOINTS: Task = Task {
    id: "hierarchy-order-waypoints",
    area: Area::Hierarchy,
    prompt: "The patrol waypoints under Patrol in the outpost level are out of order. Reorder them so they go WP1, WP2, WP3, WP4.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        let names: Vec<&str> = w
            .children(OUTPOST, Some("e_patrol"))
            .iter()
            .map(|e| e.name())
            .collect();
        c.check(
            "Patrol's children are WP1, WP2, WP3, WP4",
            names == ["WP1", "WP2", "WP3", "WP4"],
            format!("children {names:?}"),
        );
        for (addr, pos) in [
            ("e_wp001", [0.0, 0.0, 10.0]),
            ("e_wp002", [5.0, 0.0, 12.0]),
            ("e_wp003", [10.0, 0.0, 10.0]),
            ("e_wp004", [10.0, 0.0, 0.0]),
        ] {
            if let Some(e) = at(c, w, OUTPOST, addr) {
                vec_is(
                    c,
                    &format!("`{addr}` keeps its position"),
                    Some(e.position()),
                    &pos,
                );
            }
        }
        with_why(c, w, OUTPOST, &["e_wp001", "e_wp002", "e_wp003", "e_wp004"]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Put the patrol waypoints in order",
            json!([
                { "cmd": "entity.reparent", "args": { "entity": "e_wp001", "parent": "e_patrol", "index": 0 } },
                { "cmd": "entity.reparent", "args": { "entity": "e_wp002", "parent": "e_patrol", "index": 1 } },
            ]),
        )
        .map(drop)
    },
};
