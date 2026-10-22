//! Live-link suites against a running player: inspect, patch round trips, time control, capture,
//! and the "Keep changes" flow that turns runtime changes into edit-world ops.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{Result, bail, ensure};
use bevy_ecs::world::World;
use serde_json::{Value, json};

use super::Ctx;
use super::players::{Launch, Player, Prepared, Transport};
use super::stats::{Stats, ms};
use crate::model::{Orbit, SceneId, state_hash};
use crate::protocol::*;
use crate::registry::{Op, apply_ops, entity_of, read_path};

fn orbiters(edit: &mut World) -> Vec<SceneId> {
    let mut ids: Vec<SceneId> = edit
        .query::<(&SceneId, &Orbit)>()
        .iter(edit)
        .map(|(id, _)| *id)
        .collect();
    ids.sort_unstable();
    ids
}

fn start_player(ctx: &mut Ctx) -> Result<(Player, Prepared)> {
    let mut p = ctx.host.launch(&Launch::standalone())?;
    let snap = Prepared::encode(&mut ctx.edit, Transport::Shm, &mut ctx.host)?;
    p.start(&snap, false)?;
    Ok((p, snap))
}

/// Patch round trips: host sends a `set` → player applies it at the next tick boundary → host
/// receives the ack with the read-back value. Sends are spread over the tick phase.
fn patch_round_trips(p: &mut Player, ids: &[SceneId], n: usize, jitter: bool) -> Result<Value> {
    let (mut rtt, mut mismatches, mut waited) = (vec![], 0, vec![]);
    for i in 0..n {
        if jitter {
            std::thread::sleep(Duration::from_micros(((i * 3727) % 17_000) as u64));
        }
        let target = ids[(i * 7919) % ids.len()];
        let value = json!(1.0 + (i % 40) as f64 * 0.25);
        let op = Op::Set {
            target,
            path: "Orbit.radius".into(),
            value: value.clone(),
            prev: None,
        };
        let t = Instant::now();
        let r = p.call::<Patch>(&PatchParams { ops: vec![op] })?;
        rtt.push(ms(t.elapsed()));
        waited.push((r.tick - r.applied_at_tick) as f64);
        if r.read_back.first() != Some(&Some(value)) {
            mismatches += 1;
        }
    }
    Ok(
        json!({ "rtt": Stats::of(&rtt), "readBackMismatches": mismatches, "ticksRunBeforeAck": Stats::of(&waited) }),
    )
}

