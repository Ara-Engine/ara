//! Save → visible latency of L1 and L3 reloads through the out-of-process compile server.
//!
//! The module is the Outpost sample plus generated filler systems (about 5k lines, the
//! soft cap) and a probe system whose constant each edit changes. A 60 Hz tick loop runs
//! the sample's systems over 20k entities; an "editor" thread saves edits and waits for
//! the first tick whose code wrote the new constant. Latency is save → end of that tick.
//!
//! ```sh
//! dotnet build dotnet/Ara.slnx -c Release
//! cargo run -p ara-script --release --example reload_bench -- [--edits 40] [--lines 5000]
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, Instant};

use ara_schema::SchemaDb;
use ara_script::abi::{RELOAD_L1, RELOAD_L3, ReloadReport, Tick};
use ara_script::{EngineBindings, ModuleSpec, ScriptConfig, ScriptHost, bind_component};
use bevy_ecs::component::Component;
use bevy_ecs::world::World;
use serde_json::json;

/// The engine `Transform` (canonical layout of `ara_components::Transform`).
#[derive(Component, Clone, Copy, Default)]
#[repr(C)]
struct EcsTransform {
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
}

fn dotnet() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dotnet")
}

fn bin(project: &str) -> PathBuf {
    dotnet().join("artifacts/bin").join(project).join("release")
}

