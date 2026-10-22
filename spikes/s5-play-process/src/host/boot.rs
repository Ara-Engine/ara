//! Snapshot-transfer and boot-time suites: how long "press Play → first tick visible in the
//! host" takes for each transport, for in-process simulation, cold processes, fresh binaries
//! and a pre-spawned (pooled) player.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use bevy_ecs::world::World;
use serde_json::{Map, Value, json};

use super::Ctx;
use super::players::{Launch, Prepared, Transport};
use super::stats::{Stats, ms, us_ms};
use crate::model::{Sim, state_hash};
use crate::protocol::*;

use crate::snapshot;

/// Hash after `ticks` ticks of an in-process copy of the edit world: the determinism reference.
pub fn reference_hash(edit: &mut World, ticks: u32) -> Result<u64> {
    let mut sim = Sim::new(snapshot::decode(&snapshot::encode(edit))?);
    for _ in 0..ticks {
        sim.tick();
    }
    Ok(state_hash(&mut sim.world))
}

/// Per transport: size, fidelity (hashes after 0 and 120 ticks) and the warm-pool boot breakdown.
pub fn snapshot_suite(ctx: &mut Ctx) -> Result<Value> {
    let edit_hash = hash_hex(state_hash(&mut ctx.edit));
    let reference = hash_hex(reference_hash(&mut ctx.edit, 120)?);
    let mut out = Map::new();
    for transport in Transport::ALL {
        let mut p = ctx.host.launch(&Launch::standalone())?;
        let snap = Prepared::encode(&mut ctx.edit, transport, &mut ctx.host)?;
        p.start(&snap, true)?;
        let tick0 = p.call::<Capture>(&Empty {})?;
        let tick120 = p.call::<Step>(&StepParams { ticks: 120 })?;
        p.stop()?;
        let bytes = snap.len();
        drop(snap);

        let (mut prepare, mut total, mut decode, mut first, mut transfer) =
            (vec![], vec![], vec![], vec![], vec![]);
        for _ in 0..ctx.reps {
            let mut p = ctx.host.launch(&Launch::standalone())?;
            let t = Instant::now();
            let snap = Prepared::encode(&mut ctx.edit, transport, &mut ctx.host)?;
            let prepared = t.elapsed();
            let r = p.start(&snap, false)?;
            let all = ms(t.elapsed());
            p.stop()?;
            prepare.push(ms(prepared));
            total.push(all);
            decode.push(us_ms(r.decode_us));
            first.push(us_ms(r.first_tick_us));
            transfer.push(all - ms(prepared) - us_ms(r.decode_us) - us_ms(r.first_tick_us));
        }
        let total = Stats::of(&total);
        println!(
            "  snapshot {:<9} {:>10} B  play→tick1 {}  (prepare {:.2}, transfer {:.2}, decode {:.2} ms p50)  hash ok: {}",
            transport.key(),
            bytes,
            total.short(),
            Stats::of(&prepare).p50,
            Stats::of(&transfer).p50,
            Stats::of(&decode).p50,
            tick0.hash == edit_hash && tick120.hash == reference,
        );
        out.insert(
            transport.key().into(),
            json!({
                "bytes": bytes,
                "tick0Hash": tick0.hash,
                "tick0MatchesEditWorld": tick0.hash == edit_hash,
                "tick120Hash": tick120.hash,
                "tick120MatchesInProcessReference": tick120.hash == reference,
                "warmPlayToFirstTick": total,
                "hostPrepare": Stats::of(&prepare),
                "transferAndProtocol": Stats::of(&transfer),
                "playerDecode": Stats::of(&decode),
                "firstTick": Stats::of(&first),
            }),
        );
    }
    out.insert("editHash".into(), json!(edit_hash));
    out.insert("inProcessTick120Hash".into(), json!(reference));
    Ok(Value::Object(out))
}

/// One cold boot: spawn, prepare the snapshot while the process starts, connect, start.
fn cold_boot(ctx: &mut Ctx, launch: &Launch) -> Result<(f64, f64, Option<GpuTimings>)> {
    let t = Instant::now();
    let spawned = ctx.host.spawn(launch)?;
    let snap = Prepared::encode(&mut ctx.edit, Transport::Shm, &mut ctx.host)?;
    let mut p = ctx.host.connect(vec![spawned])?.remove(0);
    let to_hello = ms(p.connected_at - p.spawned_at);
    p.start(&snap, false)?;
    let total = ms(t.elapsed());
    let gpu = p.hello.gpu;
    p.stop()?;
    Ok((total, to_hello, gpu))
}

/// One warm boot: the player is already running and connected (a pool of one).
fn warm_boot(ctx: &mut Ctx, launch: &Launch, transport: Transport) -> Result<f64> {
    let mut p = ctx.host.launch(launch)?;
    // Let a GPU-initialising player finish idling into its read loop, like a real pool would.
    std::thread::sleep(Duration::from_millis(20));
    let t = Instant::now();
    let snap = Prepared::encode(&mut ctx.edit, transport, &mut ctx.host)?;
    p.start(&snap, false)?;
    let total = ms(t.elapsed());
    p.stop()?;
    Ok(total)
}

