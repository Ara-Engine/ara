//! The reload levels against a running game: each kind of edit takes the level the
//! classifier promises (L1 body, L2 additive, L3 structural) and the game behaves as the
//! new code says afterwards, with ECS state and behaviour objects kept. L4 is in
//! `restart.rs` (it needs its own copy of the runtime).

mod support;

use ara_script::abi::{ESC_STRUCTURAL, RELOAD_L1, RELOAD_L2, RELOAD_L3};
use serde_json::json;
use support::Live;

const LEVELS: &str = r#"namespace Levels;

/// <summary>What the running code wrote last.</summary>
[Component]
public partial struct Probe
{
    /// <summary>The value of the running code.</summary>
    public int Value;
}

/// <summary>Counts the ticks it sees.</summary>
public sealed partial class Ticker : Behaviour
{
    /// <summary>Ticks seen (weighted after the L2 edit).</summary>
    [Property] public partial int Seen { get; set; }

    /// <summary>OnSpawn calls: reloads must not spawn objects again.</summary>
    [Property] public partial int Spawns { get; set; }

    /// <inheritdoc/>
    protected override void OnSpawn() => Spawns += 1;

    /// <inheritdoc/>
    protected override void OnTick(in Tick t)
    {
        Seen += 1;
    }
    // @ticker
}

public static partial class Probing
{
    [Query]
    static void Run(ref Probe p)
    {
        p.Value = 1;
    }

    static int Shape(int x) => x;
    // @members
}
// @types
"#;

#[test]
fn every_level_applies_and_the_game_follows_the_new_code() {
    let _clr = support::clr_lock();
    let mut s = Live::start("levels", &[("Levels", LEVELS, &[])]);
    let probe = s.spawn(&[("Probe", json!({}))]);
    let ticker = s.spawn(&[("Ticker", json!({}))]);
    s.until("the initial code", |s| s.int(probe, "Probe", "value") == 1);
    let value = |s: &Live| s.int(probe, "Probe", "value");

    // L1: a body edit.
    s.edit("Levels", "p.Value = 1;", "p.Value = 2;");
    s.until("L1", |s| value(s) == 2);
    assert_eq!(s.last().kind, RELOAD_L1);

    // L2: a new method, called by the edited body.
    s.edit_many(
        "Levels",
        &[
            ("p.Value = 2;", "p.Value = Three();"),
            ("// @members", "static int Three() => 3;\n    // @members"),
        ],
    );
    s.until("L2 new method", |s| value(s) == 3);
    assert_eq!(s.last().kind, RELOAD_L2);

    // L2: a new type.
    s.edit_many(
        "Levels",
        &[
            ("p.Value = Three();", "p.Value = Helpers.Four();"),
            (
                "// @types",
                "public static class Helpers { public static int Four() => 4; }\n// @types",
            ),
        ],
    );
    s.until("L2 new type", |s| value(s) == 4);
    assert_eq!(s.last().kind, RELOAD_L2);

    // L2: a capturing lambda in a new method.
    s.edit_many("Levels", &[
        ("p.Value = Helpers.Four();", "p.Value = Captured(5);"),
        ("// @members", "static int Captured(int k) { System.Func<int> f = () => k; return f(); }\n    // @members"),
    ]);
    s.until("L2 lambda", |s| value(s) == 5);
    assert_eq!(s.last().kind, RELOAD_L2);

    // L2: a field on a class (the behaviour); the live object reads its default first.
    s.edit_many(
        "Levels",
        &[
            ("Seen += 1;", "Seen += 1 + bonus; bonus = 10;"),
            ("// @ticker", "[Transient] int bonus;"),
        ],
    );
    let l2 = s.count(RELOAD_L2);
    s.until("L2 field", |s| s.count(RELOAD_L2) > l2);
    s.tick(); // the first tick on the new code reads the field's default, then sets it
    let a = s.int(ticker, "Ticker", "seen");
    s.tick();
    assert_eq!(
        s.int(ticker, "Ticker", "seen") - a,
        11,
        "the new field is live on the existing object"
    );

    // L3: a signature change; ECS state and behaviour objects survive the swap.
    s.edit_many(
        "Levels",
        &[
            ("p.Value = Captured(5);", "p.Value = Captured(6);"),
            (
                "static int Shape(int x) => x;",
                "static long Shape(long x) => x;",
            ),
        ],
    );
    s.until("L3", |s| value(s) == 6);
    assert_eq!(
        (s.last().kind, s.last().escalation),
        (RELOAD_L3, ESC_STRUCTURAL)
    );
    let seen = s.int(ticker, "Ticker", "seen");
    s.tick();
    assert!(
        s.int(ticker, "Ticker", "seen") > seen,
        "the behaviour keeps counting"
    );
    assert_eq!(s.int(ticker, "Ticker", "spawns"), 1, "no second OnSpawn");

    // L3: a component layout change migrates the column.
    s.edit(
        "Levels",
        "    public int Value;\n}",
        "    public int Value;\n\n    /// <summary>Added by a reload.</summary>\n    public int Extra;\n}",
    );
    s.until("migrated layout", |s| {
        s.host
            .read_component(&s.world, probe, "Probe")
            .is_some_and(|v| v.get("extra").is_some())
    });
    assert_eq!(
        s.host.read_component(&s.world, probe, "Probe"),
        Some(json!({ "value": 6, "extra": 0 }))
    );
    assert_eq!(s.finish(), 0, "every generation unloaded");
}
