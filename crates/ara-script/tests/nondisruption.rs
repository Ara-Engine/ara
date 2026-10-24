//! D20 non-disruption: compiles never block the editor or the tick thread, reloads apply
//! only at tick boundaries, a failed compile leaves the running game untouched with a
//! structured diagnostic, and agent compiles are coalesced and run behind the human's.

mod support;

use std::time::{Duration, Instant};

use ara_script::abi::{ORIGIN_AGENT, ORIGIN_HUMAN, RELOAD_ERROR, RELOAD_L1, RELOAD_L3};
use ara_script::{EditOrigin, ModuleSpec, codes};
use serde_json::json;
use support::Live;

const DIAL: &str = r#"namespace Dials;

/// <summary>What the running code wrote.</summary>
[Component]
public partial struct Dial
{
    /// <summary>The value of the running code.</summary>
    public int Value;

    /// <summary>Ticks seen.</summary>
    public int Ticks;
}

public static partial class Turning
{
    [Query]
    static void Run(ref Dial d)
    {
        d.Value = 1;
        d.Ticks += 1;
    }

    static int Shape(int x) => x;
}
"#;

#[test]
fn compiles_never_block_the_editor_or_the_tick_thread() {
    let _clr = support::clr_lock();
    let mut s = Live::start("nb-block", &[("Dials", DIAL, &[])]);
    let e = s.spawn(&[("Dial", json!({}))]);
    s.until("the initial code", |s| s.int(e, "Dial", "value") == 1);

    // An L3 compile (a signature change) runs in the compile server; the editor's call
    // returns at once and the game keeps ticking while it compiles.
    s.edit_many(
        "Dials",
        &[
            ("d.Value = 1;", "d.Value = 2;"),
            (
                "static int Shape(int x) => x;",
                "static long Shape(long x) => x;",
            ),
        ],
    );
    let t = Instant::now();
    assert!(
        s.host
            .request_compile("Dials", &[s.file("Dials")], EditOrigin::Human)
    );
    assert!(
        t.elapsed() < Duration::from_millis(20),
        "request_compile returned in {:?}",
        t.elapsed()
    );
    let (mut during, mut worst) = (0, Duration::ZERO);
    let deadline = Instant::now() + Duration::from_secs(60);
    while s.int(e, "Dial", "value") != 2 {
        assert!(
            Instant::now() < deadline,
            "the L3 never applied: {:#?}",
            s.diagnostics
        );
        let compiling = s.host.compiling();
        let t = Instant::now();
        s.tick();
        worst = worst.max(t.elapsed());
        during += usize::from(compiling);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(s.last().kind, RELOAD_L3);
    assert!(
        during >= 3,
        "only {during} ticks ran while the compile server compiled"
    );
    assert!(
        worst < Duration::from_millis(100),
        "a tick took {worst:?} during the compile"
    );
    s.finish();
}

#[test]
fn reloads_apply_only_at_tick_boundaries() {
    let _clr = support::clr_lock();
    let mut s = Live::start("nb-boundary", &[("Dials", DIAL, &[])]);
    let e = s.spawn(&[("Dial", json!({}))]);
    s.until("the initial code", |s| s.int(e, "Dial", "value") == 1);
    let reports = s.reports.len();

    s.edit("Dials", "d.Value = 1;", "d.Value = 2;");
    // Tick without boundaries until the delta is staged, then a while longer: old code runs.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !s.host.pending() {
        assert!(
            Instant::now() < deadline,
            "nothing was staged: {:#?}",
            s.host.take_diagnostics()
        );
        s.run();
        std::thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..5 {
        s.run();
    }
    assert_eq!(
        s.int(e, "Dial", "value"),
        1,
        "staged code does not run before a boundary"
    );
    assert_eq!(s.reports.len(), reports, "nothing applied yet");
    assert!(s.tick(), "the boundary applies it");
    assert_eq!((s.int(e, "Dial", "value"), s.last().kind), (2, RELOAD_L1));
    s.finish();
}

#[test]
fn a_failed_compile_leaves_the_game_untouched() {
    let _clr = support::clr_lock();
    let mut s = Live::start("nb-error", &[("Dials", DIAL, &[])]);
    let e = s.spawn(&[("Dial", json!({}))]);
    s.until("the initial code", |s| s.int(e, "Dial", "value") == 1);
    let generation = s.host.modules()[0].generation;

    s.edit("Dials", "d.Value = 1;", "d.Value = 1 +;");
    let mut reloaded = false;
    let deadline = Instant::now() + Duration::from_secs(60);
    while s.count(RELOAD_ERROR) == 0 {
        assert!(Instant::now() < deadline, "no error report");
        reloaded |= s.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!reloaded, "no code changed");
    let err = s
        .diagnostics
        .iter()
        .find(|d| d.code == codes::COMPILE_ERROR)
        .unwrap_or_else(|| panic!("{:#?}", s.diagnostics));
    let at = &err.location;
    assert!(
        at.file.as_deref().is_some_and(|f| f.ends_with("Code.cs"))
            && at.line.is_some()
            && at.column.is_some(),
        "{err:?}"
    );
    let ticks = s.int(e, "Dial", "ticks");
    s.tick();
    assert_eq!(
        (s.int(e, "Dial", "value"), s.int(e, "Dial", "ticks")),
        (1, ticks + 1),
        "the old code keeps running"
    );
    assert_eq!(s.host.modules()[0].generation, generation);
    s.finish();
}

const WORK: &str = r#"namespace Work;

/// <summary>Sum of the three helpers.</summary>
[Component]
public partial struct Total
{
    /// <summary>A1 + A2 + A3.</summary>
    public int Value;
}

public static partial class Summing
{
    [Query]
    static void Run(ref Total t)
    {
        t.Value = A1() + A2() + A3();
    }

    static int A1() { return 1; }
    static int A2() { return 2; }
    static int A3() { return 3; }
}
"#;

#[test]
fn agent_compiles_are_coalesced_and_run_behind_the_humans() {
    let _clr = support::clr_lock();
    let root = Live::write("nb-agent", &[("Work", WORK, &[]), ("Dials", DIAL, &[])]);
    let mut s = Live::host(root);
    let modules: Vec<_> = ["Work", "Dials"]
        .into_iter()
        .map(|name| {
            let mut module = ModuleSpec::from_dir(&s.root.join(name)).expect("module file");
            module.watch = false;
            module
        })
        .collect();
    s.host
        .load_modules(&mut s.world, &modules)
        .expect("modules");
    let total = s.spawn(&[("Total", json!({}))]);
    let dial = s.spawn(&[("Dial", json!({}))]);
    s.until("the initial code", |s| {
        s.int(total, "Total", "value") == 6 && s.int(dial, "Dial", "value") == 1
    });

    // Exercise the explicit queue API: watcher delivery can arrive after an agent's
    // quiet window on a busy machine. The scheduler's own tests cover preemption.
    let work = s.file("Work");
    s.edit_many(
        "Work",
        &[
            ("return 1;", "return 10;"),
            ("return 2;", "return 20;"),
            ("return 3;", "return 30;"),
        ],
    );
    s.edit("Dials", "d.Value = 1;", "d.Value = 2;");
    assert!(
        s.host
            .request_compile("Dials", &[s.file("Dials")], EditOrigin::Human)
    );
    for _ in 0..3 {
        assert!(
            s.host
                .request_compile("Work", std::slice::from_ref(&work), EditOrigin::Agent)
        );
    }
    s.until("both edits", |s| {
        s.int(total, "Total", "value") == 60 && s.int(dial, "Dial", "value") == 2
    });
    let order: Vec<_> = s
        .reports
        .iter()
        .filter(|r| r.kind == RELOAD_L1)
        .map(|r| (r.module, r.origin, r.changed_members))
        .collect();
    assert_eq!(
        order,
        [(1, ORIGIN_HUMAN, 1), (0, ORIGIN_AGENT, 3)],
        "the human's reload first; the agent's three edits as one"
    );
    s.finish();
}
