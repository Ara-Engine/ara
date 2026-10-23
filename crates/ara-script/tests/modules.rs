//! Cross-module references against a running game: module Game uses module Core's
//! component and functions. Core's body edits stay local (L1); Core's new generations
//! cascade to Game (relinked when Core's surface is unchanged, rebuilt otherwise) in one
//! swap with consistent state; an edit to Core that breaks Game applies nowhere. Cycles
//! and unknown references are refused at load.

mod support;

use std::time::{Duration, Instant};

use ara_script::abi::{
    ESC_CLOSURE, ESC_DEPENDENCY, ESC_RELINK, RELOAD_ERROR, RELOAD_L1, RELOAD_L3,
};
use ara_script::{ModuleSpec, codes};
use bevy_ecs::entity::Entity;
use serde_json::json;
use support::Live;

const CORE: &str = r#"namespace Core;

/// <summary>Health every module shares.</summary>
[Component]
public partial struct Health
{
    /// <summary>Points.</summary>
    public int Value;
}

/// <summary>Rules other modules call.</summary>
public static class Rules
{
    /// <summary>Points one regeneration tick adds.</summary>
    public static int Heal() { return 1; }

    /// <summary>Not used by Game.</summary>
    public static int Unused(int x) => x;
}
"#;

const GAME: &str = r#"namespace Game;

public static partial class Regen
{
    [Query]
    static void Run(ref Core.Health h)
    {
        h.Value += Core.Rules.Heal();
    }
}
"#;

/// Ticks twice and returns the per-tick rate.
fn rate(s: &mut Live, e: Entity) -> i64 {
    s.tick();
    let a = s.int(e, "Health", "value");
    s.tick();
    s.int(e, "Health", "value") - a
}

fn wait_rate(s: &mut Live, e: Entity, want: i64, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while rate(s, e) != want {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; reports {:#?}; diagnostics {:#?}",
            s.reports,
            s.diagnostics
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn edits_cascade_to_dependents_in_one_consistent_swap() {
    let _clr = support::clr_lock();
    let mut s = Live::start("modules", &[("Core", CORE, &[]), ("Game", GAME, &["Core"])]);
    let e = s.spawn(&[("Health", json!({ "value": 100 }))]);
    wait_rate(&mut s, e, 1, "the initial code");

    // A body edit in Core is an L1 delta of Core alone; Game calls the new body at once.
    s.edit("Core", "return 1;", "return 2;");
    wait_rate(&mut s, e, 2, "Core L1");
    let r = s.last();
    assert_eq!((r.module, r.kind, r.cascade), (0, RELOAD_L1, 1), "{r:?}");

    // An L3 of Core that keeps its surface relinks Game: both swap at one boundary.
    let before = s.reports.len();
    s.edit(
        "Core",
        "return 2;",
        "System.Func<int> f = static () => 3; return f();",
    );
    wait_rate(&mut s, e, 3, "Core L3 with Game relinked");
    let swap: Vec<_> = s.reports[before..]
        .iter()
        .map(|r| (r.module, r.kind, r.escalation, r.cascade))
        .collect();
    assert_eq!(
        swap,
        [
            (0, RELOAD_L3, ESC_CLOSURE, 2),
            (1, RELOAD_L3, ESC_RELINK, 2)
        ]
    );
    assert!(
        s.int(e, "Health", "value") > 100,
        "state kept across the cascade"
    );

    // A surface change rebuilds Game against the new Core.
    let before = s.reports.len();
    s.edit_many(
        "Core",
        &[
            (
                "public static int Unused(int x) => x;",
                "public static long Unused(long x) => x;",
            ),
            (
                "System.Func<int> f = static () => 3; return f();",
                "return 4;",
            ),
        ],
    );
    wait_rate(&mut s, e, 4, "Core L3 with Game rebuilt");
    let swap: Vec<_> = s.reports[before..]
        .iter()
        .map(|r| (r.module, r.kind, r.escalation))
        .collect();
    // Core's first escalation reason is the old lambda in the edited body; Game is rebuilt.
    assert_eq!(
        swap,
        [(0, RELOAD_L3, ESC_CLOSURE), (1, RELOAD_L3, ESC_DEPENDENCY)]
    );

    // A layout change of Core's component migrates the column under Game's system too.
    let hp = s.int(e, "Health", "value");
    s.edit(
        "Core",
        "    public int Value;\n}",
        "    public int Value;\n\n    /// <summary>Cap.</summary>\n    public int Max;\n}",
    );
    s.until("Health migrated", |s| {
        s.host
            .read_component(&s.world, e, "Health")
            .is_some_and(|v| v.get("max").is_some())
    });
    assert!(
        s.int(e, "Health", "value") >= hp,
        "values migrated, not reset"
    );
    wait_rate(&mut s, e, 4, "Game runs on the migrated column");

    // An edit to Core that breaks Game applies nowhere: both keep their last good code.
    let errors = s.count(RELOAD_ERROR);
    s.edit(
        "Core",
        "public static int Heal()",
        "public static int Mend()",
    );
    s.until("the dependency error", |s| {
        s.count(RELOAD_ERROR) >= errors + 2
    });
    assert!(
        s.diagnostics
            .iter()
            .any(|d| d.code == codes::DEPENDENCY_FAILED),
        "{:#?}",
        s.diagnostics
    );
    assert_eq!(rate(&mut s, e), 4, "the last good code keeps running");

    // Reverting is a no-op against the last good state; the next edit applies normally.
    s.edit(
        "Core",
        "public static int Mend()",
        "public static int Heal()",
    );
    s.edit("Core", "return 4;", "return 5;");
    wait_rate(&mut s, e, 5, "Core L1 after the failed cascade");
    assert_eq!(s.finish(), 0, "every generation unloaded");
}

#[test]
fn cycles_and_unknown_references_are_refused_at_load() {
    let _clr = support::clr_lock();
    let module = |name: &str| {
        format!(
            "namespace {name};\n\n/// <summary>A component.</summary>\n[Component]\npublic partial struct {name}Thing\n{{\n    /// <summary>A value.</summary>\n    public int Value;\n}}\n"
        )
    };
    let (a, b, c, d) = (module("A"), module("B"), module("C"), module("D"));
    let root = Live::write(
        "cycles",
        &[
            ("A", &a, &["B"]),
            ("B", &b, &["A"]),
            ("C", &c, &[]),
            ("D", &d, &["Missing"]),
        ],
    );
    let mut s = Live::host(root);
    let specs: Vec<_> = ["A", "B", "C", "D"]
        .iter()
        .map(|n| ModuleSpec::from_dir(&s.root.join(n)).expect("module"))
        .collect();
    let problems = s
        .host
        .load_modules(&mut s.world, &specs)
        .expect_err("the cycle is refused");
    let found: Vec<_> = problems.iter().map(|d| d.code).collect();
    assert!(
        found.contains(&codes::MODULE_CYCLE) && found.contains(&codes::UNKNOWN_MODULE),
        "{problems:#?}"
    );
    let loaded: Vec<_> = s.host.modules().into_iter().map(|m| m.name).collect();
    assert_eq!(loaded, ["C"], "modules outside the cycle still load");
    s.finish();
}
