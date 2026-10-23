//! Collaboration (D21): a human teammate, Mia, holds claims in the outpost level
//! while the agent works. Claims are enforced for agents, so the agent has to find
//! out what is claimed (from `ARA2500` or `who`) and work around it: do the rest
//! of the job, or queue the claimed part in a branch session.

use std::path::Path;

use ara_collab::BranchStatus;
use ara_host::{Host, HostConfig, Project};
use ara_ops::Author;
use ara_proto::ConnId;
use serde_json::{Value, json};

use super::{OUTPOST, at, num_is, with_why};
use crate::solve::Solver;
use crate::task::{Area, Budget, Checks, Fixture, Task};
use crate::world::World;

/// Mia (a human in the editor) joins and claims `subjects` of the outpost level for
/// three hours. Her session stays open, so the claim survives into the agent's run.
fn mia_claims(root: &Path, subjects: &[&str], why: &str) -> Result<(), String> {
    let project = Project::open(root).map_err(|d| d.to_string())?;
    let mut host = Host::open(project, HostConfig::default()).map_err(|d| d.to_string())?;
    let mut call = |method: &str, params: Value| {
        host.call(ConnId::LOCAL, method, params)
            .map_err(|e| format!("{method}: {e}"))
    };
    let begun = call(
        "session.begin",
        json!({ "client": "editor", "author": "human", "label": "Mia" }),
    )?;
    let s = begun["session"].as_str().unwrap_or_default().to_owned();
    call("collab.join", json!({ "session": s, "name": "Mia" }))?;
    call(
        "collab.claim",
        json!({ "session": s, "doc": OUTPOST, "subjects": subjects, "why": why, "ttlMs": 3 * 3600 * 1000 }),
    )?;
    host.flush();
    Ok(())
}

/// Checks that no agent transaction touched record `id` and no file was edited by
/// hand (which would sidestep the claim).
fn left_alone(c: &mut Checks, w: &World, id: &str, what: &str) {
    let touched: Vec<String> = w
        .txns_touching(OUTPOST, id)
        .iter()
        .filter(|t| matches!(t.author, Author::Agent(_)))
        .map(|t| t.id.to_string())
        .collect();
    c.check(
        format!("no agent changed {what} while it was claimed"),
        touched.is_empty(),
        touched.join(", "),
    );
    c.check(
        "no hand edits of the scene file",
        w.external_edits() == 0,
        format!("{} external edits", w.external_edits()),
    );
}

/// Work around a claimed entity: do everything else, leave the claimed light alone.
pub const CLAIMED_LIGHTS: Task = Task {
    id: "collab-avoid-claimed",
    area: Area::Collab,
    prompt: "Double the brightness of every light in the outpost level. Heads up: a teammate is working in that level right now too.",
    fixture: Fixture::Outpost,
    setup: Some(|root| {
        mia_claims(
            root,
            &["e_dlamp1", "e_door01"],
            "re-timing the gate and its lamp",
        )
    }),
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        for (addr, want) in [
            ("e_wlight1", 2400.0),
            ("e_flood1", 0.0),
            ("e_ylamp1/e_bulb01", 1600.0),
        ] {
            if let Some(e) = at(c, w, OUTPOST, addr) {
                num_is(
                    c,
                    &format!("`{addr}` doubled"),
                    e.num("Light.intensity"),
                    want,
                );
            }
        }
        if let Some(e) = at(c, w, OUTPOST, "e_dlamp1/e_bulb01") {
            num_is(
                c,
                "Mia's claimed DoorLamp is untouched",
                e.num("Light.intensity"),
                800.0,
            );
        }
        left_alone(c, w, "e_dlamp1", "the DoorLamp");
        with_why(c, w, OUTPOST, &["e_wlight1", "e_ylamp1"]);
    },
    solve: |s: &mut Solver| {
        let who = s.call("collab.who", json!({}))?;
        let text = who["text"].as_str().unwrap_or_default();
        if !text.contains("e_dlamp1") {
            return Err(format!("the claim is not listed: {text}"));
        }
        s.apply(
            OUTPOST,
            "Double the lights Mia is not working on",
            json!([
                { "cmd": "property.set", "args": { "entity": "e_wlight1", "path": "Light.intensity", "value": 2400 } },
                { "cmd": "property.set", "args": { "entity": "e_ylamp1/e_bulb01", "path": "Light.intensity", "value": 1600 } },
            ]),
        )
        .map(drop)
    },
};

/// Queue a change to a claimed entity in a branch session instead of waiting.
pub const BRANCH_FOR_CLAIMED: Task = Task {
    id: "collab-branch-claimed",
    area: Area::Collab,
    prompt: "The gate door in the outpost level should slide twice as fast. Mia is reworking the gate right now, so don't change it under her feet; set the change up so it goes in once she is done.",
    fixture: Fixture::Outpost,
    setup: Some(|root| mia_claims(root, &["e_door01"], "reworking the gate")),
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        if let Some(d) = at(c, w, OUTPOST, "e_door01") {
            num_is(
                c,
                "the live door is untouched while Mia works",
                d.num("Door.speed"),
                1.2,
            );
        }
        left_alone(c, w, "e_door01", "the GateDoor");
        let queued = w.branches().into_iter().find(|b| {
            matches!(b.author, Author::Agent(_))
                && matches!(b.status, BranchStatus::Submitted | BranchStatus::Merged)
        });
        let Some(b) = c.some("a submitted branch session holds the change", queued) else {
            return;
        };
        let speed = b
            .store()
            .workspace()
            .doc("sc_outpost1")
            .and_then(|d| d.record("e_door01"))
            .and_then(|r| r.get("Door"))
            .and_then(|d| d.get("speed"))
            .and_then(Value::as_f64);
        num_is(c, "the queued change doubles the door speed", speed, 2.4);
    },
    solve: |s: &mut Solver| {
        let opened = s.call(
            "branch.open",
            json!({ "why": "Double the gate door speed once Mia is done" }),
        )?;
        let branch = opened["id"].as_str().unwrap_or_default().to_owned();
        s.call(
            "cmd.apply",
            json!({ "doc": OUTPOST, "branch": branch, "why": "Faster gate door",
                "commands": [{ "cmd": "property.set", "args": { "entity": "e_door01", "path": "Door.speed", "value": 2.4 } }] }),
        )?;
        s.call("branch.submit", json!({ "branch": branch }))
            .map(drop)
    },
};
