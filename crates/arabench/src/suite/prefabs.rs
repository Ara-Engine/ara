//! Prefabs: instantiate, override one instance, revert overrides, reach inside.

use serde_json::{Value, json};

use super::{OUTPOST, at, prefabs_untouched, red, vec_is, with_why};
use crate::solve::Solver;
use crate::task::{Area, Budget, Checks, Fixture, Task};
use crate::world::World;

/// Root addresses of `doc`'s instances of the prefab with asset ID `prefab`.
pub(crate) fn instances<'w>(w: &'w World, doc: &str, prefab: &str) -> Vec<&'w str> {
    w.entities(doc)
        .into_iter()
        .filter(|e| !e.addr.contains('/'))
        .filter(|e| {
            w.record(doc, e.addr)
                .and_then(|r| r.get("prefab"))
                .and_then(Value::as_str)
                .is_some_and(|p| p.contains(prefab))
        })
        .map(|e| e.addr)
        .collect()
}

/// Two new instances, the second stacked on the first.
pub const STACK_CRATES: Task = Task {
    id: "prefab-stack-crates",
    area: Area::Prefabs,
    prompt: "Place two crates from the crate prefab near the player spawn in the outpost level: one at (4, 0, 2), and a second one stacked right on top of it (the crates are 1 m tall).",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        let new: Vec<&str> = instances(w, OUTPOST, "pf_crate001")
            .into_iter()
            .filter(|a| !["e_gcrt01", "e_gtcrt1"].contains(a))
            .collect();
        if !c.check(
            "two new crate instances",
            new.len() == 2,
            format!("{} new", new.len()),
        ) {
            return;
        }
        let mut pos: Vec<Vec<f64>> = new.iter().filter_map(|a| w.world_pos(OUTPOST, a)).collect();
        pos.sort_by(|a, b| a[1].total_cmp(&b[1]));
        vec_is(
            c,
            "lower crate at (4, 0, 2)",
            pos.first().cloned(),
            &[4.0, 0.0, 2.0],
        );
        vec_is(
            c,
            "upper crate at (4, 1, 2)",
            pos.get(1).cloned(),
            &[4.0, 1.0, 2.0],
        );
        with_why(c, w, OUTPOST, &new);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Stack two crates by the spawn",
            json!([
                { "cmd": "prefab.instantiate", "as": "$low", "args": { "prefab": "@prefabs/crate.prefab", "position": [4, 0, 2] } },
                { "cmd": "prefab.instantiate", "args": { "prefab": "@prefabs/crate.prefab", "parent": "$low", "position": [0, 1, 0] } },
            ]),
        )
        .map(drop)
    },
};

/// An override on one instance, not the prefab.
pub const TINT_ONE_CRATE: Task = Task {
    id: "prefab-tint-one-crate",
    area: Area::Prefabs,
    prompt: "In the outpost level, paint the crate next to the generator red: just that one crate's body (not its lid), and don't change any other crate.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        if let Some(e) = at(c, w, OUTPOST, "e_gcrt01") {
            let tint = e.vec("MeshRenderer.tint").unwrap_or_default();
            c.check(
                "GeneratorCrate body is red",
                red(&tint),
                format!("tint {tint:?}"),
            );
        }
        if let Some(e) = at(c, w, OUTPOST, "e_gcrt01/e_lid001") {
            vec_is(
                c,
                "its lid keeps the prefab tint",
                e.vec("MeshRenderer.tint"),
                &[0.8, 0.6, 0.4, 1.0],
            );
        }
        if let Some(e) = at(c, w, OUTPOST, "e_gtcrt1") {
            vec_is(
                c,
                "GateCrate is not tinted",
                e.vec("MeshRenderer.tint"),
                &[1.0; 4],
            );
        }
        prefabs_untouched(c, w);
        with_why(c, w, OUTPOST, &["e_gcrt01"]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Paint the generator crate red",
            json!([{ "cmd": "property.set", "args": { "entity": "e_gcrt01", "path": "MeshRenderer.tint", "value": [1, 0, 0, 1] } }]),
        )
        .map(drop)
    },
};

/// Reverting some overrides while keeping another.
pub const REVERT_KEEP_POSITION: Task = Task {
    id: "prefab-revert-keep-position",
    area: Area::Prefabs,
    prompt: "Someone resized the crate by the gate in the outpost level and recolored its lid. Put that crate back the way the crate prefab defines it, but leave it where it is.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        let overrides = w
            .record(OUTPOST, "e_gtcrt1")
            .and_then(|r| r.get("overrides"))
            .and_then(Value::as_object)
            .map(|o| o.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        c.check(
            "only the position override is left",
            overrides == ["Transform.position"],
            format!("overrides {overrides:?}"),
        );
        let removed = w
            .record(OUTPOST, "e_gtcrt1")
            .and_then(|r| r.get("removed"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        c.check(
            "nothing removed from the instance",
            removed == 0,
            format!("{removed} removed"),
        );
        if let Some(e) = at(c, w, OUTPOST, "e_gtcrt1") {
            vec_is(
                c,
                "GateCrate stays at (12, 0, 2)",
                Some(e.position()),
                &[12.0, 0.0, 2.0],
            );
            vec_is(
                c,
                "GateCrate scale is back to 1",
                e.vec("Transform.scale"),
                &[1.0; 3],
            );
        }
        if let Some(e) = at(c, w, OUTPOST, "e_gtcrt1/e_lid001") {
            vec_is(
                c,
                "its lid has the prefab tint",
                e.vec("MeshRenderer.tint"),
                &[0.8, 0.6, 0.4, 1.0],
            );
        }
        with_why(c, w, OUTPOST, &["e_gtcrt1"]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Restore the gate crate to the prefab, keeping its position",
            json!([
                { "cmd": "prefab.override.revert", "args": { "entity": "e_gtcrt1", "path": "Transform.scale" } },
                { "cmd": "prefab.override.revert", "args": { "entity": "e_gtcrt1/e_lid001", "path": "MeshRenderer.tint" } },
            ]),
        )
        .map(drop)
    },
};

/// An override on an entity inside an instance.
pub const BLUE_BULB: Task = Task {
    id: "prefab-blue-bulb",
    area: Area::Prefabs,
    prompt: "In the outpost level, make the bulb of the lamp by the gate shine blue. Leave the other lamps alone.",
    fixture: Fixture::Outpost,
    setup: None,
    budget: Budget::new(20, 5),
    verify: |w: &World, c: &mut Checks| {
        if let Some(e) = at(c, w, OUTPOST, "e_dlamp1/e_bulb01") {
            let color = e.vec("Light.color").unwrap_or_default();
            let blue = color.len() >= 3 && color[2] >= 0.8 && color[0] <= 0.2 && color[1] <= 0.3;
            c.check(
                "the gate lamp's bulb is blue",
                blue,
                format!("color {color:?}"),
            );
        }
        if let Some(e) = at(c, w, OUTPOST, "e_ylamp1/e_bulb01") {
            vec_is(
                c,
                "the yard lamp's bulb stays red",
                e.vec("Light.color"),
                &[1.0, 0.0, 0.0, 1.0],
            );
        }
        prefabs_untouched(c, w);
        with_why(c, w, OUTPOST, &["e_dlamp1"]);
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Blue bulb for the gate lamp",
            json!([{ "cmd": "property.set", "args": { "entity": "e_dlamp1/e_bulb01", "path": "Light.color", "value": [0, 0, 1, 1] } }]),
        )
        .map(drop)
    },
};
