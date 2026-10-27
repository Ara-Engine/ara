//! Save → visible latency of L1, L2 and L3 reloads through the out-of-process compile
//! server, with a per-phase breakdown (p50 and p95).
//!
//! The module is the Outpost sample plus generated filler systems (about 5k lines, the
//! soft cap) and a probe system whose constant each edit changes. A 60 Hz tick loop runs
//! the sample's systems over 20k entities; an "editor" thread saves edits and waits for
//! the first tick whose code wrote the new constant. Latency is save → end of that tick.
//! Edits cycle: L1 (a body), L2 (a new method), L3 (a signature change).
//!
//! ```sh
//! dotnet build dotnet/Ara.slnx -c Release
//! cargo run -p ara-script --release --example reload_bench -- [--edits 60] [--lines 5000] [--verbose]
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, Instant};

use ara_script::abi::{RELOAD_L1, RELOAD_L2, RELOAD_L3, ReloadReport, Tick};
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

/// The probe helper's two signatures: each L3 edit swaps one for the other.
const SHAPE_INT: &str = "static int Shape(int x) => x;";
const SHAPE_LONG: &str = "static int Shape(long x) => (int)x;";

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
        p.Value = Shape(0); // @value
    }

    static int Shape(int x) => x;
    // @pad
}
";

