//! The reload latency harness: a fixed-tick ECS loop running the hot C# module, and an "editor"
//! thread that saves the scripted edits (`edits.rs`) to a copy of `game/` that the C# file watcher
//! watches. Latency = save → end of the first tick that ran the new code.

use crate::clr::{self, HostCtx, ms};
use crate::edits::{EDITS, Edit, Expect};
use crate::sched::{self, SimTick};
use crate::schema::{self, RELOAD_ERROR, RELOAD_L1, RELOAD_L3, ReloadReport, Tick};
use crate::{arg, filler, flag, world};
use bevy_ecs::prelude::*;
use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// What the tick loop waits for before an edit counts as done.
#[derive(Clone, Copy, Debug)]
enum Wait {
    /// Code stamping this value ran.
    Stamp(u32),
    /// Code stamping this value ran and a fault was reported.
    StampAndFault(u32),
    /// Any reload report arrived (and the tick after it ran).
    Report,
    /// A compile-error report arrived.
    Error,
    /// Nothing reloads for this long.
    Quiet(Duration),
}

/// Editor → tick loop: start watching for an edit's completion. Sent *before* the file is written.
struct Arm {
    wait: Wait,
}

/// Tick loop → editor: the edit completed (or timed out).
struct Done {
    t1_ns: u64,
    reports: Vec<ReloadReport>,
    faults: usize,
    /// Checksum time spent at reload boundaries inside the window (excluded from latency).
    checksum_ns: u64,
    max_tick_ms: f64,
    timed_out: bool,
}

/// One measured edit.
struct Sample {
    round: usize,
    name: &'static str,
    expect: Expect,
    latency_ms: f64,
    done: Done,
    t0_ns: u64,
}

impl Sample {
    fn kind(&self) -> u32 {
        self.done.reports.iter().map(|r| r.kind).find(|&k| k != RELOAD_ERROR).or(self.done.reports.first().map(|r| r.kind)).unwrap_or(0)
    }
}

fn work_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("work/reload")
}

/// Pristine hand-written module sources (relative name → text).
fn pristine() -> BTreeMap<String, String> {
    std::fs::read_dir(crate::bench::game_dir())
        .expect("read game/")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "cs"))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).expect("read source")))
        .collect()
}

/// Applies `edit` (with version `v`) to `files`. Returns the new text and what to wait for.
fn apply(files: &BTreeMap<String, String>, edit: &Edit, v: u32) -> Result<(String, Wait), String> {
    let mut text = files.get(edit.file).cloned().unwrap_or_default();
    for (find, replace) in edit.steps {
        let replace = replace.replace("{v}", &v.to_string());
        if find.is_empty() {
            text = replace;
            continue;
        }
        let n = text.matches(find).count();
        if n != 1 {
            return Err(format!("edit '{}': `{}` found {n} times in {}", edit.name, find.escape_debug(), edit.file));
        }
        text = text.replacen(find, &replace, 1);
    }
    let stamp_value = |site: &str, text: &str| -> Option<u32> {
        let tag = format!("); // @stamp:{site}");
        let line = text.lines().find(|l| l.contains(&tag))?;
        let start = line.find("Probe.Stamp(")? + "Probe.Stamp(".len();
        line[start..line.find(&tag)?].parse().ok()
    };
    let wait = match (edit.expect, edit.site) {
        (Expect::Noop, _) => Wait::Quiet(Duration::from_millis(600)),
        (Expect::Error, _) => Wait::Error,
        (_, None) => Wait::Report,
        (expect, Some(site)) => {
            let tag = format!("); // @stamp:{site}");
            let value = if let Some(line) = text.lines().find(|l| l.contains(&tag)) {
                // The site is in this file: bump it to this edit's version.
                let old = line.to_string();
                let indent = &old[..old.len() - old.trim_start().len()];
                text = text.replacen(&old, &format!("{indent}Probe.Stamp({v}); // @stamp:{site}"), 1);
                v
            } else {
                // The site lives in another file (e.g. registering a system): wait for its current value.
                files.values().find_map(|t| stamp_value(site, t)).ok_or(format!("edit '{}': no stamp site {site}", edit.name))?
            };
            if expect == Expect::Fault { Wait::StampAndFault(value) } else { Wait::Stamp(value) }
        }
    };
    Ok((text, wait))
}

fn version(round: usize, i: usize) -> u32 {
    u32::try_from(round * 100 + i + 1).expect("version fits")
}

