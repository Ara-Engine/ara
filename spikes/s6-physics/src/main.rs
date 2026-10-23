//! S6 physics spike: Jolt (Ara's own C shim) vs Rapier 0.36 on identical scenarios.
//!
//! Usage: `s6 <scenario> [--engine jolt|rapier|both] [--threads N] [--count N] [--steps N] [--out DIR]`
//! Scenarios: character, stacking, vehicle, scale, determinism, queries, all, det-child.
//! Results print as JSON and are written to `<out>/<flavor>/<scenario>-<engine>[-...].json`.

mod common;
mod engine;
mod jolt;
mod rapier;
mod scenarios;
mod scene;

use common::{EngineKind, Flavor, ms_since};
use engine::Engine;
use jolt::JoltEngine;
use rapier::RapierEngine;
use scenarios::RunHashes;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

struct Args {
    scenario: String,
    engines: Vec<EngineKind>,
    threads: Vec<usize>,
    counts: Vec<usize>,
    steps: usize,
    ticks: u32,
    out: PathBuf,
}

fn parse_list<T: std::str::FromStr>(s: &str) -> Vec<T> {
    s.split(',').filter_map(|x| x.trim().parse().ok()).collect()
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let scenario = it.next().unwrap_or_else(|| "all".into());
    let mut a = Args {
        scenario,
        engines: vec![EngineKind::Jolt, EngineKind::Rapier],
        threads: vec![1],
        counts: vec![5000, 20000],
        steps: 600,
        ticks: 300,
        out: PathBuf::from("evidence"),
    };
    while let Some(flag) = it.next() {
        let val = it.next().unwrap_or_default();
        match flag.as_str() {
            "--engine" => {
                a.engines = if val == "both" {
                    vec![EngineKind::Jolt, EngineKind::Rapier]
                } else {
                    EngineKind::parse(&val).into_iter().collect()
                }
            }
            "--threads" => a.threads = parse_list(&val),
            "--count" => a.counts = parse_list(&val),
            "--steps" => a.steps = val.parse().unwrap_or(600),
            "--ticks" => a.ticks = val.parse().unwrap_or(300),
            "--out" => a.out = PathBuf::from(val),
            other => panic!("unknown flag {other}"),
        }
    }
    a
}

#[derive(Serialize)]
struct Envelope<'a, T: Serialize> {
    scenario: &'a str,
    flavor: Flavor,
    result: T,
}

fn emit<T: Serialize>(args: &Args, scenario: &str, name: &str, result: T) {
    let env = Envelope {
        scenario,
        flavor: Flavor::current(),
        result,
    };
    let json = serde_json::to_string_pretty(&env).expect("serializable");
    let dir = args.out.join(Flavor::current().tag());
    std::fs::create_dir_all(&dir).expect("create evidence dir");
    std::fs::write(dir.join(format!("{name}.json")), &json).expect("write evidence");
    println!("{json}");
}

/// Runs `f` with the concrete engine type for `kind`.
macro_rules! with_engine {
    ($kind:expr, $E:ident => $body:expr) => {
        match $kind {
            EngineKind::Jolt => {
                type $E = JoltEngine;
                $body
            }
            EngineKind::Rapier => {
                type $E = RapierEngine;
                $body
            }
        }
    };
}

#[derive(Serialize)]
struct SameProcess {
    threads: usize,
    run_a: RunHashes,
    run_b: RunHashes,
    identical: bool,
    first_divergent_tick: Option<usize>,
}

#[derive(Serialize)]
struct CrossProcess {
    threads: usize,
    child_a: RunHashes,
    child_b: RunHashes,
    children_identical: bool,
    identical_to_in_process: bool,
}

#[derive(Serialize)]
struct DetReport {
    engine: &'static str,
    ticks: u32,
    /// Negative control: one body's start x moved by one float step must change the hash.
    perturbed_final_hash: String,
    perturbation_detected: bool,
    same_process: Vec<SameProcess>,
    thread_count_invariant: bool,
    cross_process: Vec<CrossProcess>,
    snapshot_restore: Vec<(usize, scenarios::SnapshotResult)>,
}

