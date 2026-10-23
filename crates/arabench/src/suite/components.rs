//! Components and properties: add, set across many entities, remove.

use serde_json::json;

use super::{OUTPOST, at, num_is, one, with_why};
use crate::solve::Solver;
use crate::task::{Area, Budget, Checks, Fixture, Task};
use crate::world::World;

/// A new component with values in several units, one of them a color.
pub const BEACON_LIGHT: Task = Task {
    id: "component-beacon-light",
    area: Area::Components,
    prompt: "Make the Beacon in the outpost level glow: give it an orange point light of 1500 lumens that reaches about 8 m.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        let Some(b) = one(c, w, OUTPOST, "Beacon") else {
            return;
        };
        if !c.check("Beacon has a Light", b.has("Light"), "no Light") {
            return;
        }
        c.check(
            "the light is a point light",
            b.get("Light.kind") == Some(json!("point")),
            format!("{:?}", b.get("Light.kind")),
        );
        num_is(c, "intensity is 1500 lm", b.num("Light.intensity"), 1500.0);
        num_is(c, "range is 8 m", b.num("Light.range"), 8.0);
        // Colors are linear: sRGB orange (1, 0.5, 0) is about (1, 0.21, 0).
        let color = b.vec("Light.color").unwrap_or_default();
        let orange = color.len() >= 3
            && color[0] >= 0.8
            && (0.1..=0.75).contains(&color[1])
            && color[2] <= 0.3;
        c.check("the light is orange", orange, format!("color {color:?}"));
        with_why(c, w, OUTPOST, &[b.addr]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Give the beacon an orange glow",
            json!([{ "cmd": "component.add", "args": { "entity": "e_beacon1", "component": "Light", "value": { "kind": "point", "intensity": 1500, "range": 8, "color": [1, 0.5, 0, 1] } } }]),
        )
        .map(drop)
    },
};

/// The same field on every light, two of them inside prefab instances.
pub const HALVE_LIGHTS: Task = Task {
    id: "property-halve-lights",
    area: Area::Components,
    prompt: "The outpost level is lit way too brightly. Halve the intensity of every light in it.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        let lights = w
            .entities(OUTPOST)
            .iter()
            .filter(|e| e.has("Light"))
            .count();
        c.check("still four lights", lights == 4, format!("{lights} lights"));
        for (addr, want) in [
            ("e_wlight1", 600.0),
            ("e_flood1", 0.0),
            ("e_dlamp1/e_bulb01", 400.0),
            ("e_ylamp1/e_bulb01", 400.0),
        ] {
            if let Some(e) = at(c, w, OUTPOST, addr) {
                num_is(
                    c,
                    &format!("`{addr}` intensity halved"),
                    e.num("Light.intensity"),
                    want,
                );
            }
        }
        with_why(c, w, OUTPOST, &["e_wlight1"]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Halve every light's intensity",
            json!([
                { "cmd": "property.set", "args": { "entity": "e_wlight1", "path": "Light.intensity", "value": 600 } },
                { "cmd": "property.set", "args": { "entity": "e_dlamp1/e_bulb01", "path": "Light.intensity", "value": 400 } },
                { "cmd": "property.set", "args": { "entity": "e_ylamp1/e_bulb01", "path": "Light.intensity", "value": 400 } },
            ]),
        )
        .map(drop)
    },
};

/// Removing one component while keeping a related one.
pub const REMOVE_RIGIDBODY: Task = Task {
    id: "component-remove-rigidbody",
    area: Area::Components,
    prompt: "In the outpost level, the first barrel (the one called just Barrel) shouldn't get knocked around by physics anymore. Take its rigid body off, but leave its collider.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        let Some(b) = at(c, w, OUTPOST, "e_brl001") else {
            return;
        };
        c.check(
            "Barrel has no RigidBody",
            !b.has("RigidBody"),
            "still there",
        );
        c.check("Barrel keeps its Collider", b.has("Collider"), "removed");
        for addr in ["e_brl002", "e_brl003"] {
            if let Some(o) = at(c, w, OUTPOST, addr) {
                c.check(
                    format!("`{addr}` keeps its RigidBody"),
                    o.has("RigidBody"),
                    "removed",
                );
            }
        }
        with_why(c, w, OUTPOST, &[b.addr]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Stop physics pushing the first barrel",
            json!([{ "cmd": "component.remove", "args": { "entity": "e_brl001", "component": "RigidBody" } }]),
        )
        .map(drop)
    },
};