pub fn live_suite(ctx: &mut Ctx, n: usize) -> Result<Value> {
    let ids = orbiters(&mut ctx.edit);
    let (mut p, _snap) = start_player(ctx)?;

    let mut inspect = vec![];
    for i in 0..n {
        let target = ids[(i * 7919) % ids.len()];
        let t = Instant::now();
        let r = p.call::<Inspect>(&InspectParams {
            target,
            components: None,
        })?;
        inspect.push(ms(t.elapsed()));
        ensure!(
            r.components.contains_key("Orbit"),
            "inspect lost components"
        );
    }

    let patch = patch_round_trips(&mut p, &ids, n, true)?;

    let mut capture = vec![];
    for _ in 0..n.min(100) {
        let t = Instant::now();
        p.call::<Capture>(&Empty {})?;
        capture.push(ms(t.elapsed()));
    }

    // Time control.
    let t = Instant::now();
    let paused = p.call::<Pause>(&Empty {})?.tick;
    let pause_rtt = ms(t.elapsed());
    std::thread::sleep(Duration::from_millis(200));
    let frozen = p.call::<Capture>(&Empty {})?;
    let t = Instant::now();
    let stepped = p.call::<Step>(&StepParams { ticks: 60 })?;
    let step_rtt = ms(t.elapsed());
    let paused_patch = patch_round_trips(&mut p, &ids, n.min(100), false)?;
    let resumed = p.call::<Resume>(&Empty {})?.tick;
    let t = Instant::now();
    std::thread::sleep(Duration::from_millis(1000));
    let after = p.call::<Capture>(&Empty {})?;
    let rate = (after.tick - resumed) as f64 / t.elapsed().as_secs_f64();
    let max_late_us = p.last_status.as_ref().map(|s| s.max_late_us);
    p.stop()?;

    let control = json!({
        "pauseRttMs": pause_rtt,
        "pausedAtTick": paused,
        "tickAfter200msPaused": frozen.tick,
        "frozenWhilePaused": frozen.tick == paused,
        "step60RttMs": step_rtt,
        "tickAfterStep60": stepped.tick,
        "stepExact": stepped.tick == paused + 60,
        "resumedTickRateHz": rate,
        "lastHeartbeatMaxLateUs": max_late_us,
    });
    println!(
        "  live inspect                  {}",
        Stats::of(&inspect).short()
    );
    println!(
        "  live patch (running, 60 Hz)   p50/p95 {:.2} / {:.2} ms, read-back mismatches {}",
        patch["rtt"]["p50"].as_f64().unwrap_or(f64::NAN),
        patch["rtt"]["p95"].as_f64().unwrap_or(f64::NAN),
        patch["readBackMismatches"]
    );
    println!(
        "  live patch (paused)           p50/p95 {:.2} / {:.2} ms",
        paused_patch["rtt"]["p50"].as_f64().unwrap_or(f64::NAN),
        paused_patch["rtt"]["p95"].as_f64().unwrap_or(f64::NAN)
    );
    println!(
        "  live capture (state hash)     {}",
        Stats::of(&capture).short()
    );
    println!(
        "  control: frozen {} step exact {} resumed {:.1} Hz",
        frozen.tick == paused,
        stepped.tick == paused + 60,
        rate
    );
    Ok(json!({
        "inspectRtt": Stats::of(&inspect),
        "patchWhileRunning": patch,
        "patchWhilePaused": paused_patch,
        "captureRtt": Stats::of(&capture),
        "control": control,
    }))
}

/// Simulation-driven fields an editor leaves unticked by default in the Keep-changes preview.
fn is_sim_driven(op: &Op) -> bool {
    matches!(op, Op::Set { path, .. } if path.starts_with("Transform") || path == "Orbit.phase")
}

/// An op conflicts if the edit world no longer holds the value the player started from.
fn conflicts(edit: &World, ops: &[Op]) -> Vec<String> {
    ops.iter()
        .filter_map(|op| {
            let ok = match op {
                Op::Set {
                    target, path, prev, ..
                } => read_path(edit, *target, path) == *prev,
                Op::Remove {
                    target,
                    component,
                    prev,
                } => read_path(edit, *target, component) == *prev,
                Op::Create { id, .. } => id.is_some_and(|id| entity_of(edit, id).is_err()),
                Op::Delete { target, .. } => entity_of(edit, *target).is_ok(),
            };
            (!ok).then(|| format!("{op:?}"))
        })
        .collect()
}

