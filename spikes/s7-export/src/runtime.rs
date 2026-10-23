//! The runtime template's core: open the pack, spawn the scene, bind C#, run fixed ticks.
//!
//! Used by the desktop `ara-player` binary and by the Android `NativeActivity` entry.
//! Everything platform-specific arrives through [`RunConfig`] and the pack bytes.

use std::collections::BTreeMap;
use std::ffi::{CString, c_void};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use libloading::Library;
use serde::{Deserialize, Serialize};

use crate::abi::{
    ABI_VERSION, Chunk, CoreclrCreateDelegate, CoreclrInitialize, HostApi, ScriptApi, ScriptInit,
    Tick, Transform, Velocity,
};
use crate::log;
use crate::pack::{Pack, Texture};

/// Rows per archetype chunk handed to C#.
pub const CHUNK_ROWS: usize = 16 * 1024;

/// How the C# gameplay library is bound.
#[derive(Clone, Debug)]
pub enum ScriptLane {
    /// A NativeAOT shared library exporting `ara_script_init` (a path, or a bare soname on Android).
    Aot { lib: PathBuf },
    /// A self-contained CoreCLR folder of ReadyToRun assemblies, hosted with `coreclr_initialize`.
    Clr { dir: PathBuf, assembly: String },
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
struct Spawner {
    count: usize,
    seed: u64,
    extent: f32,
    speed: f32,
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

/// Component columns for one archetype: `Transform` + `Velocity`.
#[derive(Clone)]
pub struct World {
    /// Column 0.
    pub transforms: Vec<Transform>,
    /// Column 1.
    pub velocities: Vec<Velocity>,
}

impl World {
    /// Deterministic spawn from the scene's seed (SplitMix64; no global RNG).
    fn spawn(s: Spawner) -> Self {
        let mut state = s.seed;
        let mut next = move || {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            ((z ^ (z >> 31)) >> 40) as f32 / (1u32 << 24) as f32
        };
        let mut transforms = Vec::with_capacity(s.count);
        let mut velocities = Vec::with_capacity(s.count);
        for _ in 0..s.count {
            transforms.push(Transform {
                x: (next() * 2.0 - 1.0) * s.extent,
                y: next() * s.extent,
                z: (next() * 2.0 - 1.0) * s.extent,
                yaw: next() * std::f32::consts::TAU,
            });
            velocities.push(Velocity {
                x: (next() * 2.0 - 1.0) * s.speed,
                y: (next() * 2.0 - 1.0) * s.speed,
                z: (next() * 2.0 - 1.0) * s.speed,
                yaw_rate: next() * 2.0 - 1.0,
            });
        }
        Self {
            transforms,
            velocities,
        }
    }

