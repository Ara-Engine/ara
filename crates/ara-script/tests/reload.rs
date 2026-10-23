//! Hot reload through the out-of-process compile server: L3 (a cold module's first
//! edit), L1 (EnC body edits), escalation of closures, a component layout change with
//! column migration, compile and analyzer errors (the last good code keeps running),
//! and a faulted system re-enabled by its L1 fix. ECS state survives every reload.

mod support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ara_script::abi::{ESC_CLOSURE, ESC_COLD, RELOAD_ERROR, RELOAD_L1, RELOAD_L3, ReloadReport, Tick};
use ara_script::{ModuleSpec, ScriptConfig, ScriptHost, codes};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde_json::{Value, json};

struct Session {
    host: ScriptHost,
    world: World,
    file: PathBuf,
    frame: u64,
    reports: Vec<ReloadReport>,
    counter: Entity,
    marker: Entity,
}

impl Session {
    fn start() -> Self {
        support::build();
        let dir = std::env::temp_dir().join(format!("ara-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp module dir");
        let src = support::dotnet().join("tests/Ara.TestModule");
        for f in ["ara.module.toml", "Probes.cs"] {
            std::fs::copy(src.join(f), dir.join(f)).expect("copy module file");
        }
        let mut world = World::new();
        let (db, engine) = support::engine();
        let mut config = ScriptConfig::jit(support::bin("Ara.Host"));
        config.compile_server = Some(support::bin("Ara.CompileServer").join("Ara.CompileServer.dll"));
        let mut host = ScriptHost::start(config, db, &engine, &mut world).expect("host starts");
        let spec = ModuleSpec::from_dir(&dir).expect("module file");
        host.load_modules(&mut world, &[spec]).unwrap_or_else(|e| panic!("initial compile: {e:#?}"));
        let counter = world.spawn_empty().id();
        host.insert_component(&mut world, counter, "Counter", &json!({})).expect("counter");
        let marker = world.spawn_empty().id();
        host.insert_component(&mut world, marker, "Marker", &json!({})).expect("marker");
        let file = dir.join("Probes.cs");
        Session { host, world, file, frame: 0, reports: Vec::new(), counter, marker }
    }

    fn tick(&mut self) {
        let b = self.host.tick_boundary(&mut self.world);
        self.reports.extend(b.reports);
        let t = Tick { frame: self.frame, seed: 1, elapsed: self.frame as f64 / 60.0, dt: 1.0 / 60.0, reserved: 0 };
        self.host.run_tick(&mut self.world, t);
        self.frame += 1;
    }

    fn read(&self, e: Entity, c: &str) -> Value {
        self.host.read_component(&self.world, e, c).unwrap_or(Value::Null)
    }

    fn marker(&self) -> Value {
        self.read(self.marker, "Marker")["value"].clone()
    }

    fn count(&self) -> i64 {
        self.read(self.counter, "Counter")["value"].as_i64().unwrap_or(-1)
    }

    /// Ticks until `done` holds (or panics after 60 s).
    fn until(&mut self, what: &str, done: impl Fn(&Session) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}; reports: {:?}; diagnostics: {:#?}", self.reports, self.host.take_diagnostics());
            self.tick();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn edit(&self, from: &str, to: &str) {
        let text = std::fs::read_to_string(&self.file).expect("read module file");
        assert!(text.contains(from), "`{from}` not in Probes.cs");
        std::fs::write(&self.file, text.replacen(from, to, 1)).expect("write module file");
    }

    fn last(&self) -> ReloadReport {
        *self.reports.last().expect("a reload report")
    }
}

fn dir_of(p: &Path) -> &Path {
    p.parent().expect("module dir")
}

#[test]
fn reloads_keep_state_and_escalate_conservatively() {
    let _clr = support::clr_lock();
    let mut s = Session::start();
    s.until("the initial code", |s| s.marker() == 1);

    // A cold module's first edit is one L3 that makes it hot.
    let before = s.count();
    s.edit("m.Value = 1;", "m.Value = 2;");
    s.until("L3", |s| s.marker() == 2);
    let r = s.last();
    assert_eq!((r.kind, r.escalation, r.hot), (RELOAD_L3, ESC_COLD, 1), "{r:?}");
    assert!(s.count() > before, "counter state kept across L3");
    assert!(s.host.modules()[0].hot);

    // Body-only edits of a hot module take L1.
    s.edit("m.Value = 2;", "m.Value = 3;");
    s.until("L1", |s| s.marker() == 3);
    assert_eq!(s.last().kind, RELOAD_L1, "{:?}", s.last());

    // A changed body with a lambda takes L3 (S2: lambda deltas crashed the process).
    s.edit("m.Value = 3;", "System.Func<int> f = static () => 4; m.Value = f();");
    s.until("closure L3", |s| s.marker() == 4);
    assert_eq!((s.last().kind, s.last().escalation), (RELOAD_L3, ESC_CLOSURE));

    // A layout change registers a new column and migrates the old values.
    s.edit(
        "    /// <summary>Written every tick by `Mark`.</summary>\n    public int Value;",
        "    /// <summary>Written every tick by `Mark`.</summary>\n    public int Value;\n\n    /// <summary>A field added by a reload.</summary>\n    public int Extra;",
    );
    s.until("migrated layout", |s| s.read(s.marker, "Marker").get("extra").is_some());
    assert_eq!(s.read(s.marker, "Marker"), json!({ "value": 4, "extra": 0 }));

    // A compile error is reported with its location; the last good code keeps running.
    let errors_before = s.reports.iter().filter(|r| r.kind == RELOAD_ERROR).count();
    s.edit("System.Func<int> f = static () => 4; m.Value = f();", "m.Value = 5 +;");
    s.until("the compile error", |s| s.reports.iter().filter(|r| r.kind == RELOAD_ERROR).count() > errors_before);
    let diags = s.host.take_diagnostics();
    let err = diags.iter().find(|d| d.code == codes::COMPILE_ERROR).unwrap_or_else(|| panic!("{diags:#?}"));
    assert!(err.location.file.as_deref().is_some_and(|f| f.ends_with("Probes.cs")) && err.location.line.is_some(), "{err:?}");
    let ticking = s.count();
    s.tick();
    assert!(s.count() > ticking && s.marker() == 4, "old code still runs");

    // An analyzer error (a mutable static) is refused the same way.
    s.edit("m.Value = 5 +;", "m.Value = 6; Hits++; } static int Hits; static void Unused() {");
    s.until("the analyzer error", |s| s.reports.iter().filter(|r| r.kind == RELOAD_ERROR).count() > errors_before + 1);
    assert!(s.host.take_diagnostics().iter().any(|d| d.code.to_string() == "ARA0100"));
    assert_eq!(s.marker(), 4);

    // A throwing system is disabled, then re-enabled by the fix.
    s.edit("m.Value = 6; Hits++; } static int Hits; static void Unused() {", "if (m.Extra == 0) throw new InvalidOperationException(\"oops\"); m.Value = 7;");
    s.until("the fault", |s| s.host.take_diagnostics().iter().any(|d| d.code == codes::SYSTEM_FAULT) || s.marker() == 7);
    s.edit("if (m.Extra == 0) throw new InvalidOperationException(\"oops\"); m.Value = 7;", "m.Value = 8;");
    s.until("the fix", |s| s.marker() == 8);

    let dir = dir_of(&s.file).to_path_buf();
    assert_eq!(s.host.shutdown(), 0, "every generation unloaded");
    let _ = std::fs::remove_dir_all(dir);
}
