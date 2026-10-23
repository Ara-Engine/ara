//! Materializes a task's starting project: `ara init` (or a minimal stand-in for
//! tests that have no binary), the fixture files, then the task's setup hook.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::task::{Fixture, Task};

/// How the project itself is created.
#[derive(Debug, Clone, Copy)]
pub enum Init<'a> {
    /// The real `ara init` (what users and agents get), from this binary.
    Binary(&'a Path),
    /// `ara.project.json` and an empty `levels/main.scene` only (no AGENTS.md or
    /// `.mcp.json`): enough for verifiers and reference solutions.
    Minimal,
}

/// The `fixtures/` directory of this crate.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// A fresh, unique directory under the system temp dir (`<temp>/arabench/<label>-…`).
pub fn scratch(label: &str) -> Result<PathBuf, String> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join("arabench")
        .join(format!("{label}-{}-{nanos:x}-{n}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create `{}`: {e}", dir.display()))?;
    Ok(dir)
}

/// Copies `from` into `to` recursively, overwriting files.
pub fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    let entries =
        fs::read_dir(from).map_err(|e| format!("cannot read `{}`: {e}", from.display()))?;
    fs::create_dir_all(to).map_err(|e| format!("cannot create `{}`: {e}", to.display()))?;
    for entry in entries.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| format!("cannot copy `{}`: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// Replaces the one occurrence of `from` in project file `rel` with `to`.
pub fn patch(root: &Path, rel: &str, from: &str, to: &str) -> Result<(), String> {
    let path = root.join(rel);
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read `{rel}`: {e}"))?;
    if text.matches(from).count() != 1 {
        return Err(format!("`{rel}` must contain `{from}` exactly once"));
    }
    fs::write(&path, text.replacen(from, to, 1)).map_err(|e| format!("cannot write `{rel}`: {e}"))
}

/// Creates `task`'s starting project at `root` (which must not exist yet).
pub fn create(root: &Path, task: &Task, init: Init<'_>) -> Result<(), String> {
    let name = match task.fixture {
        Fixture::Empty => "game",
        Fixture::Outpost => "outpost",
    };
    match init {
        Init::Binary(ara) => {
            let out = Command::new(ara)
                .arg("init")
                .arg(root)
                .args(["--name", name])
                .output()
                .map_err(|e| format!("cannot run `{}`: {e}", ara.display()))?;
            if !out.status.success() {
                return Err(format!(
                    "`ara init` failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
        }
        Init::Minimal => {
            let write = |rel: &str, v: serde_json::Value| -> Result<(), String> {
                let path = root.join(rel);
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                fs::write(&path, ara_json::to_canonical_string(&v)).map_err(|e| e.to_string())
            };
            write("ara.project.json", json!({ "name": name }))?;
            write(
                "levels/main.scene",
                json!({ "$schema": "ara://schema/scene/1", "id": "sc_main0001", "records": {} }),
            )?;
        }
    }
    if task.fixture == Fixture::Outpost {
        copy_dir(&fixtures_dir().join("outpost"), root)?;
    }
    if let Some(setup) = task.setup {
        setup(root)?;
    }
    Ok(())
}