/// Dry-runs every edit so a broken script fails before anything is measured.
fn validate(rounds: usize) {
    for round in 0..rounds {
        let mut files = pristine();
        for (i, edit) in EDITS.iter().enumerate() {
            match apply(&files, edit, version(round, i)) {
                Ok((text, _)) => {
                    files.insert(edit.file.to_string(), text);
                }
                Err(e) => panic!("{e}"),
            }
        }
    }
}

/// Nearest-rank percentile of `xs` (sorted copy).
fn pct(xs: &[f64], p: f64) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    v[((p * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1]
}

pub fn run(args: &[String]) {
    let rounds: usize = arg(args, "--rounds", 3);
    let tick_hz: f64 = arg(args, "--tick-hz", 60.0);
    let threads: usize = arg(args, "--threads", 8);
    let prewarm = !flag(args, "--no-prewarm");
    let verbose = flag(args, "--verbose");
    validate(rounds);

    // Work copy of the module: hand-written sources + generated filler.
    let dir = work_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clear work dir");
    }
    std::fs::create_dir_all(&dir).expect("create work dir");
    let originals = pristine();
    for (name, text) in &originals {
        std::fs::write(dir.join(name), text).expect("copy source");
    }
    let hand_lines: usize = originals.values().map(|t| t.lines().count()).sum();
    let filler_lines = filler::write(&dir);

    let ctx = HostCtx::new(true);
    let clr = clr::start(!flag(args, "--no-enc"), ctx.clone());
    let load_ms = clr.load_module(&dir, schema::LOAD_DEBUG_IL | schema::LOAD_WATCH);
    let t = Instant::now();
    if prewarm {
        // SAFETY: plain call into the C# runtime.
        assert_eq!(unsafe { (clr.api.prewarm)() }, 0, "prewarm failed");
    }
    let prewarm_ms = ms(t);
    ctx.verbose.store(verbose, std::sync::atomic::Ordering::Relaxed);
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(threads).build());

    let mut world = World::new();
    world::spawn(&mut world);
    let dt = (1.0 / tick_hz) as f32;
    let mut tick = Tick { frame: 0, dt, elapsed: 0.0 };
    world.insert_resource(SimTick(tick));
    let api = clr.api;
    let mut systems = sched::read_systems(&api);
    let mut schedule = sched::build_schedule(api, &systems, None);
    let t = Instant::now();
    schedule.run(&mut world);
    let first_tick_ms = ms(t);
    println!("module: {} hand-written + {} filler files, {} + {} = {} lines (Debug IL, hot)", originals.len(), filler::FILES, hand_lines, filler_lines, hand_lines + filler_lines);
    println!(
        "cold start: runtime init {:.1} ms, script init {:.1} ms, first compile+load {:.0} ms, prewarm {:.0} ms, first tick {:.1} ms (JIT + task pool)",
        clr.runtime_init_ms, clr.script_init_ms, load_ms, prewarm_ms, first_tick_ms
    );
    println!("systems: {}", systems.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "));
    println!("{rounds} rounds x {} edits, tick {tick_hz} Hz, task pool {threads} threads\n", EDITS.len());

    let gc0 = (
        clr.diag(schema::DIAG_GC_PAUSE_US),
        clr.diag(schema::DIAG_GC_GEN0),
        clr.diag(schema::DIAG_GC_GEN1),
        clr.diag(schema::DIAG_GC_GEN2),
    );
    let (arm_tx, arm_rx) = mpsc::channel::<Arm>();
    let (done_tx, done_rx) = mpsc::channel::<Done>();
    let editor = {
        let ctx = ctx.clone();
        let dir = dir.clone();
        std::thread::Builder::new()
            .name("editor".into())
            .spawn(move || editor(rounds, &dir, &ctx, &arm_tx, &done_rx))
            .expect("spawn editor")
    };

    // The tick loop.
    let period = Duration::from_secs_f64(1.0 / tick_hz);
    let mut next = Instant::now();
    let mut inflight: Option<(Wait, usize, usize, u64, Instant, u64, f64)> = None;
    let (mut state_checks, mut state_mismatches) = (0usize, 0usize);
    let mut tick_ms: Vec<f64> = Vec::new();
    let mut reload_tick_ms: Vec<f64> = Vec::new();
    let mut rebuilds = 0usize;
    while !editor.is_finished() {
        if let Ok(arm) = arm_rx.try_recv() {
            let r = ctx.reports.lock().expect("reports").len();
            let f = ctx.faults.lock().expect("faults").len();
            inflight = Some((arm.wait, r, f, 0, Instant::now(), 0, 0.0));
        }
        let t_tick = Instant::now();
        // SAFETY: plain call into the C# runtime.
        let pending = unsafe { (api.has_pending)() } != 0;
        let mut ck_ns = 0;
        let before = if pending {
            let t = Instant::now();
            let c = world::checksum_fast(&mut world);
            ck_ns += t.elapsed().as_nanos() as u64;
            Some(c)
        } else {
            None
        };
        // SAFETY: called between ticks on the tick thread, as the ABI requires.
        let flags = unsafe { (api.tick_boundary)(&tick) };
        if flags & schema::BOUNDARY_RELOADED != 0 {
            let t = Instant::now();
            let after = world::checksum_fast(&mut world);
            ck_ns += t.elapsed().as_nanos() as u64;
            state_checks += 1;
            if before != Some(after) {
                state_mismatches += 1;
                eprintln!("STATE MISMATCH across reload at tick {}", tick.frame);
            }
        }
        if flags & schema::BOUNDARY_SYSTEMS_CHANGED != 0 {
            systems = sched::read_systems(&api);
            schedule = sched::build_schedule(api, &systems, None);
            rebuilds += 1;
        }
        world.resource_mut::<SimTick>().0 = tick;
        let t_run = Instant::now();
        schedule.run(&mut world);
        let run_ms = ms(t_run);
        tick_ms.push(run_ms);
        let _ = t_tick;

        if let Some((wait, r0, f0, ck, armed, _, max_tick)) = inflight.as_mut() {
            *ck += ck_ns;
            *max_tick = max_tick.max(run_ms);
            reload_tick_ms.push(run_ms);
            let reports = ctx.reports.lock().expect("reports")[*r0..].to_vec();
            let faults = ctx.faults.lock().expect("faults").len() - *f0;
            let done = match *wait {
                Wait::Stamp(v) => ctx.seen(v),
                Wait::StampAndFault(v) => ctx.seen(v) && faults > 0,
                Wait::Report => !reports.is_empty(),
                Wait::Error => reports.iter().any(|r| r.kind == RELOAD_ERROR),
                Wait::Quiet(d) => armed.elapsed() >= d,
            };
            let timed_out = armed.elapsed() > Duration::from_secs(8);
            if done || timed_out {
                let msg = Done { t1_ns: ctx.now_ns(), reports, faults, checksum_ns: *ck, max_tick_ms: *max_tick, timed_out: timed_out && !done };
                let _ = done_tx.send(msg);
                inflight = None;
            }
        }

        tick.frame += 1;
        tick.elapsed = tick.frame as f32 * dt;
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now; // overran: don't try to catch up
        }
    }
    let samples = editor.join().expect("editor thread");

    summarize(&samples, &tick_ms, &reload_tick_ms);
    println!("\nstate: {state_checks} reload boundaries checksummed, {state_mismatches} mismatches; schedule rebuilt {rebuilds}x; {} ticks run", tick.frame);
    let faults = ctx.faults.lock().expect("faults").len();
    println!(
        "faults contained: {faults} (host kept ticking); ALCs created {}, alive before shutdown {}; managed heap {:.0} MB, working set {:.0} MB",
        clr.diag(schema::DIAG_ALCS_CREATED),
        clr.diag(schema::DIAG_ALCS_ALIVE),
        clr.diag(schema::DIAG_MANAGED_HEAP) as f64 / 1e6,
        clr.diag(schema::DIAG_WORKING_SET) as f64 / 1e6
    );
    println!(
        "GC during session: {:.1} ms total pause; gen0 {} gen1 {} gen2 {} collections",
        (clr.diag(schema::DIAG_GC_PAUSE_US) - gc0.0) as f64 / 1e3,
        clr.diag(schema::DIAG_GC_GEN0) - gc0.1,
        clr.diag(schema::DIAG_GC_GEN1) - gc0.2,
        clr.diag(schema::DIAG_GC_GEN2) - gc0.3
    );
    ctx.verbose.store(true, std::sync::atomic::Ordering::Relaxed);
    let leaked = clr.shutdown();
    println!("shutdown: {leaked} ALCs still alive after forced GC");
}

