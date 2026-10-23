//! The export conformance template's core: open the pack, spawn the scene, bind C#, run fixed ticks.
//!
//! Used by the desktop `ara-player` binary and by the Android `NativeActivity` entry.
//! Everything platform-specific arrives through [`RunConfig`] and the pack bytes.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::abi::{ScriptApi, Tick};
use crate::columns::{Columns, checksum, movement_reference};
use crate::log;
use crate::script::Script;
use ara_pack::Pack;
use bevy_ecs::prelude::World as EcsWorld;

/// Rows per archetype chunk handed to C#.
pub const CHUNK_ROWS: usize = 16 * 1024;

/// How the C# gameplay library is bound.
#[derive(Clone, Debug)]
pub enum ScriptLane {
    /// A NativeAOT shared library exporting `ara_script_init` (a path, or a bare soname on Android).
    Aot {
        /// Path or Android soname of the shared NativeAOT library.
        lib: PathBuf,
    },
    /// A self-contained CoreCLR folder of ReadyToRun assemblies, hosted with `coreclr_initialize`.
    Clr {
        /// Self-contained CoreCLR directory.
        dir: PathBuf,
        /// Managed assembly stem.
        assembly: String,
    },
}

impl ScriptLane {
    fn name(&self) -> &'static str {
        match self {
            ScriptLane::Aot { .. } => "nativeaot",
            ScriptLane::Clr { .. } => "r2r-coreclr",
        }
    }
}

/// Everything the platform entry decides before calling [`run`].
pub struct RunConfig {
    /// Explicit child-process fault probe, run only after the managed image has loaded.
    pub crash_probe: Option<crate::shipping::CrashProbe>,
    /// Optional content decryption key supplied by the runtime host, never stored in the pack.
    pub pack_key: Option<ara_pack::EncryptionKey>,
    /// Script binding lane.
    pub lane: ScriptLane,
    /// Override the scene's tick count.
    pub ticks: Option<u64>,
    /// Iterations for the call-overhead benchmark; 0 skips it.
    pub bench_iters: u64,
    /// Run the Rust reference system alongside C# and compare checksums.
    pub verify: bool,
    /// Optional IL mod to load through C#.
    pub mod_path: Option<PathBuf>,
    /// Wall-clock UNIX ns at which the parent spawned us (`ARA_T0_UNIX_NS`), for cold-start timing.
    pub spawn_unix_ns: Option<u128>,
}

/// What a run measured and proved; printed as one `ARA_RESULT {json}` line.
#[derive(Serialize)]
pub struct RunReport {
    /// This fixture validates shipping and the S7 ABI, not authored demo gameplay.
    pub template_contract: &'static str,
    /// `os-arch` of the running binary.
    pub platform: String,
    /// `nativeaot` or `r2r-coreclr`.
    pub lane: &'static str,
    /// Pack id (manifest BLAKE3).
    pub pack_id: String,
    /// Pack file size.
    pub pack_bytes: usize,
    /// Cooked texture summary.
    pub texture: String,
    /// Entities simulated.
    pub entities: usize,
    /// Fixed ticks run.
    pub ticks: u64,
    /// Checksum computed (and logged) by C#.
    pub checksum_cs: String,
    /// Checksum of the Rust reference run, when verifying.
    pub checksum_rust: Option<String>,
    /// C# and Rust agree bit-for-bit (true when not verifying).
    pub verified: bool,
    /// The intentionally faulting system returned status 1 and the process survived.
    pub fault_contained: bool,
    /// `load_mod` status: 0 loaded, 2 rejected (expected under NativeAOT).
    pub mod_status: Option<i32>,
    /// Startup phases in ms since entry.
    pub startup: Startup,
    /// Per-tick timing of the C# system.
    pub tick: TickStats,
    /// Boundary call costs.
    pub bench: Option<Bench>,
}