fn arg(name: &str, default: usize) -> usize {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

const PROBE: &str = "namespace Bench;

/// <summary>What the edited system writes.</summary>
[Component]
public partial struct BenchProbe
{
    /// <summary>The constant of the running code.</summary>
    public int Value;
}

public static partial class BenchProbes
{
    [Query]
    static void Run(ref BenchProbe p)
    {
        p.Value = 0; // @value
    }
    // @pad
}
";

/// A filler file: one component and one system with a little arithmetic (~50 lines).
fn filler(i: usize) -> String {
    let mut s = format!(
        "namespace Bench;\n\n/// <summary>Filler state {i}.</summary>\n[Component]\npublic partial struct Filler{i}\n{{\n    /// <summary>A value.</summary>\n    public float A;\n    /// <summary>Another value.</summary>\n    public float B;\n}}\n\n/// <summary>Filler systems {i}.</summary>\npublic static partial class FillerSystems{i}\n{{\n    [Query]\n    static void Step(ref Filler{i} f, in Tick t)\n    {{\n"
    );
    for k in 0..12 {
        s.push_str(&format!("        f.A = f.A * 0.5f + {k}.25f * t.Dt;\n        f.B = MathF.Max(f.B, f.A - {k});\n"));
    }
    s.push_str("    }\n\n    /// <summary>A helper.</summary>\n    public static float Helper(float x)\n    {\n");
    for k in 0..8 {
        s.push_str(&format!("        x = x * 1.0001f + {k};\n"));
    }
    s.push_str("        return x;\n    }\n}\n");
    s
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        return f64::NAN;
    }
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn main() {
    let edits = arg("--edits", 45);
    let lines_target = arg("--lines", 5000);
    let dir = std::env::temp_dir().join(format!("ara-reload-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("bench dir");
    let sample = dotnet().join("samples/Outpost.Sample");
    for f in ["ara.module.toml", "Vitals.cs", "Motion.cs", "Door.cs"] {
        std::fs::copy(sample.join(f), dir.join(f)).expect("copy sample");
    }
    std::fs::write(dir.join("Probe.cs"), PROBE).expect("probe");
    let mut lines = 0;
    let mut i = 0;
    while lines < lines_target {
        let text = filler(i);
        lines += text.lines().count();
        std::fs::write(dir.join(format!("Filler{i}.cs")), text).expect("filler");
        i += 1;
    }
    println!("module: {} filler files, ~{} lines", i, lines + 150);

    let mut db = SchemaDb::new();
    ara_components::register(&mut db);
    let engine = bind_component!(EngineBindings::new(), EcsTransform as "Transform" { position, rotation, scale });
    let mut world = World::new();
    let verbose = std::env::args().any(|a| a == "--verbose");
    let mut config = ScriptConfig::jit(bin("Ara.Host"));
    config.log = Some(Box::new(move |level, msg| {
        if level > 0 || verbose {
            eprintln!("[cs] {msg}");
        }
    }));
    config.compile_server = Some(bin("Ara.CompileServer").join("Ara.CompileServer.dll"));
    let t0 = Instant::now();
    let mut host = ScriptHost::start(config, db, &engine, &mut world).expect("host");
    host.load_modules(&mut world, &[ModuleSpec::from_dir(&dir).expect("module")]).expect("initial compile");
    println!("start + initial compile: {:.0} ms", t0.elapsed().as_secs_f64() * 1e3);

    for n in 0..20_000 {
        let e = world.spawn(EcsTransform::default()).id();
        host.insert_component(&mut world, e, "Velocity", &json!({ "linear": [1, 0, 0] })).expect("velocity");
        host.insert_component(&mut world, e, "Mass", &json!({ "value": if n % 2 == 0 { 1 } else { 0 } })).expect("mass");
    }
    let probe = world.spawn_empty().id();
    host.insert_component(&mut world, probe, "BenchProbe", &json!({})).expect("probe");

    let visible = Arc::new(AtomicI64::new(i64::MIN));
    let done = Arc::new(AtomicBool::new(false));
    let editor = {
        let (visible, done, file) = (visible.clone(), done.clone(), dir.join("Probe.cs"));
        std::thread::spawn(move || {
            let mut results: Vec<(bool, f64)> = Vec::new();
            let wait = |k: i64| -> Option<Instant> {
                let deadline = Instant::now() + Duration::from_secs(20);
                while Instant::now() < deadline {
                    if visible.load(Ordering::Acquire) == k {
                        return Some(Instant::now());
                    }
                    std::thread::sleep(Duration::from_micros(200));
                }
                None
            };
            std::thread::sleep(Duration::from_secs(2)); // let the prewarm finish
            let mut text = PROBE.to_owned();
            let mut value = 0i64;
            // Edit 0 makes the cold module hot (one L3); then L1 and L3 edits alternate 2:1.
            for e in 0..=edits {
                value += 1;
                let structural = e == 0 || e % 3 == 2;
                text = text.replace(&format!("p.Value = {};", value - 1), &format!("p.Value = {value};"));
                if structural {
                    text = text.replace("// @pad", &format!("static int Pad{e}() => {e};\n    // @pad"));
                }
                std::fs::write(&file, &text).expect("save");
                let saved = Instant::now();
                match wait(value) {
                    Some(seen) if e > 0 => results.push((structural, (seen - saved).as_secs_f64() * 1e3)),
                    Some(_) => {}
                    None => eprintln!("edit {e} never became visible"),
                }
                std::thread::sleep(Duration::from_millis(150 + (e as u64 * 37) % 120));
            }
            done.store(true, Ordering::Release);
            results
        })
    };

    let dt = 1.0 / 60.0;
    let mut frame = 0u64;
    let mut reports: Vec<ReloadReport> = Vec::new();
    let mut ticks: Vec<f64> = Vec::new();
    let mut boundaries: Vec<f64> = Vec::new();
    let mut next = Instant::now();
    while !done.load(Ordering::Acquire) {
        let start = Instant::now();
        let b = host.tick_boundary(&mut world);
        if b.reloaded {
            boundaries.push(start.elapsed().as_secs_f64() * 1e3);
        }
        reports.extend(b.reports);
        host.run_tick(&mut world, Tick { frame, seed: 1, elapsed: frame as f64 * dt, dt: dt as f32, reserved: 0 });
        ticks.push(start.elapsed().as_secs_f64() * 1e3);
        let v = host.read_component(&world, probe, "BenchProbe").and_then(|p| p["value"].as_i64()).unwrap_or(-1);
        visible.store(v, Ordering::Release);
        frame += 1;
        next += Duration::from_secs_f64(dt);
        // Sleep most of the gap (high-resolution timers), spin only the last half millisecond.
        if let Some(gap) = next.checked_duration_since(Instant::now()).and_then(|g| g.checked_sub(Duration::from_micros(500))) {
            std::thread::sleep(gap);
        }
        while Instant::now() < next {
            std::hint::spin_loop();
        }
    }
    let results = editor.join().expect("editor thread");
    let mut l1: Vec<f64> = results.iter().filter(|r| !r.0).map(|r| r.1).collect();
    let mut l3: Vec<f64> = results.iter().filter(|r| r.0).map(|r| r.1).collect();
    let kinds = |k: u32| reports.iter().filter(|r| r.kind == k).count();
    println!("reload reports: {} L1, {} L3", kinds(RELOAD_L1), kinds(RELOAD_L3));
    println!(
        "L1 save→visible (n={}): p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        l1.len(),
        percentile(&mut l1, 0.5),
        percentile(&mut l1, 0.95),
        l1.iter().copied().fold(0.0, f64::max)
    );
    println!(
        "L3 save→visible (n={}): p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        l3.len(),
        percentile(&mut l3, 0.5),
        percentile(&mut l3, 0.95),
        l3.iter().copied().fold(0.0, f64::max)
    );
    let phase = |k: u32, f: &dyn Fn(&ReloadReport) -> f64| -> f64 {
        let mut v: Vec<f64> = reports.iter().filter(|r| r.kind == k).map(f).collect();
        percentile(&mut v, 0.5)
    };
    for (name, k) in [("L1", RELOAD_L1), ("L3", RELOAD_L3)] {
        println!(
            "{name} phase medians: server (event→sent) {:.1} ms, staging {:.1} ms, wait for boundary {:.1} ms",
            phase(k, &|r| r.server_us as f64 / 1e3),
            phase(k, &|r| (r.t_staged.saturating_sub(r.t_received)) as f64 / 1e6),
            phase(k, &|r| (r.t_applied.saturating_sub(r.t_staged)) as f64 / 1e6),
        );
    }
    let mut all = ticks.clone();
    println!(
        "tick (systems + behaviours, 20k entities): p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms",
        percentile(&mut all.clone(), 0.5),
        percentile(&mut all, 0.99),
        ticks.iter().copied().fold(0.0, f64::max)
    );
    println!(
        "reload tick boundaries (registration + schedule rebuild): p50 {:.2} ms, max {:.2} ms",
        percentile(&mut boundaries.clone(), 0.5),
        boundaries.iter().copied().fold(0.0, f64::max)
    );
    let errors: Vec<_> = host.take_diagnostics().into_iter().filter(|d| d.is_error()).collect();
    if !errors.is_empty() {
        println!("errors: {errors:#?}");
    }
    println!("leaked generations at shutdown: {}", host.shutdown());
    let _ = std::fs::remove_dir_all(dir);
}
