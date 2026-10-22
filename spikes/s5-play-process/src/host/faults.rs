//! Crash isolation (panic, access violation, hang) and multi-client launch suites.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::Ctx;
use super::players::{Launch, Player, Prepared, Transport};
use super::stats::{Stats, ms};
use crate::model::state_hash;
use crate::protocol::*;

/// Crash each way, report the structured fault, then take the "restart" action.
pub fn crash_suite(ctx: &mut Ctx) -> Result<Value> {
    let edit_before = state_hash(&mut ctx.edit);
    let mut results = vec![];
    for kind in [
        CrashKind::Panic,
        CrashKind::AccessViolation,
        CrashKind::Hang,
    ] {
        let mut p = ctx.host.launch(&Launch::standalone())?;
        let snap = Prepared::encode(&mut ctx.edit, Transport::Shm, &mut ctx.host)?;
        p.start(&snap, false)?;
        std::thread::sleep(Duration::from_millis(300));
        p.call::<Capture>(&Empty {})?;

        let t = Instant::now();
        p.link.request::<Crash>(&CrashParams { kind })?;
        let failure = p.wait_failure();
        let detected = ms(t.elapsed());
        let fault = p.fault(failure);
        let reported = ms(t.elapsed());

        // The editor offers "restart"; taking it relaunches from the same snapshot.
        let t = Instant::now();
        let mut restarted = ctx.host.launch(&Launch::standalone())?;
        let first = restarted.start(&snap, false)?;
        let restart_ms = ms(t.elapsed());
        restarted.stop()?;

        println!(
            "  crash {:<16} → {} ({}) detected {:.1} ms, reported {:.1} ms, restart→tick1 {:.1} ms",
            format!("{kind:?}"),
            fault.code,
            fault.exit_code.as_deref().unwrap_or("killed"),
            detected,
            reported,
            restart_ms
        );
        results.push(json!({
            "kind": kind,
            "fault": fault,
            "detectedMs": detected,
            "reportedMs": reported,
            "restartToFirstTickMs": restart_ms,
            "restartedAtTick": first.tick,
        }));
    }
    let edit_after = state_hash(&mut ctx.edit);
    Ok(json!({
        "hostSurvived": true,
        "editWorldUnchanged": edit_before == edit_after,
        "cases": results,
    }))
}

/// Starts one server and two clients; clients complete a UDP hello/welcome with the server
/// before answering `play.start`. Returns the time from `t` to all three answering.
fn start_session(
    ctx: &mut Ctx,
    mut players: Vec<Player>,
    t: Instant,
) -> Result<(f64, Vec<Player>, Vec<StartResult>)> {
    let snap = Prepared::encode(&mut ctx.edit, Transport::Shm, &mut ctx.host)?;
    players.sort_by_key(|p| p.hello.role != Role::Server);
    let server_addr = players[0]
        .hello
        .net_addr
        .clone()
        .context("server address")?;
    let mut ids = vec![];
    for p in &mut players {
        let connect_to = (p.hello.role == Role::Client).then(|| server_addr.clone());
        ids.push(p.send_start(&snap, false, connect_to)?);
    }
    let mut results = vec![];
    for (p, id) in players.iter_mut().zip(ids) {
        results.push(p.wait::<Start>(id)?);
    }
    Ok((ms(t.elapsed()), players, results))
}

/// Waits for the server's heartbeat to report both clients.
fn server_clients(server: &mut Player) -> Result<u32> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        server.call::<Capture>(&Empty {})?;
        if let Some(status) = &server.last_status
            && status.clients >= 2
        {
            return Ok(status.clients);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(server.last_status.as_ref().map_or(0, |s| s.clients))
}

pub fn multi_suite(ctx: &mut Ctx) -> Result<Value> {
    let roles = [Role::Server, Role::Client, Role::Client];
    let launch = |role| Launch {
        role,
        ..Launch::standalone()
    };
    let (mut cold, mut warm, mut handshakes, mut all_seen) = (vec![], vec![], vec![], true);
    for _ in 0..ctx.reps.min(10) {
        let t = Instant::now();
        let spawned = roles
            .iter()
            .map(|&r| ctx.host.spawn(&launch(r)))
            .collect::<Result<Vec<_>>>()?;
        let players = ctx.host.connect(spawned)?;
        let (total, mut players, results) = start_session(ctx, players, t)?;
        cold.push(total);
        handshakes.extend(
            results
                .iter()
                .filter_map(|r| r.net_handshake_us)
                .map(|us| us as f64 / 1000.0),
        );
        all_seen &= server_clients(&mut players[0])? == 2;
        for p in players {
            p.stop()?;
        }

        let spawned = roles
            .iter()
            .map(|&r| ctx.host.spawn(&launch(r)))
            .collect::<Result<Vec<_>>>()?;
        let players = ctx.host.connect(spawned)?;
        let t = Instant::now();
        let (total, mut players, _) = start_session(ctx, players, t)?;
        warm.push(total);
        all_seen &= server_clients(&mut players[0])? == 2;
        let pids: Vec<u32> = players.iter().map(Player::pid).collect();
        ensure!(pids.len() == 3, "expected three processes");
        for p in players {
            p.stop()?;
        }
    }
    println!(
        "  multi-client 1 server + 2 clients: cold {} / warm pool {}  (client UDP handshake {}), server saw both: {}",
        Stats::of(&cold).short(),
        Stats::of(&warm).short(),
        Stats::of(&handshakes).short(),
        all_seen
    );
    Ok(json!({
        "coldAllRunning": Stats::of(&cold),
        "warmPoolAllRunning": Stats::of(&warm),
        "clientHandshake": Stats::of(&handshakes),
        "serverSawBothClientsEveryRun": all_seen,
    }))
}
