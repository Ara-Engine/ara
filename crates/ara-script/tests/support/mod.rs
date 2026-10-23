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
