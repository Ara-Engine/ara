//! Out-of-process compilation: .NET and Roslyn never block the native editor.
use serde_json::json;
use std::path::Path;

/// Compile the project's real Ara modules and emit their reflected types as JSON.
/// Invoked only in a hidden child process by the script authoring service.
pub fn compile_scripts(root: &Path) -> ara_ui::Result<()> {
    println!(
        "{}",
        ara_json::to_canonical_string(&response(compile(root)))
    );
    Ok(())
}

/// Actual reflected types and the complete ordered compiler diagnostic stream.
pub(super) struct Compiled {
    pub(super) types: serde_json::Value,
    pub(super) diagnostics: Vec<ara_diag::Diagnostic>,
}

/// The helper protocol is identical for child stdout and the preparation thread.
pub(super) fn response(result: Result<Compiled, Vec<ara_diag::Diagnostic>>) -> serde_json::Value {
    match result {
        Ok(compiled)=>json!({"ok":true,"types":compiled.types,"diagnostics":compiled.diagnostics}),
        Err(diagnostics)=>json!({"ok":false,"diagnostics":diagnostics}),
    }
}

pub(super) fn compile(root: &Path) -> Result<Compiled, Vec<ara_diag::Diagnostic>> {
    let inventory = ara_script::project::publish(root, ara_script::ModuleProfile::Gameplay)
        .map_err(|d| vec![d])?;
    compiled(inventory)
}

pub(super) fn compiled(inventory: ara_script::project::Inventory) -> Result<Compiled, Vec<ara_diag::Diagnostic>> {
    if inventory.diagnostics.iter().any(ara_diag::Diagnostic::is_error) {
        return Err(inventory.diagnostics);
    }
    Ok(Compiled{types:serde_json::Value::Object(
        inventory
            .modules
            .into_iter()
            .filter(|m| m.profile == ara_script::ModuleProfile::Gameplay)
            .filter_map(|m| m.schema.map(|schema| (m.name, schema)))
            .collect(),
    ),diagnostics:inventory.diagnostics})
}
