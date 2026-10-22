//! Criterion 3: C# chunk loop vs the same loop in Rust, ns/entity, single-threaded; plus the same
//! systems on Bevy's compute task pool to prove C# runs from task-pool threads.

use crate::clr::{self, HostCtx};
use crate::sched::{self, SimTick, ThreadTally, chunk_for, push_column};
use crate::schema::{self, ScriptApi, Tick, Transform, Velocity};
use crate::world::{self, ENTITIES};
use crate::{arg, flag};
use bevy_ecs::change_detection::ContiguousMut;
use bevy_ecs::prelude::*;
use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The pristine gameplay module (the reload harness edits a copy, never this).
pub fn game_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("game")
}

const TICK: Tick = Tick { frame: 0, dt: 1.0 / 60.0, elapsed: 0.0 };

/// Times `frames` calls of `f`, `samples` times; returns (median, min) ns per entity.
fn measure(samples: usize, frames: usize, mut f: impl FnMut()) -> (f64, f64) {
    let mut xs: Vec<f64> = (0..samples)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..frames {
                f();
            }
            t.elapsed().as_secs_f64() * 1e9 / (frames * ENTITIES) as f64
        })
        .collect();
    xs.sort_by(f64::total_cmp);
    (xs[xs.len() / 2], xs[0])
}

/// Runs `f` until tiered JIT has promoted the C# loop to tier 1 (call counting + 100 ms delay).
fn warm(mut f: impl FnMut()) {
    for _ in 0..200 {
        f();
    }
    std::thread::sleep(Duration::from_millis(500));
    for _ in 0..200 {
        f();
    }
}

fn find(systems: &[sched::SystemEntry], name: &str) -> u32 {
    systems.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("system {name} not registered")).id
}

/// Single-threaded `integrate` over every chunk: C# when `cs` is Some(id), else the Rust port.
fn integrate_frame(world: &mut World, state: &mut QueryState<(Entity, &mut Transform, &Velocity)>, api: &ScriptApi, cs: Option<u32>, mark: bool) {
    let mut q = state.query_mut(world);
    for (ents, tr, vel) in q.contiguous_iter_mut().expect("dense query") {
        let (tr, _) = if mark { ContiguousMut::split(tr) } else { ContiguousMut::bypass_change_detection_split(tr) };
        match cs {
            Some(id) => {
                let mut chunk = chunk_for(ents);
                push_column(&mut chunk, tr.as_mut_ptr().cast());
                push_column(&mut chunk, vel.as_ptr().cast_mut().cast());
                // SAFETY: column pointers are valid for `len` rows for this call; see CsSystem::run.
                unsafe { (api.run_system)(id, &chunk, &TICK) };
            }
            None => world::integrate(tr, vel, TICK.dt),
        }
    }
}

/// Single-threaded `gravity` over every chunk (lighter, memory-bound loop).
fn gravity_frame(world: &mut World, state: &mut QueryState<(Entity, &mut Velocity, &Transform)>, api: &ScriptApi, cs: Option<u32>, mark: bool) {
    let mut q = state.query_mut(world);
    for (ents, vel, tr) in q.contiguous_iter_mut().expect("dense query") {
        let (vel, _) = if mark { ContiguousMut::split(vel) } else { ContiguousMut::bypass_change_detection_split(vel) };
        match cs {
            Some(id) => {
                let mut chunk = chunk_for(ents);
                push_column(&mut chunk, vel.as_mut_ptr().cast());
                push_column(&mut chunk, tr.as_ptr().cast_mut().cast());
                // SAFETY: column pointers are valid for `len` rows for this call; see CsSystem::run.
                unsafe { (api.run_system)(id, &chunk, &TICK) };
            }
            None => world::gravity(vel, tr, TICK.dt),
        }
    }
}

/// Rust twin of the C# `integrate` wrapper that `schema::add_shape_system` builds (same par iter, same change ticks).
fn rust_integrate_par(mut q: Query<(Entity, &mut Transform, &Velocity)>, tick: Res<SimTick>) {
    let dt = tick.0.dt;
    q.contiguous_par_iter_mut().expect("dense query").for_each(|(_, tr, vel)| {
        world::integrate(ContiguousMut::split(tr).0, vel, dt);
    });
}

