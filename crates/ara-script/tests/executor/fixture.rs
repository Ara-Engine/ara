//! Real CLR commands, query refresh and reload across executor selection changes.
use crate::support;
use ara_script::abi::RELOAD_L3;
use bevy_tasks::{ComputeTaskPool, TaskPoolBuilder};
use serde_json::json;

const ONE: &str = r#"
/// <summary>Authoritative count retained across reloads.</summary>
[Component] public partial struct Counter
{
    /// <summary>Number of observed writes.</summary>
    public int Value;
}
/// <summary>Deferred marker that excludes subsequent writer visits.</summary>
[Component] public partial struct Stamp
{
    /// <summary>Value published at the command boundary.</summary>
    public int Value;
}
/// <summary>Independent state that keeps a genuine fork in the schedule.</summary>
[Component] public partial struct IndependentCounter
{
    /// <summary>Number of independent visits.</summary>
    public int Value;
}
/// <summary>Writes each new row once, then publishes its marker.</summary>
public static partial class Writer
{
    [Query, Without<Stamp>]
    static void Run(Entity entity, ref Counter counter, ref Commands commands)
    {
        counter.Value++;
        commands.Add(entity, new Stamp { Value = counter.Value });
    }
}
"#;

const READER: &str = r#"
/// <summary>Dependent writer used to switch between one and two systems.</summary>
public static partial class Reader
{
    [Query(Order = 10), With<Stamp>]
    static void Run(ref Counter counter) { counter.Value += 10; }
}
"#;

const INDEPENDENT: &str = r#"
/// <summary>Runs independently of the counter/stamp dependency chain.</summary>
public static partial class Independent
{
    [Query(Order = 20)]
    static void Run(ref IndependentCounter counter) { counter.Value++; }
}
"#;

/// Fresh process selects a real worker budget before ScriptHost initializes it.
pub fn exercise(workers: usize) {
    let _clr = support::clr_lock();
    let pool = ComputeTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(workers)
            .on_thread_spawn(ara_script::register_tick_thread)
            .build()
    });
    assert_eq!(pool.thread_num(), workers);
    let mut live = support::Live::start("executor-transition", &[("Game", ONE, &[])]);
    let entity = live.spawn(&[("Counter", json!({"value": 0}))]);
    let independent = live.spawn(&[("IndependentCounter", json!({"value": 0}))]);
    live.run();
    assert_eq!(live.int(entity, "Counter", "value"), 1);
    assert_eq!(
        live.int(entity, "Stamp", "value"),
        1,
        "final deferred playback"
    );
    live.run();
    assert_eq!(
        live.int(entity, "Counter", "value"),
        1,
        "new marker refreshes the exclusion query"
    );

    let previous = live.reports.len();
    live.edit("Game", ONE, &format!("{ONE}{READER}"));
    live.until("second system registered", |live| {
        live.reports[previous..].iter().any(|r| r.kind == RELOAD_L3)
    });
    let before = live.int(entity, "Counter", "value");
    live.run();
    assert_eq!(live.int(entity, "Counter", "value"), before + 10);

    // Both systems see the deferred boundary in dependency order in this tick.
    let new_entity = live.spawn(&[("Counter", json!({"value": 0}))]);
    live.run();
    assert_eq!(live.int(new_entity, "Stamp", "value"), 1);
    assert_eq!(live.int(new_entity, "Counter", "value"), 11);

    let previous = live.reports.len();
    let fork = format!("{READER}{INDEPENDENT}");
    live.edit("Game", READER, &fork);
    live.until("independent system registered", |live| {
        live.reports[previous..].iter().any(|r| r.kind == RELOAD_L3)
    });
    let before = live.int(independent, "IndependentCounter", "value");
    let dependent_before = live.int(entity, "Counter", "value");
    live.run();
    assert_eq!(
        live.int(independent, "IndependentCounter", "value"),
        before + 1
    );
    assert_eq!(live.int(entity, "Counter", "value"), dependent_before + 10);
    let fork_entity = live.spawn(&[("Counter", json!({"value": 0}))]);
    live.run();
    assert_eq!(live.int(fork_entity, "Stamp", "value"), 1);
    assert_eq!(live.int(fork_entity, "Counter", "value"), 11);

    let previous = live.reports.len();
    live.edit("Game", &fork, "");
    live.until("dependent and independent systems removed", |live| {
        live.reports[previous..].iter().any(|r| r.kind == RELOAD_L3)
    });
    let before = live.int(entity, "Counter", "value");
    let independent_before = live.int(independent, "IndependentCounter", "value");
    live.run();
    assert_eq!(
        live.int(entity, "Counter", "value"),
        before,
        "removed system stays removed"
    );
    assert_eq!(
        live.int(independent, "IndependentCounter", "value"),
        independent_before
    );
    assert!(
        live.world.get_entity(entity).is_ok(),
        "live handle survives both transitions"
    );
    let final_entity = live.spawn(&[("Counter", json!({"value": 0}))]);
    live.run();
    assert_eq!(live.int(final_entity, "Counter", "value"), 1);
    assert_eq!(live.int(final_entity, "Stamp", "value"), 1);
    assert!(
        live.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        live.diagnostics
    );
    assert_eq!(live.finish(), 0, "all code generations unload");
}