fn child_run(kind: EngineKind, threads: usize, ticks: u32) -> RunHashes {
    let exe = std::env::current_exe().expect("current exe");
    let engine = match kind {
        EngineKind::Jolt => "jolt",
        EngineKind::Rapier => "rapier",
    };
    let out = Command::new(exe)
        .args([
            "det-child",
            "--engine",
            engine,
            "--threads",
            &threads.to_string(),
            "--ticks",
            &ticks.to_string(),
        ])
        .output()
        .expect("spawn det-child");
    assert!(
        out.status.success(),
        "det-child failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("det-child JSON")
}

fn determinism<E: Engine>(kind: EngineKind, threads: &[usize], ticks: u32) -> DetReport {
    let mut same = Vec::new();
    let mut cross = Vec::new();
    let mut snaps = Vec::new();
    for &t in threads {
        let (a, all_a) = scenarios::det_run::<E>(t, ticks, false);
        let (b, all_b) = scenarios::det_run::<E>(t, ticks, false);
        let first_div = all_a.iter().zip(&all_b).position(|(x, y)| x != y);
        let child_a = child_run(kind, t, ticks);
        let child_b = child_run(kind, t, ticks);
        cross.push(CrossProcess {
            threads: t,
            children_identical: child_a == child_b,
            identical_to_in_process: child_a == a,
            child_a,
            child_b,
        });
        snaps.push((t, scenarios::snapshot_restore::<E>(t, all_a[199])));
        same.push(SameProcess {
            threads: t,
            identical: a == b,
            first_divergent_tick: first_div,
            run_a: a,
            run_b: b,
        });
    }
    let invariant = same.windows(2).all(|w| w[0].run_a == w[1].run_a);
    let perturbed = scenarios::det_run::<E>(threads[0], ticks, true).0;
    DetReport {
        engine: E::NAME,
        ticks,
        perturbation_detected: perturbed.final_hash != same[0].run_a.final_hash,
        perturbed_final_hash: perturbed.final_hash,
        same_process: same,
        thread_count_invariant: invariant,
        cross_process: cross,
        snapshot_restore: snaps,
    }
}

#[derive(Serialize)]
struct FfiReport {
    noop_call_ns: f64,
    queries: scenarios::QueryReport,
}

fn jolt_queries() -> FfiReport {
    let calls = 20_000_000u32;
    let j = JoltEngine::new(engine::WorldConfig {
        max_bodies: 16,
        threads: 1,
    });
    let t = Instant::now();
    let mut x = 0u32;
    for _ in 0..calls {
        x = j.noop(std::hint::black_box(x));
    }
    std::hint::black_box(x);
    let noop = ms_since(t) * 1e6 / f64::from(calls);
    let queries = scenarios::queries::<JoltEngine>(|e, ids| {
        let mut raw = vec![ara_jolt_sys::AraBodyState::default(); ids.len()];
        let rounds = 200;
        let t = Instant::now();
        for _ in 0..rounds {
            e.read_raw(ids, &mut raw);
            std::hint::black_box(&raw);
        }
        ms_since(t) * 1e6 / (rounds * ids.len()) as f64
    });
    FfiReport {
        noop_call_ns: noop,
        queries,
    }
}

fn rapier_queries() -> scenarios::QueryReport {
    scenarios::queries::<RapierEngine>(|e, ids| {
        let mut out = vec![engine::BodyState::default(); ids.len()];
        let rounds = 200;
        let t = Instant::now();
        for _ in 0..rounds {
            e.read_into(ids, &mut out);
            std::hint::black_box(&out);
        }
        ms_since(t) * 1e6 / (rounds * ids.len()) as f64
    })
}

fn run(args: &Args, scenario: &str) {
    for &kind in &args.engines {
        let name = match kind {
            EngineKind::Jolt => "jolt",
            EngineKind::Rapier => "rapier",
        };
        match scenario {
            "character" => {
                with_engine!(kind, E => emit(args, scenario, &format!("character-{name}"), scenarios::character::<E>()))
            }
            "stacking" => {
                with_engine!(kind, E => emit(args, scenario, &format!("stacking-{name}"), scenarios::stacking::<E>(None)));
                // Jolt's 20-box tower needs 15 velocity steps (default 10) to stand; record both.
                if kind == EngineKind::Jolt {
                    emit(
                        args,
                        scenario,
                        "stacking-jolt-v15",
                        scenarios::stacking::<JoltEngine>(Some(15)),
                    );
                }
            }
            "vehicle" => {
                with_engine!(kind, E => emit(args, scenario, &format!("vehicle-{name}"), scenarios::vehicle::<E>()))
            }
            "scale" => {
                for &count in &args.counts {
                    for &t in &args.threads {
                        with_engine!(kind, E => emit(
                            args,
                            scenario,
                            &format!("scale-{name}-{count}-t{t}"),
                            scenarios::scale::<E>(count, t, args.steps),
                        ));
                    }
                }
            }
            "determinism" => {
                with_engine!(kind, E => emit(args, scenario, &format!("determinism-{name}"), determinism::<E>(kind, &args.threads, args.ticks)))
            }
            "queries" => match kind {
                EngineKind::Jolt => emit(args, scenario, "queries-jolt", jolt_queries()),
                EngineKind::Rapier => emit(args, scenario, "queries-rapier", rapier_queries()),
            },
            other => panic!("unknown scenario {other}"),
        }
    }
}

fn main() {
    let args = parse_args();
    match args.scenario.as_str() {
        "det-child" => {
            let kind = args.engines[0];
            let t = args.threads[0];
            let hashes = with_engine!(kind, E => scenarios::det_run::<E>(t, args.ticks, false).0);
            println!("{}", serde_json::to_string(&hashes).expect("serializable"));
        }
        "all" => {
            for s in [
                "character",
                "stacking",
                "vehicle",
                "queries",
                "determinism",
                "scale",
            ] {
                run(&args, s);
            }
        }
        s => run(&args, s),
    }
}
