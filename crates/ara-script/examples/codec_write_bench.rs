//! Real Rust/C# engine JSON accessor reads, writes and callback flushes.
//! Use a prebuilt solution, private CoreCLR and an optimized Rust profile.
//! --case light|audio|graph, --rows, --parameters, --warmup and --samples.
#[path = "../tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

struct Allocations;
static TRACK: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: all allocation operations are delegated unchanged to the system allocator.
unsafe impl GlobalAlloc for Allocations {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        // SAFETY: the caller supplies the GlobalAlloc layout contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the pointer/layout contract is forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size, Ordering::Relaxed);
        }
        // SAFETY: the pointer/layout/new-size contract is forwarded unchanged.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocations = Allocations;

fn arg(name: &str, default: usize) -> usize {
    std::env::args()
        .skip_while(|a| a != name)
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let case = std::env::args()
        .skip_while(|a| a != "--case")
        .nth(1)
        .unwrap_or_else(|| "graph".into());
    let hot = std::env::args().any(|a| a == "--hot");
    let rows = arg("--rows", 2048).max(1);
    let parameters = arg("--parameters", 8).max(1);
    let warmup = arg("--warmup", 12);
    let samples = arg("--samples", 32).max(1);
    let (component, field, statement, initial) = match case.as_str() {
        "light" => (
            "Light",
            "intensity",
            "value.Intensity += 0.125f;",
            json!({"intensity":1.0}),
        ),
        "audio" => (
            "AudioSource",
            "gain",
            "value.Gain += 0.125f;",
            json!({"gain":1.0,"bus":"effects/spatial"}),
        ),
        "graph" => {
            let parameters = (0..parameters)
                .map(|i| (format!("parameter{i}"), json!(1.0)))
                .collect::<serde_json::Map<_, _>>();
            (
                "AudioGraphSource",
                "parameters",
                "value.Parameters[\"parameter0\"] += 0.125f;",
                json!({"parameters":parameters,"triggers":{"pulse":3}}),
            )
        }
        _ => panic!("unknown case {case}"),
    };
    // One exclusive gameplay callback touches distinct entities; cache hits cannot
    // hide JSON reads or flushes. ECS owns every authoritative component value.
    let source = format!(
        r#"
/// <summary>Updates a batch of nonnative engine components.</summary>
public sealed partial class Driver : Behaviour {{
    protected override void OnTick(in Tick tick) {{
        Span<Entity> scratch = stackalloc Entity[{rows}];
        foreach (var entity in World.Query<{component}>(scratch)) {{
            ref var value = ref World.Get<{component}>(entity);
            {statement}
        }}
    }}
}}
"#
    );
    let root = support::Live::write("codec-write-bench", &[("Game", &source, &[])]);
    if !hot {
        let manifest = root.join("Game/ara.module.toml");
        let text = std::fs::read_to_string(&manifest).expect("manifest");
        std::fs::write(manifest, text.replace("hot = true", "hot = false")).expect("cold module");
    }
    let mut live = support::Live::host(root);
    let spec = ara_script::ModuleSpec::from_dir(&live.root.join("Game")).expect("module spec");
    live.host
        .load_modules(&mut live.world, &[spec])
        .expect("compile kernel");
    live.spawn(&[("Driver", json!({}))]);
    let entities = (0..rows)
        .map(|_| live.spawn(&[(component, initial.clone())]))
        .collect::<Vec<_>>();
    for _ in 0..warmup {
        live.run();
    }
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        live.run();
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    // Count one separate warmed tick so the atomic counters cannot inflate timings.
    TRACK.store(true, Ordering::Relaxed);
    live.run();
    TRACK.store(false, Ordering::Relaxed);
    let expected = 1.0f32 + (warmup + samples + 1) as f32 * 0.125;
    for entity in entities {
        let value = live
            .host
            .read_component(&live.world, entity, component)
            .expect("component");
        let actual = if case == "graph" {
            &value[field]["parameter0"]
        } else {
            &value[field]
        };
        assert_eq!(
            (actual.as_f64().expect("numeric") as f32).to_bits(),
            expected.to_bits()
        );
    }
    assert!(
        live.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        live.diagnostics
    );
    let mut sorted = timings.clone();
    sorted.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({"case":case,"rows":rows,"parameters":parameters,"warmup":warmup,
        "csharp_hot":hot,"samples":samples,"samples_ms":timings,"p50_ms":sorted[samples/2],
        "p95_ms":sorted[(samples*95/100).min(samples-1)],"expected_bits":expected.to_bits(),
        "rust_allocations_per_tick":COUNT.load(Ordering::Relaxed),
        "rust_allocated_bytes_per_tick":BYTES.load(Ordering::Relaxed)})
    );
    assert_eq!(live.finish(), 0, "no leaked generations");
}