/// The editor thread: saves each edit, waits for the tick loop to see it, then pauses.
fn editor(rounds: usize, dir: &Path, ctx: &Arc<HostCtx>, arm: &Sender<Arm>, done: &Receiver<Done>) -> Vec<Sample> {
    let mut samples = Vec::new();
    let mut rng: u32 = 0x9e37_79b9;
    let mut pause = || {
        rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        Duration::from_millis(150 + u64::from(rng >> 24))
    };
    let originals = pristine();
    std::thread::sleep(Duration::from_millis(500));
    for round in 0..rounds {
        let mut files = originals.clone();
        if round > 0 {
            restore(dir, &originals, &mut files_on_disk(dir, &originals), arm, done);
        }
        for (i, edit) in EDITS.iter().enumerate() {
            let (text, wait) = apply(&files, edit, version(round, i)).expect("validated");
            arm.send(Arm { wait }).expect("tick loop alive");
            std::fs::write(dir.join(edit.file), &text).expect("save edit");
            let t0_ns = ctx.now_ns();
            files.insert(edit.file.to_string(), text);
            let d = done.recv().expect("tick loop alive");
            let latency_ms = (d.t1_ns - t0_ns - d.checksum_ns) as f64 / 1e6;
            let s = Sample { round, name: edit.name, expect: edit.expect, latency_ms, done: d, t0_ns };
            print_sample(i, &s);
            samples.push(s);
            std::thread::sleep(pause());
        }
    }
    samples
}

