use ara_script::{EngineBindings, ModuleSpec, ScriptConfig, ScriptHost, bind_component};
use bevy_ecs::prelude::Component;
use bevy_ecs::world::World;

use super::support;

/// A canonical Transform layout in native sparse storage, using the existing ABI.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
#[component(storage = "SparseSet")]
#[repr(C)]
pub struct SparseTransform {
    /// Local position.
    pub position: [f32; 3],
    /// Euler rotation in degrees.
    pub rotation: [f32; 3],
    /// Local scale.
    pub scale: [f32; 3],
}

/// Unrelated sparse membership changes archetypes without changing their table.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub struct SparseMarker;

/// An unrelated dense tag fragments native columns into separate tables.
#[derive(Component)]
pub struct DenseMarker;

/// Starts the actual CLR host with a checked, canonical sparse Transform prefix.
pub fn start(tag: &str, source: &str, hot: bool) -> support::Live {
    let root = support::Live::write(tag, &[("Game", source, &[])]);
    if !hot {
        let manifest = root.join("Game/ara.module.toml");
        let text = std::fs::read_to_string(&manifest).expect("written module");
        std::fs::write(manifest, text.replace("hot = true", "hot = false")).expect("cold module");
    }
    let mut world = World::new();
    let schema = ara_runtime::schema(&[]).expect("engine schema");
    let bindings = bind_component!(EngineBindings::new(), SparseTransform as "Transform" { position, rotation, scale });
    assert_eq!(bindings.names(), EngineBindings::builtin().names());
    let mut config = ScriptConfig::jit(support::bin("Ara.Host"));
    config.compile_server = Some(support::bin("Ara.CompileServer").join("Ara.CompileServer.dll"));
    let host = ScriptHost::start(config, schema, &bindings, &mut world)
        .expect("checked native sparse prefix must start with the canonical engine ABI");
    let mut live = support::Live {
        host,
        world,
        root,
        frame: 0,
        reports: Vec::new(),
        diagnostics: Vec::new(),
    };
    let module = ModuleSpec::from_dir(&live.root.join("Game")).expect("module spec");
    live.host
        .load_modules(&mut live.world, &[module])
        .expect("sparse gameplay module");
    live
}