pub fn keep_suite(ctx: &mut Ctx) -> Result<Value> {
    let original = state_hash(&mut ctx.edit);
    let (a, b, c, d) = (SceneId(2), SceneId(3), SceneId(4), SceneId(5));
    let (mut p, _snap) = start_player(ctx)?;

    // Live tweaks during play, through the same op vocabulary.
    let spawn: BTreeMap<String, Value> = [
        ("Label".to_owned(), json!("Spawned in play")),
        ("Tint".to_owned(), json!({ "rgba": [0.0, 1.0, 0.0, 1.0] })),
        ("Transform".to_owned(), json!({ "position": [1.0, 2.0, 3.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] })),
    ]
    .into();
    let tweaks = vec![
        Op::Set {
            target: a,
            path: "Orbit.radius".into(),
            value: json!(9.5),
            prev: None,
        },
        Op::Set {
            target: b,
            path: "Tint.rgba".into(),
            value: json!([1.0, 0.0, 0.0, 1.0]),
            prev: None,
        },
        Op::Set {
            target: c,
            path: "Label".into(),
            value: json!("Renamed in play"),
            prev: None,
        },
        Op::Create {
            id: None,
            components: spawn,
        },
        Op::Delete {
            target: d,
            snapshot: None,
        },
    ];
    let patched = p.call::<Patch>(&PatchParams { ops: tweaks })?;
    let Some(created) = patched.inverse.iter().find_map(|op| match op {
        Op::Delete { target, .. } => Some(*target),
        _ => None,
    }) else {
        bail!("create did not report its id");
    };
    std::thread::sleep(Duration::from_millis(500));
    p.call::<Pause>(&Empty {})?;

    // Stop → "Keep changes" preview for the selected entities.
    let t = Instant::now();
    let diff = p.call::<Diff>(&DiffParams {
        targets: Some(vec![a, b, c, d, created]),
        components: None,
    })?;
    let diff_ms = ms(t.elapsed());
    let chosen: Vec<Op> = diff
        .ops
        .iter()
        .filter(|op| !is_sim_driven(op))
        .cloned()
        .collect();
    let conflicts = conflicts(&ctx.edit, &chosen);

    let t = Instant::now();
    let inverse = apply_ops(&mut ctx.edit, &chosen)?;
    let apply_ms = ms(t.elapsed());
    let kept = state_hash(&mut ctx.edit);
    let radius_runtime = p.call::<Inspect>(&InspectParams {
        target: a,
        components: Some(vec!["Orbit".into()]),
    })?;
    let radius_edit = read_path(&ctx.edit, a, "Orbit.radius");
    let values_match = chosen.iter().all(|op| match op {
        Op::Set {
            target,
            path,
            value,
            ..
        } => read_path(&ctx.edit, *target, path).as_ref() == Some(value),
        Op::Create { id, .. } => id.is_some_and(|id| entity_of(&ctx.edit, id).is_ok()),
        Op::Delete { target, .. } => entity_of(&ctx.edit, *target).is_err(),
        Op::Remove {
            target, component, ..
        } => read_path(&ctx.edit, *target, component).is_none(),
    });

    apply_ops(&mut ctx.edit, &inverse)?;
    let undone = state_hash(&mut ctx.edit);
    let redo_inverse = apply_ops(&mut ctx.edit, &chosen)?;
    let redone = state_hash(&mut ctx.edit);
    // Leave the edit world pristine for the other suites.
    apply_ops(&mut ctx.edit, &redo_inverse)?;

    let t = Instant::now();
    let full = p.call::<Diff>(&DiffParams {
        targets: None,
        components: None,
    })?;
    let full_ms = ms(t.elapsed());
    p.stop()?;

    let ok = conflicts.is_empty()
        && values_match
        && undone == original
        && redone == kept
        && kept != original;
    println!(
        "  keep changes: {} ops in preview, {} chosen, applied in {:.3} ms; undo restores {}; redo {}; conflicts {}",
        diff.ops.len(),
        chosen.len(),
        apply_ms,
        undone == original,
        redone == kept,
        conflicts.len()
    );
    println!(
        "  full diff of every entity: {} ops in {:.1} ms",
        full.ops.len(),
        full_ms
    );
    Ok(json!({
        "pass": ok,
        "tweaks": patched.inverse.len(),
        "createdId": created,
        "diffTick": diff.tick,
        "diffRttMs": diff_ms,
        "previewOps": diff.ops,
        "chosenOps": chosen,
        "inverseOps": inverse,
        "conflicts": conflicts,
        "applyMs": apply_ms,
        "editValuesMatchChosenOps": values_match,
        "runtimeOrbit": radius_runtime.components.get("Orbit"),
        "editOrbitRadius": radius_edit,
        "undoRestoresOriginalHash": undone == original,
        "redoReproducesKeptHash": redone == kept,
        "fullDiffOps": full.ops.len(),
        "fullDiffRttMs": full_ms,
    }))
}