/// Current on-disk texts of the hand-written files (plus any created by edits).
fn files_on_disk(dir: &Path, originals: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for e in EDITS.iter().map(|e| e.file).chain(originals.keys().map(String::as_str)) {
        if let Ok(t) = std::fs::read_to_string(dir.join(e)) {
            m.insert(e.to_string(), t);
        }
    }
    m
}

/// Restores the pristine module between rounds, one file at a time in an order that always compiles
/// (registrations first, the added file last). Not measured.
fn restore(dir: &Path, originals: &BTreeMap<String, String>, disk: &mut BTreeMap<String, String>, arm: &Sender<Arm>, done: &Receiver<Done>) {
    let mut order: Vec<&str> = vec!["ModuleEntry.cs", "Visuals.cs", "Vitals.cs", "Forces.cs", "Steering.cs", "Movement.cs", "Tuning.cs"];
    order.retain(|f| originals.contains_key(*f));
    for f in order {
        if disk.get(f) == originals.get(f) {
            continue;
        }
        arm.send(Arm { wait: Wait::Report }).expect("tick loop alive");
        std::fs::write(dir.join(f), &originals[f]).expect("restore");
        let d = done.recv().expect("tick loop alive");
        assert!(!d.timed_out && d.reports.iter().all(|r| r.kind != RELOAD_ERROR), "restore of {f} did not reload cleanly");
    }
    for f in disk.keys().filter(|f| !originals.contains_key(*f)) {
        arm.send(Arm { wait: Wait::Report }).expect("tick loop alive");
        std::fs::remove_file(dir.join(f)).expect("remove added file");
        let d = done.recv().expect("tick loop alive");
        assert!(!d.timed_out, "removing {f} did not reload");
    }
    std::thread::sleep(Duration::from_millis(300));
}

fn kind_name(k: u32) -> &'static str {
    match k {
        RELOAD_L1 => "L1",
        RELOAD_L3 => "L3",
        RELOAD_ERROR => "error",
        0 => "none",
        _ => "?",
    }
}

fn print_sample(i: usize, s: &Sample) {
    let r = s.done.reports.iter().find(|r| r.kind != RELOAD_ERROR).or(s.done.reports.first());
    let phases = r
        .map(|r| {
            let d = |a: u64, b: u64| (b.saturating_sub(a)) as f64 / 1e6;
            format!(
                "watch {:>5.1} parse {:>4.1} classify {:>4.1} emit {:>6.1} stage {:>5.1} wait {:>5.1} tick {:>5.1}",
                d(s.t0_ns, r.t_event),
                d(r.t_event, r.t_parsed),
                d(r.t_parsed, r.t_classified),
                d(r.t_classified, r.t_emitted),
                d(r.t_emitted, r.t_staged),
                d(r.t_staged, r.t_applied),
                d(r.t_applied, s.done.t1_ns)
            )
        })
        .unwrap_or_default();
    let got = kind_name(s.kind());
    let mark = match (s.expect, got) {
        (Expect::L1 | Expect::Fault, "L1") | (Expect::L3, "L3") | (Expect::Error, "error") | (Expect::Noop, "none") => "",
        _ => " <-- MISMATCH",
    };
    let fault = if s.done.faults > 0 { format!(" faults+{}", s.done.faults) } else { String::new() };
    let to = if s.done.timed_out { " TIMEOUT" } else { "" };
    println!(
        "r{} #{i:<2} {:<44} {:?}->{got:<5} {:>7.1} ms  [{phases}] maxtick {:.1}{fault}{to}{mark}",
        s.round,
        s.name,
        s.expect,
        s.latency_ms,
        s.done.max_tick_ms
    );
}

