//! `cargo run -p ara-bench --release --example scenario -- script.json output.araprof`
use ara_bench::{Scenario, ScenarioConfig};
use ara_profile::{Config, Counters, Profiler, ThreadRegistration};
use ara_sim::{EditWorld, Session, SimRegistry, TickConfig};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracing_subscriber::prelude::*;

#[global_allocator]
static ALLOCATOR: ara_profile::CountingAllocator<std::alloc::System> =
    ara_profile::CountingAllocator(std::alloc::System);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Script {
    project: PathBuf,
    scene: String,
    seed: u64,
    run: ScenarioConfig,
}
struct Ticks<'a> {
    script: &'a Script,
    root: PathBuf,
    session: Option<Session>,
}
impl Scenario for Ticks<'_> {
    fn reset(&mut self) -> Result<(), String> {
        let project = ara_host::Project::open(&self.root).map_err(|e| e.to_string())?;
        let (workspace, diagnostics) = project.load(&[]);
        if let Some(error) = diagnostics.values().flatten().find(|d| d.is_error()) {
            return Err(error.to_string());
        }
        let scene = workspace
            .docs()
            .find(|(path, _)| *path == self.script.scene)
            .map(|(_, doc)| doc.id.clone())
            .ok_or("scene is not loaded by the project")?;
        let registry = SimRegistry::new(project.db).map_err(|e| e.to_string())?;
        let world =
            EditWorld::from_workspace(registry, workspace, scene).map_err(|e| e.to_string())?;
        let mut session = Session::local(
            world,
            TickConfig::default(),
            self.script.seed,
            ara_input::ActionMap::default(),
        )
        .map_err(|e| e.to_string())?;
        session
            .add_system("scenario.tick", || {})
            .map_err(|e| e.to_string())?;
        self.session = Some(session);
        Ok(())
    }
    fn frame(&mut self) -> Result<Counters, String> {
        let session = self
            .session
            .as_mut()
            .ok_or("scenario must be reset before measurement")?;
        session.step().map_err(|e| e.to_string())?;
        Ok(Counters {
            entities: u64::from(session.world().entities().len()),
            ..Counters::default()
        })
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("expected script.json output.araprof".into());
    }
    let script_path = Path::new(&args[0]);
    let script: Script =
        serde_json::from_value(ara_json::parse(&fs::read_to_string(script_path)?)?)?;
    let root = script_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(&script.project);
    let capacity = (script.run.frames as usize)
        .checked_mul(script.run.rounds as usize)
        .and_then(|n| n.checked_mul(2))
        .ok_or("scenario frame count overflow")?;
    let mut profiler = Profiler::new(Config {
        history: capacity,
        ..Config::default()
    })?;
    profiler.register_thread("scenario").attach()?;
    profiler.start();
    let subscriber = tracing_subscriber::registry().with(ara_profile::ProfileLayer);
    let mut a = Ticks {
        script: &script,
        root: root.to_owned(),
        session: None,
    };
    let mut b = Ticks {
        script: &script,
        root,
        session: None,
    };
    let samples = tracing::subscriber::with_default(subscriber, || {
        ara_bench::interleave(&mut a, &mut b, &mut profiler, script.run)
    })?;
    profiler.stop();
    profiler.collect();
    ThreadRegistration::detach();
    let output = Path::new(&args[1]);
    ara_profile::write_capture(output, profiler.profile())?;
    for variant in ["a", "b"] {
        let mut profile = profiler.profile().clone();
        profile.frames.retain(|f| {
            samples
                .iter()
                .any(|s| s.variant == variant && (s.first_frame..=s.last_frame).contains(&f.id))
        });
        let summary = profile.summary(ara_profile::SummaryBudget {
            top_n: 1024,
            hitches: 0,
            max_bytes: 1_048_576,
        })?;
        ara_profile::write_summary(&output.with_extension(format!("{variant}.json")), &summary)?;
    }
    fs::write(
        output.with_extension("runs.json"),
        ara_json::to_canonical_string(&serde_json::to_value(samples)?),
    )?;
    Ok(())
}
