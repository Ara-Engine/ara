//! The task suites. `v0` uses only what the engine can do today (documents,
//! commands, prefabs, history, review, schema, docs, merge); tasks that need the
//! renderer, simulation or C# join when those land.
//!
//! Every task starts from the outpost fixture (`fixtures/outpost/` over a fresh
//! `ara init`), so `levels/main.scene` is empty and `levels/outpost.scene` is a
//! populated level. Prompts read like user requests; verifiers check the
//! documents, history and calls, never the agent's prose.

mod assembly;
mod components;
mod diagnose;
mod hierarchy;
mod prefabs;
mod repair;
mod workflow;

use std::collections::BTreeSet;

use ara_ops::Transaction;

use crate::task::{Checks, Task};
use crate::world::{EXTERNAL_EDIT, Ent, World};

/// The empty starter scene.
pub const MAIN: &str = "levels/main.scene";
/// The populated outpost level.
pub const OUTPOST: &str = "levels/outpost.scene";

/// Suite names, for `--suite`.
pub const SUITES: &[&str] = &["v0", "diagnose"];

/// The tasks of suite `name`.
pub fn suite(name: &str) -> Option<Vec<Task>> {
    match name {
        "v0" => Some(v0()),
        "diagnose" => Some(diagnose()),
        _ => None,
    }
}

/// Suite `diagnose`: four broken scenes described only by their symptom (a bad
/// material, a mesh lost in a re-export, a culling mask, a NaN from physics).
pub fn diagnose() -> Vec<Task> {
    vec![
        diagnose::BAD_MATERIAL,
        diagnose::MISSING_MESH,
        diagnose::CULLING_MASK,
        diagnose::NAN_BARREL,
    ]
}

/// Suite v0: 22 tasks over scene assembly, components, prefabs, hierarchy,
/// references, merge, schema, undo, diagnostics, docs, batches and dry runs.
pub fn v0() -> Vec<Task> {
    vec![
        assembly::FLOOR_CRATE,
        assembly::TURN_AND_RAISE,
        assembly::GROUP_BARRELS,
        components::BEACON_LIGHT,
        components::HALVE_LIGHTS,
        components::REMOVE_RIGIDBODY,
        prefabs::STACK_CRATES,
        prefabs::TINT_ONE_CRATE,
        prefabs::REVERT_KEEP_POSITION,
        prefabs::BLUE_BULB,
        hierarchy::RADIO_TO_BENCH,
        hierarchy::ORDER_WAYPOINTS,
        repair::BROKEN_REFS,
        repair::MERGE_CONFLICT,
        repair::FIX_TYPOS,
        workflow::DOOR_SCHEMA,
        workflow::UNDO_OTHER_AGENT,
        workflow::DOCS_CHEATSHEET,
        workflow::WATCHTOWER_BATCH,
        workflow::DRY_RUN_RUINS,
        workflow::STORAGE_ROOM,
        workflow::QA_FIXES,
    ]
}

/// Equal within a millimeter (or a thousandth of a unit).
pub(crate) fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

fn show(v: &[f64]) -> String {
    let parts: Vec<String> = v.iter().map(|x| ara_json::format_f64(*x)).collect();
    format!("({})", parts.join(", "))
}

/// The one entity of `doc` named `name`.
pub(crate) fn one<'w>(c: &mut Checks, w: &'w World, doc: &str, name: &str) -> Option<Ent<'w>> {
    let found = w.named(doc, name);
    let ok = found.len() == 1;
    c.check(
        format!("one `{name}` in {doc}"),
        ok,
        format!("found {}", found.len()),
    );
    found.into_iter().next().filter(|_| ok)
}

/// The entity at `addr`, which the fixture defines.
pub(crate) fn at<'w>(c: &mut Checks, w: &'w World, doc: &str, addr: &str) -> Option<Ent<'w>> {
    c.some(format!("`{addr}` still exists"), w.get(doc, addr))
}

/// Checks a vector (or color) against `want`.
pub(crate) fn vec_is(c: &mut Checks, what: &str, got: Option<Vec<f64>>, want: &[f64]) -> bool {
    let ok = got
        .as_ref()
        .is_some_and(|g| g.len() == want.len() && g.iter().zip(want).all(|(a, b)| close(*a, *b)));
    let detail = match &got {
        Some(g) => format!("got {}, want {}", show(g), show(want)),
        None => format!("missing, want {}", show(want)),
    };
    c.check(what, ok, if ok { String::new() } else { detail })
}

/// Checks a number against `want`.
pub(crate) fn num_is(c: &mut Checks, what: &str, got: Option<f64>, want: f64) -> bool {
    let ok = got.is_some_and(|g| close(g, want));
    let detail = format!("got {got:?}, want {want}");
    c.check(what, ok, if ok { String::new() } else { detail })
}

/// Checks that `e`'s parent is `parent` (`None`: a root).
pub(crate) fn parent_is(c: &mut Checks, e: &Ent<'_>, parent: Option<&str>, label: &str) -> bool {
    let got = e.parent();
    c.check(
        format!("`{}` is a child of {label}", e.name()),
        got == parent,
        format!("parent is {got:?}"),
    )
}

/// The document record behind an entity address (`e_4f9a/e_m4tz` → `e_4f9a`).
pub(crate) fn record_of(addr: &str) -> &str {
    addr.split('/').next().unwrap_or(addr)
}

fn has_why(t: &Transaction) -> bool {
    t.why.as_deref().is_some_and(|w| !w.trim().is_empty())
}

/// Checks that the records `ids` of `doc` were changed, only by engine commands that
/// carry a `why` (not raw file edits).
pub(crate) fn with_why(c: &mut Checks, w: &World, doc: &str, ids: &[&str]) -> bool {
    let mut seen = BTreeSet::new();
    let txns: Vec<&Transaction> = ids
        .iter()
        .flat_map(|id| w.txns_touching(doc, record_of(id)))
        .filter(|t| seen.insert(t.id.clone()))
        .collect();
    let bad: Vec<String> = txns
        .iter()
        .filter(|t| !has_why(t) || t.label.as_deref() == Some(EXTERNAL_EDIT))
        .map(|t| format!("{} ({})", t.id, t.label.as_deref().unwrap_or("no label")))
        .collect();
    let ok = !txns.is_empty() && bad.is_empty();
    let detail = if txns.is_empty() {
        "no transaction touched them".to_owned()
    } else {
        format!("without a why or from a file edit: {}", bad.join(", "))
    };
    c.check(
        "changed through commands with a `why`",
        ok,
        if ok { String::new() } else { detail },
    )
}

/// Checks that `ara check` reports no errors.
pub(crate) fn clean(c: &mut Checks, w: &World) -> bool {
    let errors: Vec<String> = w
        .errors()
        .iter()
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    c.check("`ara check` passes", errors.is_empty(), errors.join("; "))
}

/// Checks that the prefab files are exactly as the fixture has them.
pub(crate) fn prefabs_untouched(c: &mut Checks, w: &World) -> bool {
    let dir = crate::fixture::fixtures_dir().join("outpost");
    let changed: Vec<&str> = ["prefabs/crate.prefab", "prefabs/lamp.prefab"]
        .into_iter()
        .filter(|rel| std::fs::read_to_string(dir.join(rel)).ok() != w.file(rel))
        .collect();
    c.check(
        "prefab files unchanged",
        changed.is_empty(),
        changed.join(", "),
    )
}

/// True for a red color.
pub(crate) fn red(v: &[f64]) -> bool {
    v.len() >= 3 && v[0] >= 0.9 && v[1] <= 0.1 && v[2] <= 0.1
}