/// A filler file: one component and one system with a little arithmetic (~50 lines).
fn filler(i: usize) -> String {
    let mut s = format!(
        "namespace Bench;\n\n/// <summary>Filler state {i}.</summary>\n[Component]\npublic partial struct Filler{i}\n{{\n    /// <summary>A value.</summary>\n    public float A;\n    /// <summary>Another value.</summary>\n    public float B;\n}}\n\n/// <summary>Filler systems {i}.</summary>\npublic static partial class FillerSystems{i}\n{{\n    [Query]\n    static void Step(ref Filler{i} f, in Tick t)\n    {{\n"
    );
    for k in 0..12 {
        s.push_str(&format!(
            "        f.A = f.A * 0.5f + {k}.25f * t.Dt;\n        f.B = MathF.Max(f.B, f.A - {k});\n"
        ));
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

/// System-wide CPU busy fraction since the previous call (Windows; NaN elsewhere).
struct Cpu(u64, u64);

impl Cpu {
    fn now() -> Self {
        let (idle, total) = cpu_times();
        Cpu(idle, total)
    }

    fn busy_since(&self) -> f64 {
        let (idle, total) = cpu_times();
        let (di, dt) = (idle.saturating_sub(self.0), total.saturating_sub(self.1));
        if dt == 0 {
            f64::NAN
        } else {
            1.0 - di as f64 / dt as f64
        }
    }
}

#[cfg(windows)]
fn cpu_times() -> (u64, u64) {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut idle, mut kernel, mut user) = (zero, zero, zero);
    // SAFETY: three valid out-pointers.
    unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    let v = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    (v(idle), v(kernel) + v(user)) // kernel time includes idle time
}

#[cfg(not(windows))]
fn cpu_times() -> (u64, u64) {
    (0, 0)
}

/// One measured edit: its kind, save → visible latency, the report and the first tick on it.
struct Sample {
    kind: u32,
    total_ms: f64,
    report: ReloadReport,
    first_tick_ms: f64,
}

fn main() {
    let edits = arg("--edits", 60);
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

    let db = ara_runtime::schema(&[]).expect("engine schema");
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
    host.load_modules(&mut world, &[ModuleSpec::from_dir(&dir).expect("module")])
        .expect("initial compile");
    println!(
        "start + initial compile: {:.0} ms",
        t0.elapsed().as_secs_f64() * 1e3
    );

    for n in 0..20_000 {
        let e = world.spawn(EcsTransform::default()).id();
        host.insert_component(
            &mut world,
            e,
            "SampleVelocity",
            &json!({ "linear": [1, 0, 0] }),
        )
        .expect("velocity");
        host.insert_component(
            &mut world,
            e,
            "Mass",
            &json!({ "value": if n % 2 == 0 { 1 } else { 0 } }),
        )
        .expect("mass");
    }
    let probe = world.spawn_empty().id();
    host.insert_component(&mut world, probe, "BenchProbe", &json!({}))
        .expect("probe");

    let visible = Arc::new(AtomicI64::new(i64::MIN));
    let done = Arc::new(AtomicBool::new(false));
    let editor = {
        let (visible, done, file) = (visible.clone(), done.clone(), dir.join("Probe.cs"));
        std::thread::spawn(move || {
            let mut results: Vec<(u32, f64)> = Vec::new();
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
            let mut long = false;
            // Edit 0 makes the cold module hot (one L3); then L1, L2 and L3 edits rotate.
            for e in 0..=edits as i64 {
                let kind = if e == 0 {
                    RELOAD_L3
                } else {
                    [RELOAD_L1, RELOAD_L2, RELOAD_L3][(e as usize - 1) % 3]
                };
                text = text.replace(&format!("Shape({e});"), &format!("Shape({});", e + 1));
                if kind == RELOAD_L2 {
                    text = text.replace(
                        "// @pad",
                        &format!("static int Pad{e}() => {e};\n    // @pad"),
                    );
                }
                if kind == RELOAD_L3 && e > 0 {
                    let (from, to) = if long {
                        (SHAPE_LONG, SHAPE_INT)
                    } else {
                        (SHAPE_INT, SHAPE_LONG)
                    };
                    text = text.replace(from, to);
                    long = !long;
                }
                std::fs::write(&file, &text).expect("save");
                let saved = Instant::now();
                match wait(e + 1) {
                    Some(seen) if e > 0 => results.push((kind, (seen - saved).as_secs_f64() * 1e3)),
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
    let mut reports: Vec<(ReloadReport, f64)> = Vec::new();
    let mut ticks: Vec<f64> = Vec::new();
    let mut boundaries: Vec<f64> = Vec::new();
    let mut next = Instant::now();
    let cpu = Cpu::now();
    while !done.load(Ordering::Acquire) {
        let start = Instant::now();
        let b = host.tick_boundary(&mut world);
        if b.reloaded {
            boundaries.push(start.elapsed().as_secs_f64() * 1e3);
        }
        let tick_start = Instant::now();
        host.run_tick(
            &mut world,
            Tick {
                frame,
                seed: 1,
                elapsed: frame as f64 * dt,
                dt: dt as f32,
                reserved: 0,
            },
        );
        let tick_ms = tick_start.elapsed().as_secs_f64() * 1e3;
        reports.extend(b.reports.into_iter().map(|r| (r, tick_ms)));
        ticks.push(start.elapsed().as_secs_f64() * 1e3);
        let v = host
            .read_component(&world, probe, "BenchProbe")
            .and_then(|p| p["value"].as_i64())
            .unwrap_or(-1);
        visible.store(v, Ordering::Release);
        frame += 1;
        next += Duration::from_secs_f64(dt);
        // Sleep most of the gap (high-resolution timers), spin only the last half millisecond.
        if let Some(gap) = next
            .checked_duration_since(Instant::now())
            .and_then(|g| g.checked_sub(Duration::from_micros(500)))
        {
            std::thread::sleep(gap);
        }
        while Instant::now() < next {
            std::hint::spin_loop();
        }
    }
    let busy = cpu.busy_since();
    let results = editor.join().expect("editor thread");
    // Reports and edits pair up in order (the warm-up L3 is the first report after the initial load).
    let edits_reports: Vec<_> = reports
        .iter()
        .filter(|(r, _)| matches!(r.kind, RELOAD_L1 | RELOAD_L2 | RELOAD_L3))
        .skip(1)
        .collect();
    let samples: Vec<Sample> = results
        .iter()
        .zip(&edits_reports)
        .map(|(&(kind, total_ms), &&(report, first_tick_ms))| Sample {
            kind,
            total_ms,
            report,
            first_tick_ms,
        })
        .collect();
    let mismatched = samples.iter().filter(|s| s.kind != s.report.kind).count();
    println!(
        "edits: {} measured, {} took a different level than intended; system CPU busy {:.0}% during the run",
        samples.len(),
        mismatched,
        busy * 100.0
    );
    print_table(&samples);
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
    let errors: Vec<_> = host
        .take_diagnostics()
        .into_iter()
        .filter(|d| d.is_error())
        .collect();
    if !errors.is_empty() {
        println!("errors: {errors:#?}");
    }
    println!("leaked generations at shutdown: {}", host.shutdown());
    let _ = std::fs::remove_dir_all(dir);
}

/// Microseconds to milliseconds.
fn us(v: u32) -> f64 {
    f64::from(v) / 1e3
}

/// Prints p50/p95 of every phase for each reload level (milliseconds).
fn print_table(samples: &[Sample]) {
    type Phase = (&'static str, fn(&Sample) -> f64);
    let phases: [Phase; 13] = [
        ("edit detect", |s| us(s.report.detect_us)),
        ("parse+classify", |s| us(s.report.parse_us)),
        ("generators", |s| us(s.report.generate_us)),
        ("emit", |s| us(s.report.emit_us)),
        ("analyzers", |s| us(s.report.analyze_us)),
        ("transfer", |s| us(s.report.transfer_us)),
        ("load (AMSI)", |s| us(s.report.load_us)),
        ("pre-JIT", |s| us(s.report.prejit_us)),
        ("register", |s| us(s.report.register_us)),
        ("wait for boundary", |s| {
            (s.report.t_applied.saturating_sub(s.report.t_staged) as f64 / 1e6
                - f64::from(s.report.apply_us) / 1e3)
                .max(0.0)
        }),
        ("state migration", |s| us(s.report.apply_us)),
        ("first tick", |s| s.first_tick_ms),
        ("save→visible", |s| s.total_ms),
    ];
    let levels = [("L1", RELOAD_L1), ("L2", RELOAD_L2), ("L3", RELOAD_L3)];
    print!("{:<20}", "phase (ms)");
    for (name, kind) in levels {
        let n = samples.iter().filter(|s| s.report.kind == kind).count();
        print!("{:>22}", format!("{name} p50 / p95 (n={n})"));
    }
    println!();
    for (name, f) in phases {
        print!("{name:<20}");
        for (_, kind) in levels {
            let mut v: Vec<f64> = samples
                .iter()
                .filter(|s| s.report.kind == kind)
                .map(f)
                .collect();
            let p50 = percentile(&mut v, 0.5);
            let p95 = percentile(&mut v, 0.95);
            print!("{:>22}", format!("{p50:.1} / {p95:.1}"));
        }
        println!();
    }
}
