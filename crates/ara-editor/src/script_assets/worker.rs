//! Out-of-process compilation: .NET and Roslyn never block the native editor.
use serde_json::json;
use std::path::Path;

/// Compile the project's real Ara modules and emit their reflected types as JSON.
/// Invoked only in a hidden child process by the script authoring service.
pub fn compile_scripts(root: &Path) -> ara_ui::Result<()> {
    let result = compile(root);
    println!(
        "{}",
        ara_json::to_canonical_string(&match result {
            Ok(types) => json!({"ok":true,"types":types}),
            Err(diagnostics) => json!({"ok":false,"diagnostics":diagnostics}),
        })
    );
    Ok(())
}

fn compile(root: &Path) -> Result<serde_json::Value, Vec<ara_diag::Diagnostic>> {
    let paths = ara_play_host::ScriptPaths::locate(root).ok_or_else(||vec![crate::invalid("C# tools not found. Install the .NET 10 SDK and run ara doctor; set ARA_DOTNET_ARTIFACTS to the engine's dotnet/artifacts/bin.")])?;
    let mut config = ara_script::ScriptConfig::jit(paths.host_dir);
    config.compile_server = paths.compile_server;
    let schema = ara_runtime::schema(&[]).map_err(|e| vec![crate::invalid(e.to_string())])?;
    let mut world = bevy_ecs::world::World::new();
    let mut host =
        ara_script::ScriptHost::start(config, schema, &ara_script::engine_bindings(), &mut world)?;
    let modules = crate::shell::scripts::modules(root)
        .iter()
        .map(|p| ara_script::ModuleSpec::from_dir(p))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|d| vec![d])?;
    host.load_modules(&mut world, &modules)?;
    let types = serde_json::to_value(host.module_types())
        .map_err(|e| vec![crate::invalid(e.to_string())])?;
    host.shutdown();
    Ok(types)
}
