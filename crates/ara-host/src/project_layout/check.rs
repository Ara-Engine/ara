//! Reject legacy layouts before schemas, C# discovery, imports or output creation.
use ara_diag::{Diagnostic, FixAction, codes};
use std::path::Path;

/// Requires explicit migration before a project enters any ordinary open path.
/// This check reads only the marker; it never creates a lock, cache or directory.
pub fn require_current_layout(root: &Path) -> Result<(), Diagnostic> {
    let file = root.join(crate::project::MARKER);
    let text = std::fs::read_to_string(&file).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            crate::project::no_project(root)
        } else {
            Diagnostic::error(codes::IO, error.to_string()).file(file.display().to_string())
        }
    })?;
    let marker = ara_json::parse(&text).map_err(|error| {
        Diagnostic::error(codes::JSON_SYNTAX, error.message)
            .file(file.display().to_string()).line_col(error.line, error.column)
    })?;
    if !marker.is_object() {
        return Err(Diagnostic::error(codes::INVALID_PARAMS, "project marker must be an object")
            .file(file.display().to_string()));
    }
    let layout = ara_doc::detect_project_layout(&marker);
    if layout.is_current() {
        return Ok(());
    }
    let detected = match layout {
        ara_doc::ProjectLayout::Preview3 => "preview-3 game/ layout",
        ara_doc::ProjectLayout::Root => "legacy project-root layout",
        ara_doc::ProjectLayout::Current => unreachable!("current layout returned above"),
    };
    Err(Diagnostic::error(codes::PROJECT_MIGRATION_REQUIRED,
        format!("this project uses the {detected}; migrate with `ara project migrate` or open in the Hub/editor to migrate to {}/ before opening", ara_doc::AUTHORING_ROOT))
        .file(file.display().to_string())
        .with_fix("Preview the migration", FixAction::Cli { args: vec!["project".into(), "migrate".into(), "--root".into(), root.display().to_string(), "--dry-run".into()] }))
}