fn summarize(samples: &[Sample], tick_ms: &[f64], reload_tick_ms: &[f64]) {
    println!("\n=== summary (latency = save -> end of first tick running new code; ms) ===");
    for (label, kind) in [("L1", RELOAD_L1), ("L3", RELOAD_L3), ("error", RELOAD_ERROR)] {
        let xs: Vec<f64> = samples.iter().filter(|s| s.kind() == kind && !s.done.timed_out).map(|s| s.latency_ms).collect();
        if xs.is_empty() {
            continue;
        }
        let first = samples.iter().find(|s| s.kind() == kind).map(|s| s.latency_ms).unwrap_or(f64::NAN);
        println!(
            "{label:<5} n={:<3} p50 {:>6.1}  p95 {:>6.1}  max {:>6.1}  min {:>6.1}  (first of kind {:.1})",
            xs.len(),
            pct(&xs, 0.5),
            pct(&xs, 0.95),
            pct(&xs, 1.0),
            pct(&xs, 0.0),
            first
        );
        // Phase medians.
        let phase = |f: &dyn Fn(&Sample, &ReloadReport) -> f64| -> f64 {
            let v: Vec<f64> = samples
                .iter()
                .filter(|s| s.kind() == kind)
                .filter_map(|s| s.done.reports.iter().find(|r| r.kind == kind).map(|r| f(s, r)))
                .collect();
            pct(&v, 0.5)
        };
        let d = |a: u64, b: u64| (b.saturating_sub(a)) as f64 / 1e6;
        println!(
            "      medians: watcher {:.1}, parse {:.1}, classify {:.1}, emit {:.1}, stage(load/bind) {:.1}, wait-for-boundary {:.1}, tick {:.1}; emit bytes {:.0} KB",
            phase(&|s, r| d(s.t0_ns, r.t_event)),
            phase(&|_, r| d(r.t_event, r.t_parsed)),
            phase(&|_, r| d(r.t_parsed, r.t_classified)),
            phase(&|_, r| d(r.t_classified, r.t_emitted)),
            phase(&|_, r| d(r.t_emitted, r.t_staged)),
            phase(&|_, r| d(r.t_staged, r.t_applied)),
            phase(&|s, r| d(r.t_applied, s.done.t1_ns)),
            phase(&|_, r| r.emit_bytes as f64 / 1e3),
        );
    }
    let mismatches = samples.iter().filter(|s| {
        let got = s.kind();
        !matches!((s.expect, got), (Expect::L1 | Expect::Fault, RELOAD_L1) | (Expect::L3, RELOAD_L3) | (Expect::Error, RELOAD_ERROR) | (Expect::Noop, 0))
    });
    let mm: Vec<_> = mismatches.map(|s| format!("{} (r{})", s.name, s.round)).collect();
    println!("classification: {} / {} as expected{}", samples.len() - mm.len(), samples.len(), if mm.is_empty() { String::new() } else { format!("; mismatches: {}", mm.join(", ")) });
    let mut esc: BTreeMap<u32, usize> = BTreeMap::new();
    for s in samples {
        for r in &s.done.reports {
            if r.kind == RELOAD_L3 {
                *esc.entry(r.escalation).or_default() += 1;
            }
        }
    }
    println!("L3 escalation reasons (1 file-set, 2 structural, 3 state machine, 4 emit failed, 7 closure): {esc:?}");
    let timeouts = samples.iter().filter(|s| s.done.timed_out).count();
    println!("timeouts: {timeouts}");
    println!(
        "tick (schedule.run) ms: all p50 {:.2} p99 {:.2} max {:.2}; during reload windows p50 {:.2} p99 {:.2} max {:.2}",
        pct(tick_ms, 0.5),
        pct(tick_ms, 0.99),
        pct(tick_ms, 1.0),
        pct(reload_tick_ms, 0.5),
        pct(reload_tick_ms, 0.99),
        pct(reload_tick_ms, 1.0)
    );
}