/// Copies the player binary with a unique trailer, so Windows has never seen this exact file:
/// what the first Play after a rebuild pays (image load without cache, Defender's first scan).
fn fresh_binary(exe: &Path, rep: usize) -> Result<PathBuf> {
    let dir = exe.parent().context("exe dir")?.join("s5-cold");
    std::fs::create_dir_all(&dir)?;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let path = dir.join(format!("ara-s5-play-cold-{rep}-{nanos}.exe"));
    let mut bytes = std::fs::read(exe)?;
    bytes.extend(nanos.to_le_bytes().iter().cycle().take(4096));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Memory held by one idle pool player, before it receives a snapshot.
fn idle_memory(ctx: &mut Ctx, launch: &Launch) -> Result<Value> {
    let p = ctx.host.launch(launch)?;
    std::thread::sleep(Duration::from_millis(100));
    let (working_set, private) = p.memory().unwrap_or_default();
    p.stop()?;
    println!(
        "  idle player memory{}       working set {:.1} MiB, private {:.1} MiB",
        if launch.gpu_init { " (Vulkan)" } else { "" },
        working_set as f64 / 1048576.0,
        private as f64 / 1048576.0
    );
    Ok(json!({ "workingSetBytes": working_set, "privateBytes": private }))
}

/// Press Play → first tick for every boot strategy.
pub fn boot_suite(ctx: &mut Ctx, gpu: bool) -> Result<Value> {
    let reps = ctx.reps;
    let mut in_process = vec![];
    for _ in 0..reps {
        let t = Instant::now();
        let bytes = snapshot::encode(&mut ctx.edit);
        let mut sim = Sim::new(snapshot::decode(&bytes)?);
        sim.tick();
        in_process.push(ms(t.elapsed()));
    }

    let mut warm_shm = vec![];
    let mut warm_bin = vec![];
    for _ in 0..reps {
        warm_shm.push(warm_boot(ctx, &Launch::standalone(), Transport::Shm)?);
        warm_bin.push(warm_boot(ctx, &Launch::standalone(), Transport::WsBinary)?);
    }

    let (mut cold, mut cold_hello) = (vec![], vec![]);
    for _ in 0..reps {
        let (total, hello, _) = cold_boot(ctx, &Launch::standalone())?;
        cold.push(total);
        cold_hello.push(hello);
    }

    let (mut fresh, mut fresh_hello) = (vec![], vec![]);
    for rep in 0..reps.min(10) {
        let path = fresh_binary(ctx.host.exe(), rep)?;
        let launch = Launch {
            exe: Some(path.clone()),
            ..Launch::standalone()
        };
        let (total, hello, _) = cold_boot(ctx, &launch)?;
        fresh.push(total);
        fresh_hello.push(hello);
        let _ = std::fs::remove_file(&path);
    }

    let mut out = json!({
        "idlePlayerMemory": idle_memory(ctx, &Launch::standalone())?,
        "inProcessSimulate": Stats::of(&in_process),
        "warmPoolShm": Stats::of(&warm_shm),
        "warmPoolWsBinary": Stats::of(&warm_bin),
        "coldProcessShm": Stats::of(&cold),
        "coldProcessSpawnToHello": Stats::of(&cold_hello),
        "freshBinaryShm": Stats::of(&fresh),
        "freshBinarySpawnToHello": Stats::of(&fresh_hello),
    });
    println!(
        "  boot in-process simulate      {}",
        Stats::of(&in_process).short()
    );
    println!(
        "  boot warm pool (shm)          {}",
        Stats::of(&warm_shm).short()
    );
    println!(
        "  boot warm pool (wsBinary)     {}",
        Stats::of(&warm_bin).short()
    );
    println!(
        "  boot cold process (shm)       {}  (spawn→hello {})",
        Stats::of(&cold).short(),
        Stats::of(&cold_hello).short()
    );
    println!(
        "  boot fresh binary (shm)       {}  (spawn→hello {})",
        Stats::of(&fresh).short(),
        Stats::of(&fresh_hello).short()
    );

    if gpu {
        let gpu_launch = Launch {
            gpu_init: true,
            ..Launch::standalone()
        };
        let (mut cold_gpu, mut warm_gpu) = (vec![], vec![]);
        let (mut total, mut instance, mut device) = (vec![], vec![], vec![]);
        for _ in 0..reps.min(10) {
            let (boot, _, timings) = cold_boot(ctx, &gpu_launch)?;
            cold_gpu.push(boot);
            let timings = timings.context("player skipped --gpu-init")?;
            total.push(us_ms(timings.total_us()));
            instance.push(us_ms(timings.loader_us + timings.instance_us));
            device.push(us_ms(timings.device_us));
            warm_gpu.push(warm_boot(ctx, &gpu_launch, Transport::Shm)?);
        }
        println!(
            "  boot cold + Vulkan device     {}  (Vulkan {}: loader+instance {}, device {})",
            Stats::of(&cold_gpu).short(),
            Stats::of(&total).short(),
            Stats::of(&instance).short(),
            Stats::of(&device).short()
        );
        println!(
            "  boot warm pool + Vulkan       {}",
            Stats::of(&warm_gpu).short()
        );
        out["coldProcessVulkanShm"] = json!(Stats::of(&cold_gpu));
        out["vulkanInit"] = json!(Stats::of(&total));
        out["vulkanLoaderAndInstance"] = json!(Stats::of(&instance));
        out["vulkanDevice"] = json!(Stats::of(&device));
        out["warmPoolVulkanShm"] = json!(Stats::of(&warm_gpu));
        out["idlePlayerMemoryVulkan"] = idle_memory(ctx, &gpu_launch)?;
        out["vkImplicitLayerPath"] = json!(std::env::var("VK_IMPLICIT_LAYER_PATH").ok());
    }
    Ok(out)
}
