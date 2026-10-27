//! Reload queues, module generations and callback contexts belong to individual worlds.
mod support;

use ara_script::abi::{RELOAD_L1, RELOAD_L3};
use serde_json::json;
use support::Live;

const SOURCE: &str = r#"namespace Separate;
/// <summary>World-local simulation state.</summary>
[Component] public partial struct Counter {
    /// <summary>The number of simulated steps.</summary>
    public int Ticks;
    // fields
}
public static partial class Count {
    [Query] static void Run(ref Counter c) => c.Ticks += 1;
}
/// <summary>Exercises the world's callback context alongside its native columns.</summary>
public sealed partial class Lamp : Behaviour {
    /// <summary>Visits in this world only.</summary>
    [Property] public partial int Visits { get; set; }
    /// <inheritdoc/>
    protected override void OnTick(in Tick tick) {
        Visits += 1;
        Get<Light>().Intensity += 1;
    }
}
"#;

#[test]
fn two_worlds_reload_independently_and_the_survivor_can_reload_again() {
    let _clr = support::clr_lock();
    let mut first = Live::start("world-reload-first", &[("Separate", SOURCE, &[])]);
    let mut second = Live::start("world-reload-second", &[("Separate", SOURCE, &[])]);
    let a = first.spawn(&[
        ("Counter", json!({"ticks":10})),
        ("Lamp", json!({})),
        ("Light", json!({"intensity":100})),
    ]);
    let b = second.spawn(&[
        ("Counter", json!({"ticks":1000})),
        ("Lamp", json!({})),
        ("Light", json!({"intensity":200})),
    ]);
    assert_eq!(a, b, "entity handles intentionally alias across worlds");
    first.run();
    second.run();
    assert_eq!(first.int(a, "Counter", "ticks"), 11);
    assert_eq!(second.int(b, "Counter", "ticks"), 1001);
    first.edit("Separate", "c.Ticks += 1", "c.Ticks += 3");
    first.until("first world's L1", |s| s.count(RELOAD_L1) > 0);
    let before = first.int(a, "Counter", "ticks");
    first.run();
    assert_eq!(first.int(a, "Counter", "ticks"), before + 3);
    assert_eq!(second.int(b, "Counter", "ticks"), 1001);
    assert_eq!(second.int(b, "Lamp", "visits"), 1);
    assert!(
        second.reports.is_empty(),
        "first world's reports never enter the second queue"
    );
    first.edit_many("Separate", &[
        ("// fields", "/// <summary>Added only in the first world.</summary>\n public int Added;\n // fields"),
        ("c.Ticks += 3", "c.Ticks += 5"),
    ]);
    first.until("first world's L3", |s| s.count(RELOAD_L3) > 0);
    let before = first.int(a, "Counter", "ticks");
    first.run();
    assert_eq!(first.int(a, "Counter", "ticks"), before + 5);
    assert_eq!(first.int(a, "Counter", "added"), 0);
    assert!(
        second
            .host
            .schema()
            .get("Counter")
            .expect("second schema")
            .field("added")
            .is_none()
    );
    second.run();
    assert_eq!(second.int(b, "Counter", "ticks"), 1002);
    assert_eq!(second.int(b, "Lamp", "visits"), 2);
    assert_eq!(
        second
            .host
            .read_component(&second.world, b, "Light")
            .expect("light")["intensity"],
        202.0
    );
    assert_eq!(first.finish(), 0);
    second.edit("Separate", "c.Ticks += 1", "c.Ticks += 7");
    second.until("survivor's own L1", |s| s.count(RELOAD_L1) > 0);
    let before = second.int(b, "Counter", "ticks");
    second.run();
    assert_eq!(second.int(b, "Counter", "ticks"), before + 7);
    assert!(
        second.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        second.diagnostics
    );
    assert_eq!(second.finish(), 0);
}
