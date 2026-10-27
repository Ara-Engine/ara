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
    let inventory = ara_script::project::publish(root, ara_script::ModuleProfile::Gameplay)
        .map_err(|d| vec![d])?;
    let errors: Vec<_> = inventory
        .diagnostics
        .into_iter()
        .filter(ara_diag::Diagnostic::is_error)
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(serde_json::Value::Object(
        inventory
            .modules
            .into_iter()
            .filter(|m| m.profile == ara_script::ModuleProfile::Gameplay)
            .filter_map(|m| m.schema.map(|schema| (m.name, schema)))
            .collect(),
    ))
}
