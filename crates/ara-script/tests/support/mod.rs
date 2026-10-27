//! Shared setup for the CLR-backed tests: builds `dotnet/` once per test binary, binds
//! the engine `Transform`, and serializes access to the one .NET runtime per process.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

use ara_schema::SchemaDb;
use ara_script::{EngineBindings, bind_component};
use bevy_ecs::component::Component;

/// The ECS `Transform` the tests bind: the schema's canonical layout of
/// `ara_components::Transform` (position, Euler rotation in degrees, scale).
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
#[repr(C)]
pub struct EcsTransform {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

/// The repository's `dotnet/` folder.
pub fn dotnet() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dotnet")
}

/// A project's Release output directory.
pub fn bin(project: &str) -> PathBuf {
    dotnet().join("artifacts/bin").join(project).join("release")
}

/// Builds the solution (Release) once per test process.
pub fn build() {
    if std::env::var_os("ARA_DOTNET_PREBUILT").is_some() {
        return;
    }
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let out = Command::new("dotnet")
            .args(["build", "Ara.slnx", "-c", "Release", "--nologo", "-v", "q"])
            .current_dir(dotnet())
            .output()
            .expect("run `dotnet build` (is the .NET 10 SDK installed?)");
        assert!(
            out.status.success(),
            "dotnet build failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    });
}

/// Publishes the NativeAOT sample (`Outpost.Sample.Native`) once per test process and
/// returns the library path.
pub fn publish_aot() -> PathBuf {
    static PUBLISHED: OnceLock<PathBuf> = OnceLock::new();
    PUBLISHED
        .get_or_init(|| {
            let rid = if cfg!(windows) {
                "win-x64"
            } else if cfg!(target_os = "macos") {
                "osx-arm64"
            } else {
                "linux-x64"
            };
            let mut cmd = Command::new("dotnet");
            cmd.args([
                "publish",
                "samples/Outpost.Sample.Aot",
                "-c",
                "Release",
                "-r",
                rid,
                "--nologo",
                "-v",
                "q",
            ])
            .current_dir(dotnet());
            if cfg!(windows) {
                // VS 2026's vcvarsall expects vswhere.exe on PATH; the NativeAOT linker probe runs it.
                let installer = r"C:\Program Files (x86)\Microsoft Visual Studio\Installer";
                let path = std::env::var_os("PATH").unwrap_or_default();
                let mut paths: Vec<PathBuf> = std::env::split_paths(&path).collect();
                paths.push(PathBuf::from(installer));
                cmd.env("PATH", std::env::join_paths(paths).expect("PATH"));
            }
            let out = cmd.output().expect("run `dotnet publish`");
            assert!(
                out.status.success(),
                "NativeAOT publish failed:\n{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let lib = if cfg!(windows) {
                "Outpost.Sample.Native.dll"
            } else if cfg!(target_os = "macos") {
                "Outpost.Sample.Native.dylib"
            } else {
                "Outpost.Sample.Native.so"
            };
            dotnet()
                .join("artifacts/publish/Outpost.Sample.Aot")
                .join(format!("release_{rid}"))
                .join(lib)
        })
        .clone()
}

/// One .NET runtime per process: CLR tests take this lock.
pub fn clr_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The engine schema and bindings the generated C# was built for.
pub fn engine() -> (SchemaDb, EngineBindings) {
    let db = ara_runtime::schema(&[]).expect("engine schema");
    let bindings = bind_component!(EngineBindings::new(), EcsTransform as "Transform" { position, rotation, scale });
    assert_eq!(bindings.names(), EngineBindings::builtin().names());
    (db, bindings)
}

/// A host with a compile server over modules written to a temp folder (one subfolder per
/// module), ticked by the test. Tests edit the sources and tick until the change shows.
pub struct Live {
    pub host: ara_script::ScriptHost,
    pub world: bevy_ecs::world::World,
    pub root: PathBuf,
    pub frame: u64,
    pub reports: Vec<ara_script::abi::ReloadReport>,
    pub diagnostics: Vec<ara_diag::Diagnostic>,
}

impl Live {
    /// Writes each `(name, source, references)` module and loads them all.
    pub fn start(tag: &str, modules: &[(&str, &str, &[&str])]) -> Live {
        let root = Self::write(tag, modules);
        let mut live = Self::host(root);
        let specs = modules
            .iter()
            .map(|(name, _, _)| {
                ara_script::ModuleSpec::from_dir(&live.root.join(name)).expect("module file")
            })
            .collect::<Vec<_>>();
        live.host
            .load_modules(&mut live.world, &specs)
            .unwrap_or_else(|e| panic!("initial compile: {e:#?}"));
        live
    }

    /// Writes module folders under a fresh temp root.
    pub fn write(tag: &str, modules: &[(&str, &str, &[&str])]) -> PathBuf {
        build();
        let root = std::env::temp_dir().join(format!("ara-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, source, refs) in modules {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).expect("module dir");
            let refs: Vec<String> = refs.iter().map(|r| format!("\"{r}\"")).collect();
            std::fs::write(
                dir.join(ara_script::MODULE_FILE),
                format!(
                    "[module]\nname = \"{name}\"\nhot = true\nreferences = [{}]\n",
                    refs.join(", ")
                ),
            )
            .expect("module file");
            std::fs::write(dir.join("Code.cs"), source).expect("module source");
        }
        root
    }

    /// A JIT host with the compile server; modules are loaded by the caller.
    pub fn host(root: PathBuf) -> Live {
        let mut world = bevy_ecs::world::World::new();
        let (db, engine) = engine();
        let mut config = ara_script::ScriptConfig::jit(bin("Ara.Host"));
        config.compile_server = Some(bin("Ara.CompileServer").join("Ara.CompileServer.dll"));
        let host =
            ara_script::ScriptHost::start(config, db, &engine, &mut world).expect("host starts");
        Live {
            host,
            world,
            root,
            frame: 0,
            reports: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /// One tick: the boundary (staged reloads apply), then the systems and behaviours.
    pub fn tick(&mut self) -> bool {
        let b = self.host.tick_boundary(&mut self.world);
        self.reports.extend(b.reports);
        self.run();
        b.reloaded
    }

    /// Runs the systems and behaviours without a boundary (nothing staged applies).
    pub fn run(&mut self) {
        let t = ara_script::abi::Tick {
            frame: self.frame,
            seed: 1,
            elapsed: self.frame as f64 / 60.0,
            dt: 1.0 / 60.0,
            reserved: 0,
        };
        self.host.run_tick(&mut self.world, t);
        self.frame += 1;
        self.diagnostics.extend(self.host.take_diagnostics());
    }

    /// Ticks until `done` holds (or panics after 60 s).
    pub fn until(&mut self, what: &str, done: impl Fn(&Live) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !done(self) {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}; reports: {:#?}; diagnostics: {:#?}",
                self.reports,
                self.diagnostics
            );
            self.tick();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// The path of a module's source.
    pub fn file(&self, module: &str) -> PathBuf {
        self.root.join(module).join("Code.cs")
    }

    /// Replaces the first `from` with `to` in a module's source.
    pub fn edit(&self, module: &str, from: &str, to: &str) {
        let path = self.file(module);
        let text = std::fs::read_to_string(&path).expect("read module source");
        assert!(text.contains(from), "`{from}` not in {module}");
        std::fs::write(&path, text.replacen(from, to, 1)).expect("write module source");
    }

    /// Applies several replacements to a module's source in one save.
    pub fn edit_many(&self, module: &str, edits: &[(&str, &str)]) {
        let path = self.file(module);
        let mut text = std::fs::read_to_string(&path).expect("read module source");
        for (from, to) in edits {
            assert!(text.contains(from), "`{from}` not in {module}");
            text = text.replacen(from, to, 1);
        }
        std::fs::write(&path, text).expect("write module source");
    }

    /// Spawns an entity with script components (JSON values).
    pub fn spawn(&mut self, components: &[(&str, serde_json::Value)]) -> bevy_ecs::entity::Entity {
        let e = self.world.spawn_empty().id();
        for (name, v) in components {
            self.host
                .insert_component(&mut self.world, e, name, v)
                .unwrap_or_else(|d| panic!("insert {name}: {d:?}"));
        }
        e
    }

    /// A component field as an integer (-1 if missing).
    pub fn int(&self, e: bevy_ecs::entity::Entity, component: &str, field: &str) -> i64 {
        self.host
            .read_component(&self.world, e, component)
            .and_then(|v| v[field].as_i64())
            .unwrap_or(-1)
    }

    /// Reports of one kind received so far.
    pub fn count(&self, kind: u32) -> usize {
        self.reports.iter().filter(|r| r.kind == kind).count()
    }

    /// The last report received.
    pub fn last(&self) -> ara_script::abi::ReloadReport {
        *self.reports.last().expect("a reload report")
    }

    /// Shuts the host down, removes the modules and returns the leaked generations.
    pub fn finish(self) -> usize {
        let leaked = self.host.shutdown();
        let _ = std::fs::remove_dir_all(&self.root);
        leaked
    }
}
