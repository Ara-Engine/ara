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
            let rid = if cfg!(windows) { "win-x64" } else if cfg!(target_os = "macos") { "osx-arm64" } else { "linux-x64" };
            let mut cmd = Command::new("dotnet");
            cmd.args(["publish", "samples/Outpost.Sample.Aot", "-c", "Release", "-r", rid, "--nologo", "-v", "q"])
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
            let lib = if cfg!(windows) { "Outpost.Sample.Native.dll" } else if cfg!(target_os = "macos") { "Outpost.Sample.Native.dylib" } else { "Outpost.Sample.Native.so" };
            dotnet().join("artifacts/publish/Outpost.Sample.Aot").join(format!("release_{rid}")).join(lib)
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
    let mut db = SchemaDb::new();
    ara_components::register(&mut db);
    let bindings = bind_component!(EngineBindings::new(), EcsTransform as "Transform" { position, rotation, scale });
    (db, bindings)
}