/// Startup timeline, all in ms.
#[derive(Serialize, Default)]
pub struct Startup {
    /// Entry → pack opened and manifest verified.
    pub pack_open_ms: f64,
    /// Entry → scene parsed and texture decoded.
    pub content_ms: f64,
    /// Entry → 100k entities spawned.
    pub spawn_ms: f64,
    /// Time spent loading the script image (and CoreCLR, on that lane).
    pub script_load_ms: f64,
    /// Entry → `ara_script_init` returned: the first C# call.
    pub first_cs_call_ms: f64,
    /// Parent spawn → first C# call, when `ARA_T0_UNIX_NS` was given.
    pub spawn_to_first_cs_call_ms: Option<f64>,
}

/// C# system timing.
#[derive(Serialize, Default)]
pub struct TickStats {
    /// Mean ms per tick (C#).
    pub cs_mean_ms: f64,
    /// Fastest tick (C#).
    pub cs_min_ms: f64,
    /// ns per entity at the fastest tick (C#).
    pub cs_ns_per_entity: f64,
    /// ns per entity at the fastest tick of the Rust reference.
    pub rust_ns_per_entity: Option<f64>,
}

/// Rust↔C# boundary costs in ns per call.
#[derive(Serialize)]
pub struct Bench {
    /// Iterations per measurement.
    pub iters: u64,
    /// Rust calling an empty `[UnmanagedCallersOnly]` export.
    pub rust_to_cs_ns: f64,
    /// C# calling an empty Rust callback through `delegate* unmanaged`.
    pub cs_to_rust_ns: f64,
    /// Same with `[SuppressGCTransition]`.
    pub cs_to_rust_fast_ns: f64,
}

/// Scene document (A3-shaped: a flat entity map keyed by short ids).
#[derive(Deserialize)]
struct Scene {
    format: String,
    entities: BTreeMap<String, SceneEntity>,
    sim: Sim,
}

#[derive(Deserialize)]
struct SceneEntity {
    #[serde(rename = "Spawner")]
    spawner: Option<Spawner>,
    #[serde(rename = "Material")]
    material: Option<Material>,
}

#[derive(Deserialize, Clone, Copy)]
pub(crate) struct Spawner {
    pub(crate) count: usize,
    pub(crate) seed: u64,
    pub(crate) extent: f32,
    pub(crate) speed: f32,
}

#[derive(Deserialize)]
struct Material {
    albedo: String,
}

