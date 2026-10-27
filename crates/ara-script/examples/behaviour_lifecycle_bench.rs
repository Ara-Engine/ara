//! Fixed-tick behaviour membership cost, with all state in ECS components.
//! Run with ARA_DOTNET_PREBUILT=1 after the solution prebuild. Output is JSON;
//! redirect it under target/ when comparing revisions on the same machine.
#[path = "../tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::time::Instant;

fn main() {
    let mut live = support::Live::start(
        "lifecycle-bench",
        &[(
            "Game",
            r#"
/// <summary>A steady-state behaviour with authoritative data in its ECS column.</summary>
public sealed partial class Counter : Behaviour
{
    /// <summary>Number of completed fixed ticks.</summary>
    [Property] public partial int Value { get; set; }
    protected override void OnTick(in Tick tick) => Value++;
}
"#,
            &[],
        )],
    );
    let entities = 8192;
    for _ in 0..entities {
        live.spawn(&[("Counter", json!({}))]);
    }
    for _ in 0..100 {
        live.tick();
    }
    let mut micros = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let start = Instant::now();
        live.tick();
        micros.push(start.elapsed().as_secs_f64() * 1_000_000.);
    }
    micros.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({"entities":entities,"ticks":micros.len(),"p50Micros":micros[500],"p95Micros":micros[950]})
    );
    assert!(live.diagnostics.is_empty(), "{:?}", live.diagnostics);
    assert_eq!(live.finish(), 0, "no leaked generations");
}