pub fn run(args: &[String]) {
    let debug_il = flag(args, "--debug-il");
    let enc = !flag(args, "--no-enc");
    let frames: usize = arg(args, "--frames", 200);
    let threads: usize = arg(args, "--threads", 8);
    let samples = 7;

    let ctx = HostCtx::new(flag(args, "--verbose"));
    let clr = clr::start(enc, ctx.clone());
    let load_ms = clr.load_module(&game_dir(), if debug_il { schema::LOAD_DEBUG_IL } else { 0 });
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(threads).build());
    println!(
        "runtime init {:.1} ms, script init {:.1} ms, module compile+load {:.0} ms ({} IL, DOTNET_MODIFIABLE_ASSEMBLIES={})",
        clr.runtime_init_ms,
        clr.script_init_ms,
        load_ms,
        if debug_il { "Debug" } else { "Release" },
        if enc { "debug" } else { "unset" }
    );

    let mut world = World::new();
    world::spawn(&mut world);
    world.insert_resource(SimTick(TICK));
    let systems = sched::read_systems(&clr.api);
    let api = clr.api;
    let (integrate, gravity) = (find(&systems, "integrate"), find(&systems, "gravity"));
    let mut iq = world.query::<(Entity, &mut Transform, &Velocity)>();
    let mut gq = world.query::<(Entity, &mut Velocity, &Transform)>();
    let chunks = iq.query_mut(&mut world).contiguous_iter_mut().expect("dense").count();
    println!("{ENTITIES} entities in {chunks} chunks (archetype tables); {samples} samples x {frames} frames; median (min) ns/entity\n");

    println!("single-threaded         | C#             | Rust           | C#/Rust");
    for mark in [false, true] {
        let label = if mark { "+change ticks" } else { "raw loop" };
        warm(|| integrate_frame(&mut world, &mut iq, &api, Some(integrate), mark));
        let cs = measure(samples, frames, || integrate_frame(&mut world, &mut iq, &api, Some(integrate), mark));
        let rs = measure(samples, frames, || integrate_frame(&mut world, &mut iq, &api, None, mark));
        println!("integrate {label:<14}| {:>5.2} ({:>5.2})  | {:>5.2} ({:>5.2})  | {:.2}x", cs.0, cs.1, rs.0, rs.1, cs.0 / rs.0);
        warm(|| gravity_frame(&mut world, &mut gq, &api, Some(gravity), mark));
        let cs = measure(samples, frames, || gravity_frame(&mut world, &mut gq, &api, Some(gravity), mark));
        let rs = measure(samples, frames, || gravity_frame(&mut world, &mut gq, &api, None, mark));
        println!("gravity   {label:<14}| {:>5.2} ({:>5.2})  | {:>5.2} ({:>5.2})  | {:.2}x", cs.0, cs.1, rs.0, rs.1, cs.0 / rs.0);
    }

    // Boundary cost: run_system on an empty chunk (Rust -> C# call, dispatch, probe call back to Rust).
    let empty = chunk_for(&[]);
    let n = 2_000_000u32;
    let t = Instant::now();
    for _ in 0..n {
        // SAFETY: an empty chunk has no rows, so C# reads no column memory.
        unsafe { (api.run_system)(integrate, &empty, &TICK) };
    }
    println!("\nboundary: run_system on an empty chunk = {:.1} ns/call", t.elapsed().as_secs_f64() * 1e9 / f64::from(n));

    // Task pool: the same system as a Bevy system over contiguous_par_iter_mut chunks.
    let tally: ThreadTally = Arc::new(Mutex::new(Default::default()));
    let entry: Vec<_> = systems.iter().filter(|s| s.name == "integrate").cloned().collect();
    let mut cs_sched = sched::build_schedule(api, &entry, Some(tally.clone()));
    let mut rs_sched = Schedule::default();
    rs_sched.set_executor(bevy_ecs::schedule::MultiThreadedExecutor::new());
    rs_sched.add_systems(rust_integrate_par);
    warm(|| cs_sched.run(&mut world));
    let par_frames = frames * 2;
    tally.lock().expect("tally").clear();
    let cs = measure(samples, par_frames, || cs_sched.run(&mut world));
    let calls: u64 = tally.lock().expect("tally").values().sum();
    warm(|| rs_sched.run(&mut world));
    let rs = measure(samples, par_frames, || rs_sched.run(&mut world));
    println!(
        "\ntask pool ({threads} threads, wall time / entity): C# {:.2} ({:.2}), Rust {:.2} ({:.2}), C#/Rust {:.2}x",
        cs.0,
        cs.1,
        rs.0,
        rs.1,
        cs.0 / rs.0
    );
    let t = tally.lock().expect("tally");
    println!(
        "C# integrate ran on {} OS threads ({} chunk calls, {:.1} chunks/frame); CLR saw {} distinct threads call run_system:",
        t.len(),
        calls,
        calls as f64 / (samples * par_frames) as f64,
        clr.diag(schema::DIAG_THREADS_SEEN)
    );
    for (name, count) in t.iter() {
        println!("  {name:<28} {count}");
    }
    drop(t);
    let leaked = clr.shutdown();
    println!("\nshutdown: {leaked} leaked ALCs; checksum {:016x}", world::checksum(&mut world));
}