    /// Chunk views over the columns. Valid until the columns are resized or moved.
    fn chunks(&mut self) -> Vec<Chunk> {
        let t = self.transforms.as_mut_ptr();
        let v = self.velocities.as_mut_ptr();
        (0..self.transforms.len())
            .step_by(CHUNK_ROWS)
            .map(|start| Chunk {
                len: CHUNK_ROWS.min(self.transforms.len() - start) as u32,
                column_count: 2,
                // SAFETY: `start < len`, so both offsets stay inside their allocations.
                col0: unsafe { t.add(start) }.cast(),
                // SAFETY: as above; both columns have the same length.
                col1: unsafe { v.add(start) }.cast(),
            })
            .collect()
    }
}

/// The Rust twin of C# `Movement.Run`; identical operation order, so results must match bit-for-bit.
pub fn movement_reference(world: &mut World, dt: f32) {
    let g = 9.81f32 * dt;
    for (t, v) in world.transforms.iter_mut().zip(world.velocities.iter_mut()) {
        v.y -= g;
        t.x += v.x * dt;
        t.y += v.y * dt;
        t.z += v.z * dt;
        if t.y < 0.0 {
            t.y = -t.y;
            v.y = -v.y * 0.8;
        }
        t.yaw += v.yaw_rate * dt;
    }
}

/// FNV-1a 64 over the bit patterns of every `Transform` float, in row order.
pub fn checksum(transforms: &[Transform]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for t in transforms {
        for w in [t.x, t.y, t.z, t.yaw] {
            h ^= u64::from(w.to_bits());
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

extern "C" fn host_log(level: u32, msg: *const u8, len: usize) {
    // SAFETY: C# passes a UTF-8 buffer of `len` bytes that lives for the call.
    let bytes = unsafe { std::slice::from_raw_parts(msg, len) };
    log::write(level, "cs", &String::from_utf8_lossy(bytes));
}

extern "C" fn host_nop(x: u32) -> u32 {
    x.wrapping_add(1)
}

static HOST: HostApi = HostApi {
    abi_version: ABI_VERSION,
    struct_size: size_of::<HostApi>() as u32,
    log: host_log,
    nop: host_nop,
};

/// A bound C# script. The images stay loaded for the process lifetime: neither a
/// NativeAOT image nor CoreCLR supports unloading.
pub struct Script {
    api: ScriptApi,
}

impl Script {
    /// Loads the script image for `lane` and calls `ara_script_init`.
    /// Returns the script plus the image-load time (excluding the init call).
    pub fn load(lane: &ScriptLane) -> Result<(Self, Duration), String> {
        let started = Instant::now();
        let init = match lane {
            ScriptLane::Aot { lib } => load_aot(lib)?,
            ScriptLane::Clr { dir, assembly } => load_clr(dir, assembly)?,
        };
        let load = started.elapsed();
        let mut api = ScriptApi::default();
        // SAFETY: `init` is `ara_script_init` with the ABI in `abi.rs`; `HOST` is 'static.
        let status = unsafe { init(&HOST, &mut api) };
        if status != 0 {
            return Err(format!("ara_script_init failed with status {status}"));
        }
        if api.abi_version != ABI_VERSION || api.struct_size as usize != size_of::<ScriptApi>() {
            return Err(format!(
                "ABI mismatch: C# v{} size {}, host v{ABI_VERSION} size {}",
                api.abi_version,
                api.struct_size,
                size_of::<ScriptApi>()
            ));
        }
        Ok((Self { api }, load))
    }
}

fn load_aot(lib: &Path) -> Result<ScriptInit, String> {
    // SAFETY: loading our own NativeAOT image; its initializers are the .NET runtime's.
    let library =
        unsafe { Library::new(lib) }.map_err(|e| format!("loading {}: {e}", lib.display()))?;
    // SAFETY: the symbol has the `ScriptInit` signature (see game/Exports.cs).
    let init =
        *unsafe { library.get::<ScriptInit>(b"ara_script_init\0") }.map_err(|e| e.to_string())?;
    std::mem::forget(library);
    Ok(init)
}

fn load_clr(dir: &Path, assembly: &str) -> Result<ScriptInit, String> {
    let name = if cfg!(windows) {
        "coreclr.dll"
    } else if cfg!(target_os = "macos") {
        "libcoreclr.dylib"
    } else {
        "libcoreclr.so"
    };
    let dir = std::path::absolute(dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    // SAFETY: loading the bundled CoreCLR from the export folder.
    let library =
        unsafe { Library::new(&path) }.map_err(|e| format!("loading {}: {e}", path.display()))?;
    // SAFETY: signatures from coreclrhost.h.
    let initialize = *unsafe { library.get::<CoreclrInitialize>(b"coreclr_initialize\0") }
        .map_err(|e| e.to_string())?;
    // SAFETY: as above.
    let create = *unsafe { library.get::<CoreclrCreateDelegate>(b"coreclr_create_delegate\0") }
        .map_err(|e| e.to_string())?;
    std::mem::forget(library);

    let mut tpa: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "dll"))
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    tpa.sort();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let dir_s = dir.to_string_lossy().into_owned();
    // hostfxr normally supplies RUNTIME_IDENTIFIER; direct coreclr hosting must pass it itself.
    let os = match std::env::consts::OS {
        "windows" => "win",
        "macos" => "osx",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let props = [
        ("RUNTIME_IDENTIFIER", format!("{os}-{arch}")),
        ("TRUSTED_PLATFORM_ASSEMBLIES", tpa.join(sep)),
        ("APP_PATHS", dir_s.clone()),
        ("NATIVE_DLL_SEARCH_DIRECTORIES", dir_s),
        ("System.Globalization.Invariant", "true".into()),
    ];
    let cstr = |s: &str| CString::new(s).map_err(|e| e.to_string());
    let keys = props
        .iter()
        .map(|(k, _)| cstr(k))
        .collect::<Result<Vec<_>, _>>()?;
    let values = props
        .iter()
        .map(|(_, v)| cstr(v))
        .collect::<Result<Vec<_>, _>>()?;
    let key_ptrs: Vec<_> = keys.iter().map(|k| k.as_ptr()).collect();
    let value_ptrs: Vec<_> = values.iter().map(|v| v.as_ptr()).collect();
    let exe = cstr(
        &std::env::current_exe()
            .map_err(|e| e.to_string())?
            .to_string_lossy(),
    )?;
    let mut handle: *mut c_void = std::ptr::null_mut();
    let mut domain = 0u32;
    // SAFETY: all pointers are valid NUL-terminated strings that outlive the call.
    let hr = unsafe {
        initialize(
            exe.as_ptr(),
            c"ara".as_ptr(),
            key_ptrs.len() as i32,
            key_ptrs.as_ptr(),
            value_ptrs.as_ptr(),
            &mut handle,
            &mut domain,
        )
    };
    if hr < 0 {
        return Err(format!("coreclr_initialize failed: 0x{:08x}", hr as u32));
    }
    let mut fp: *mut c_void = std::ptr::null_mut();
    let asm = cstr(assembly)?;
    // SAFETY: valid handle/domain from coreclr_initialize; NUL-terminated names.
    let hr = unsafe {
        create(
            handle,
            domain,
            asm.as_ptr(),
            c"Ara.Game.Exports".as_ptr(),
            c"Init".as_ptr(),
            &mut fp,
        )
    };
    if hr < 0 || fp.is_null() {
        return Err(format!(
            "coreclr_create_delegate(Exports.Init) failed: 0x{:08x}",
            hr as u32
        ));
    }
    // SAFETY: `Exports.Init` is [UnmanagedCallersOnly] with the `ScriptInit` signature.
    Ok(unsafe { std::mem::transmute::<*mut c_void, ScriptInit>(fp) })
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

    let scene_bytes = pack
        .read("scene/main.scene.json")
        .map_err(|e| e.to_string())?;
    let scene: Scene = serde_json::from_slice(&scene_bytes).map_err(|e| format!("scene: {e}"))?;
    if scene.format != "ara.scene/0" {
        return Err(format!("unknown scene format {}", scene.format));
    }
    let spawner = scene
        .entities
        .values()
        .find_map(|e| e.spawner)
        .ok_or("scene has no Spawner")?;
    let albedo = scene
        .entities
        .values()
        .find_map(|e| e.material.as_ref())
        .ok_or("scene has no Material")?;
    let tex_path = albedo
        .albedo
        .strip_prefix('@')
        .ok_or("references must be @path")?;
    let tex_bytes = pack.read(tex_path).map_err(|e| e.to_string())?;
    let tex = Texture::parse(&tex_bytes).map_err(|e| e.to_string())?;
    let texture = format!(
        "{}x{} rgba8 blake3:{}",
        tex.width,
        tex.height,
        &blake3::hash(tex.pixels).to_hex()[..16]
    );
    let content = t0.elapsed();

    let mut world = World::spawn(spawner);
    let spawn = t0.elapsed();

    let (script, load) = Script::load(&cfg.lane)?;
    let first_call = t0.elapsed();
    let spawn_to_first = cfg
        .spawn_unix_ns
        .map(|s| (unix_ns().saturating_sub(s)) as f64 / 1e6);
    log::write(
        1,
        "rt",
        &format!(
            "pack {} ok ({} bytes), texture {texture}, first C# call at {:.2} ms",
            &pack.id.to_hex()[..16],
            pack.len(),
            ms(first_call)
        ),
    );

    let api = &script.api;
    let run_system = api.run_system.ok_or("run_system missing")?;
    let cs_checksum = api.checksum.ok_or("checksum missing")?;
    let ticks = cfg.ticks.unwrap_or(scene.sim.ticks);
    let dt = 1.0 / scene.sim.tick_hz as f32;

    let mut reference = cfg.verify.then(|| world.clone());
    let chunks = world.chunks();
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
        if let Some(r) = reference.as_mut() {
            let started = Instant::now();
            movement_reference(r, dt);
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
    let sum_rust = reference.as_ref().map(|r| checksum(&r.transforms));
    let verified = sum_rust.is_none_or(|r| r == sum_cs) && checksum(&world.transforms) == sum_cs;
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

    let entities = world.transforms.len();
    let cs_min = cs_times.iter().copied().fold(f64::MAX, f64::min);
    Ok(RunReport {
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        lane: cfg.lane.name(),
        pack_id: pack.id.to_hex().to_string(),
        pack_bytes: pack.len(),
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
