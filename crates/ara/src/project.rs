//! Finding the project root for a command: `--root`, else the nearest ancestor of
//! the current directory with `ara.project.json`, else (for commands given files)
//! the nearest one above the first file. Never the bare current directory: without
//! a marker the command fails with ARA1901 and an `ara init` fix.

use std::path::{Path, PathBuf};

use ara_diag::Diagnostic;
pub use ara_host::Project;
use ara_host::project::{MARKER, discover, no_project};

/// The project root for this invocation.
pub fn root(flag: Option<&Path>, files: &[PathBuf]) -> Result<PathBuf, Diagnostic> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(dir) = flag {
        return if dir.join(MARKER).is_file() {
            Ok(dir.to_path_buf())
        } else {
            Err(no_project(dir))
        };
    }
    if let Some(r) = discover(&cwd) {
        return Ok(r);
    }
    files
        .iter()
        .filter_map(|f| std::path::absolute(f).ok())
        .find_map(|f| f.parent().and_then(discover))
        .ok_or_else(|| no_project(&cwd))
}

/// Opens the project at the root this invocation resolves to.
pub fn open(flag: Option<&Path>, files: &[PathBuf]) -> Result<Project, Diagnostic> {
    load(&root(flag, files)?)
}

/// Opens a project with every engine component (physics, navigation, rendering,
/// gameplay) registered, so documents using them validate everywhere.
pub fn load(root: &Path) -> Result<Project, Diagnostic> {
    let mut project = Project::open(root)?;
    ara_play_host::register_engine_schemas(&mut project.db);
    Ok(project)
}