#[derive(Deserialize)]
struct Sim {
    tick_hz: u32,
    ticks: u64,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// Wall-clock UNIX ns; only for the cold-start measurement, never for simulation.
pub fn unix_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Runs the game from pack bytes: content → spawn → bind C# → fixed ticks → verify → bench.
pub fn run(cfg: &RunConfig, pack_bytes: &[u8], t0: Instant) -> Result<RunReport, String> {
    let pack = Pack::open(pack_bytes).map_err(|e| e.to_string())?;
    let pack_open = t0.elapsed();

    let (scene, texture) = if pack.manifest().entries.contains_key("ara.project.json") {
        // The real demo is packed verbatim. Validate every file and its main
        // scene; the existing S7 workload remains the conformance simulation.
        pack.validate_all(cfg.pack_key.as_ref())
            .map_err(|e| e.to_string())?;
        let project: serde_json::Value = serde_json::from_slice(
            &pack
                .read("ara.project.json", 1024 * 1024, cfg.pack_key.as_ref())
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let main = project
            .get("main")
            .and_then(|v| v.as_str())
            .ok_or("project main scene is missing")?;
        let bytes = pack
            .read(main, 16 * 1024 * 1024, cfg.pack_key.as_ref())
            .map_err(|e| e.to_string())?;
        let document: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if document.get("$schema").and_then(|v| v.as_str()) != Some("ara://schema/scene/1")
            || !document.get("records").is_some_and(|v| v.is_object())
        {
            return Err("invalid packed demo scene".into());
        }
        let scene: Scene = serde_json::from_slice(include_bytes!(
            "../../ara-export/fixtures/sample/content/scene/main.scene.json"
        ))
        .map_err(|e| e.to_string())?;
        (
            scene,
            format!(
                "validated {} packed demo files; main {main}; conformance simulation only",
                pack.manifest().entries.len()
            ),
        )
    } else {
        let scene_bytes = pack
            .read(
                "scene/main.scene.json",
                16 * 1024 * 1024,
                cfg.pack_key.as_ref(),
            )
            .map_err(|e| e.to_string())?;
        let scene: Scene =
            serde_json::from_slice(&scene_bytes).map_err(|e| format!("scene: {e}"))?;
        if scene.format != "ara.scene/0" {
            return Err(format!("unknown scene format {}", scene.format));
        }
        let albedo = scene
            .entities
            .values()
            .find_map(|e| e.material.as_ref())
            .ok_or("scene has no Material")?;
        let tex_path = albedo
            .albedo
            .strip_prefix('@')
            .ok_or("references must be @path")?;
        let tex_bytes = pack
            .read(tex_path, 16 * 1024 * 1024, cfg.pack_key.as_ref())
            .map_err(|e| e.to_string())?;
        let texture = format!(
            "fixture-media blake3:{}",
            &blake3::hash(&tex_bytes).to_hex()[..16]
        );
        (scene, texture)
    };
    let spawner = scene
        .entities
        .values()
        .find_map(|e| e.spawner)
        .ok_or("scene has no conformance Spawner")?;
    let content = t0.elapsed();

    if spawner.count == 0
        || spawner.count > 1_000_000
        || scene.sim.tick_hz == 0
        || scene.sim.ticks == 0
        || scene.sim.ticks > 100_000
    {
        return Err("fixture scene exceeds conformance limits".into());
    }
    let mut ecs = EcsWorld::new();
    let columns = Columns::spawn(spawner);
    let reference = cfg.verify.then(|| ecs.spawn(columns.clone()).id());
    let simulation = ecs.spawn(columns).id();
    let spawn = t0.elapsed();

    let (script, load) = Script::load(&cfg.lane)?;
    match cfg.crash_probe {
        Some(crate::shipping::CrashProbe::Native) => ara_crash::native_crash_probe(),
        Some(crate::shipping::CrashProbe::Panic) => panic!("Ara exported player crash probe"),
        Some(crate::shipping::CrashProbe::Managed) | None => {}
    }
    let first_call = t0.elapsed();
    let spawn_to_first = cfg
        .spawn_unix_ns
        .map(|s| (unix_ns().saturating_sub(s)) as f64 / 1e6);
    log::write(
        1,
        "rt",
        &format!(
            "pack {} ok ({} bytes), texture {texture}, first C# call at {:.2} ms",
            &pack.id().map_err(|e| e.to_string())?[..16],
            pack_bytes.len(),
            ms(first_call)
        ),
    );

    let api = &script.api;
    let run_system = api.run_system.ok_or("run_system missing")?;
    let cs_checksum = api.checksum.ok_or("checksum missing")?;
    let ticks = cfg.ticks.unwrap_or(scene.sim.ticks);
    let dt = 1.0 / scene.sim.tick_hz as f32;

    let chunks = ecs
        .get_mut::<Columns>(simulation)
        .ok_or("missing simulation component")?
        .chunks();
    let mut cs_times = Vec::with_capacity(ticks as usize);
    let mut rust_best = f64::MAX;
    for frame in 0..ticks {
        let tick = Tick {
            frame,
            dt,
            elapsed: frame as f32 * dt,
        };
        let started = Instant::now();
        // SAFETY: chunks point into `world`, which is not touched while C# runs.
        let status = unsafe { run_system(0, chunks.as_ptr(), chunks.len() as u32, &tick) };
        cs_times.push(ms(started.elapsed()));
        if status != 0 {
            return Err(format!(
                "Movement failed at tick {frame} with status {status}"
            ));
        }
        if let Some(entity) = reference {
            let mut r = ecs
                .get_mut::<Columns>(entity)
                .ok_or("missing reference component")?;
            let started = Instant::now();
            movement_reference(&mut r, dt);
            rust_best = rust_best.min(ms(started.elapsed()));
        }
    }

    // System 1 throws inside C#; the boundary must turn that into status 1, not a crash.
    let tick = Tick {
        frame: ticks,
        dt,
        elapsed: ticks as f32 * dt,
    };
    // SAFETY: as above.
    let fault_contained =
        unsafe { run_system(1, chunks.as_ptr(), chunks.len() as u32, &tick) } == 1;

    // SAFETY: as above.
    let sum_cs = unsafe { cs_checksum(chunks.as_ptr(), chunks.len() as u32) };
    let sum_rust = reference
        .and_then(|e| ecs.get::<Columns>(e))
        .map(|r| checksum(&r.transforms));
    let verified = sum_rust.is_none_or(|r| r == sum_cs)
        && checksum(
            &ecs.get::<Columns>(simulation)
                .ok_or("missing simulation component")?
                .transforms,
        ) == sum_cs;
    log::write(
        if verified { 1 } else { 2 },
        "rt",
        &format!(
            "verify: C# 0x{sum_cs:016x} vs Rust {} -> {}",
            sum_rust.map_or("skipped".into(), |r| format!("0x{r:016x}")),
            if verified { "MATCH" } else { "MISMATCH" }
        ),
    );

    let mod_status = match (&cfg.mod_path, api.load_mod) {
        (Some(path), Some(load_mod)) => {
            let p = path.to_string_lossy();
            // SAFETY: UTF-8 buffer valid for the call.
            Some(unsafe { load_mod(p.as_ptr(), p.len()) })
        }
        _ => None,
    };

    let bench = if cfg.bench_iters > 0 {
        Some(bench(api, cfg.bench_iters)?)
    } else {
        None
    };

    let entities = ecs
        .get::<Columns>(simulation)
        .ok_or("missing simulation component")?
        .transforms
        .len();
    let cs_min = cs_times.iter().copied().fold(f64::MAX, f64::min);
    Ok(RunReport {
        template_contract: "ara.player.conformance/1",
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        lane: cfg.lane.name(),
        pack_id: pack.id().map_err(|e| e.to_string())?,
        pack_bytes: pack_bytes.len(),
        texture,
        entities,
        ticks,
        checksum_cs: format!("0x{sum_cs:016x}"),
        checksum_rust: sum_rust.map(|r| format!("0x{r:016x}")),
        verified,
        fault_contained,
        mod_status,
        startup: Startup {
            pack_open_ms: ms(pack_open),
            content_ms: ms(content),
            spawn_ms: ms(spawn),
            script_load_ms: ms(load),
            first_cs_call_ms: ms(first_call),
            spawn_to_first_cs_call_ms: spawn_to_first,
        },
        tick: TickStats {
            cs_mean_ms: cs_times.iter().sum::<f64>() / cs_times.len().max(1) as f64,
            cs_min_ms: cs_min,
            cs_ns_per_entity: cs_min * 1e6 / entities as f64,
            rust_ns_per_entity: (rust_best < f64::MAX).then(|| rust_best * 1e6 / entities as f64),
        },
        bench,
    })
}

fn bench(api: &ScriptApi, iters: u64) -> Result<Bench, String> {
    let nop = api.nop.ok_or("nop missing")?;
    let call_host = api.call_host.ok_or("call_host missing")?;
    let per_call = |d: Duration| d.as_secs_f64() * 1e9 / iters as f64;

    // Warm both directions once so first-call stubs are excluded.
    // SAFETY: plain value-only exports.
    unsafe {
        nop(0);
        call_host(1000, 0);
        call_host(1000, 1);
    }
    let started = Instant::now();
    let mut acc = 0u32;
    for i in 0..iters as u32 {
        // SAFETY: as above.
        acc = acc.wrapping_add(unsafe { nop(std::hint::black_box(i)) });
    }
    let rust_to_cs = per_call(started.elapsed());
    std::hint::black_box(acc);

    let started = Instant::now();
    // SAFETY: as above.
    std::hint::black_box(unsafe { call_host(iters, 0) });
    let cs_to_rust = per_call(started.elapsed());
    let started = Instant::now();
    // SAFETY: as above.
    std::hint::black_box(unsafe { call_host(iters, 1) });
    let cs_to_rust_fast = per_call(started.elapsed());
    Ok(Bench {
        iters,
        rust_to_cs_ns: rust_to_cs,
        cs_to_rust_ns: cs_to_rust,
        cs_to_rust_fast_ns: cs_to_rust_fast,
    })
}
