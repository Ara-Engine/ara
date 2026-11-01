//! Refuse a companion executable built before its sources changed. `ara` spawns the
//! sibling `ara-player` (renders and captures) and `ara-import-worker` (imports); after
//! `cargo build -p ara` alone those are older builds that load this tree's cooked
//! shaders and importers against old code, with no sign of it in the output.
//!
//! The `ara` binary calls [`enforce`] once; spawn sites call [`verify`] with the path
//! they are about to run.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// The executables checked, by package (and binary) name.
const COMPANIONS: [&str; 2] = ["ara-player", "ara-import-worker"];

/// Set `ARA_ALLOW_STALE_COMPANIONS=1` to run an old companion on purpose.
const ALLOW: &str = "ARA_ALLOW_STALE_COMPANIONS";

static ENFORCED: OnceLock<()> = OnceLock::new();

/// Turns checking on for this process. Until called, [`verify`] always passes, so only
/// the `ara` binary refuses companions.
pub fn enforce() {
    let _ = ENFORCED.set(());
}

/// Why a companion must not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The companion executable.
    pub exe: PathBuf,
    /// Its package name (`ara-player`).
    pub package: &'static str,
    /// What is wrong with it.
    pub reason: Reason,
}

/// What is wrong with a refused companion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// The executable does not exist.
    Missing,
    /// A source in its closure changed after the executable was built.
    Stale {
        /// The newer source file.
        newer: PathBuf,
    },
}

impl Refusal {
    /// The `cargo build` that produces this companion in the profile it was found in.
    pub fn fix(&self) -> String {
        let profile = self
            .exe
            .parent()
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            .map_or(String::new(), |name| match name {
                "debug" | "deps" => String::new(),
                "release" => " --release".into(),
                other => format!(" --profile {other}"),
            });
        format!("cargo build -p {}{profile}", self.package)
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let exe = self.exe.display();
        match &self.reason {
            Reason::Missing => write!(
                f,
                "ARA7308: `{exe}` is not built, but `ara` runs it beside itself. Build it: {}",
                self.fix()
            ),
            Reason::Stale { newer } => write!(
                f,
                "ARA7308: `{exe}` is older than its sources (`{}` changed after it was built), \
                 so it would run this tree's cooked shaders and importers against old code. \
                 Rebuild it: {} (or all companions together: \
                 cargo build -p ara -p ara-player -p ara-import-worker)",
                newer.display(),
                self.fix()
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// Checks `exe` before it is spawned. Passes for anything that is not a companion
/// beside the running executable, and whenever the source tree is not on this machine
/// (packaged builds were all built together).
pub fn verify(exe: &Path) -> Result<(), Refusal> {
    if ENFORCED.get().is_none() || std::env::var_os(ALLOW).is_some_and(|v| !v.is_empty()) {
        return Ok(());
    }
    let Some(package) = package_of(exe) else {
        return Ok(());
    };
    if !beside_running_executable(exe) {
        return Ok(());
    }
    let Some(root) = source_root() else {
        return Ok(());
    };
    let built = std::fs::metadata(exe).and_then(|m| m.modified()).ok();
    type Cache = Mutex<HashMap<(PathBuf, Option<SystemTime>), Option<Reason>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    let key = (exe.to_path_buf(), built);
    let cached = cache.lock().ok().and_then(|c| c.get(&key).cloned());
    let reason = cached.unwrap_or_else(|| {
        let reason = assess(&root, package, built);
        if let Ok(mut cache) = cache.lock() {
            cache.insert(key, reason.clone());
        }
        reason
    });
    reason.map_or(Ok(()), |reason| {
        Err(Refusal {
            exe: exe.to_path_buf(),
            package,
            reason,
        })
    })
}

fn package_of(exe: &Path) -> Option<&'static str> {
    let stem = exe.file_stem()?.to_str()?;
    COMPANIONS.into_iter().find(|name| *name == stem)
}

/// Companions are looked up beside the running executable; test binaries live one
/// level down in `deps`.
fn beside_running_executable(exe: &Path) -> bool {
    let Ok(running) = std::env::current_exe() else {
        return false;
    };
    let mut dir = running.parent().map(Path::to_path_buf).unwrap_or_default();
    if dir.ends_with("deps") {
        dir.pop();
    }
    exe.parent().is_some_and(|parent| same_dir(parent, &dir))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The workspace this crate was compiled in, if it still exists here.
fn source_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    root.join("crates").is_dir().then(|| root.to_path_buf())
}

/// `None` when the companion built at `built` is fresh.
fn assess(root: &Path, package: &str, built: Option<SystemTime>) -> Option<Reason> {
    let Some(built) = built else {
        return Some(Reason::Missing);
    };
    let mut seen = BTreeSet::new();
    let mut pending = vec![root.join("crates").join(package)];
    while let Some(dir) = pending.pop() {
        if !seen.insert(dir.clone()) {
            continue;
        }
        if let Some(newer) = newer_source(&dir, built) {
            return Some(Reason::Stale { newer });
        }
        pending.extend(path_dependencies(root, &dir));
    }
    None
}

/// A file Cargo compiles for the crate at `dir` that changed after `built`.
fn newer_source(dir: &Path, built: SystemTime) -> Option<PathBuf> {
    let newer = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified > built)
    };
    ["Cargo.toml", "build.rs"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|path| newer(path))
        .or_else(|| newer_in(&dir.join("src"), &newer))
}

fn newer_in(dir: &Path, newer: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            if let Some(found) = newer_in(&path, newer) {
                return Some(found);
            }
        } else if newer(&path) {
            return Some(path);
        }
    }
    None
}

/// The path dependencies of the crate at `dir` that its normal build compiles:
/// `[dependencies]` and `[build-dependencies]` (also under a `[target.*]` that names
/// this OS), skipping `optional` ones, which only a feature pulls in. Entries written
/// `name.workspace = true` resolve through the root manifest's
/// `[workspace.dependencies]`.
///
/// A line scanner, not a TOML parser: this workspace writes one dependency per line
/// (`name = { path = "../x" }`), and a TOML crate in `ara-asset`'s graph changes type
/// inference in `ara-importers`.
fn path_dependencies(root: &Path, dir: &Path) -> Vec<PathBuf> {
    let read = |path: PathBuf| std::fs::read_to_string(path).unwrap_or_default();
    let shared = dependency_lines(&read(root.join("Cargo.toml")), "workspace.dependencies");
    let mut found = Vec::new();
    for (name, spec) in dependency_lines(&read(dir.join("Cargo.toml")), "") {
        if spec.replace(' ', "").contains("optional=true") {
            continue;
        }
        if let Some(path) = quoted_after(&spec, "path") {
            found.push(tidy(&dir.join(path)));
        } else if spec.replace(' ', "").contains("workspace=true")
            && let Some((_, shared)) = shared.iter().find(|(shared, _)| *shared == name)
            && let Some(path) = quoted_after(shared, "path")
        {
            found.push(tidy(&root.join(path)));
        }
    }
    found
}

/// `path` without `..` components, so messages show `crates/ara-render`, not `crates/ara-player/../ara-render`.
fn tidy(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `(name, rest of the line)` of every dependency line in the sections `header`
/// selects: an exact section such as `workspace.dependencies`, or (empty) a crate's
/// compiled dependency sections for this OS.
fn dependency_lines(manifest: &str, header: &str) -> Vec<(String, String)> {
    let mut selected = false;
    let mut lines = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if let Some(section) = line.strip_prefix('[') {
            let section = section.trim_end_matches(']');
            selected = if header.is_empty() {
                compiled_section(section)
            } else {
                section == header
            };
        } else if selected && !line.starts_with('#') {
            let Some((key, spec)) = line.split_once('=') else {
                continue;
            };
            // `name.workspace = true` and `name = { ... }` both name `name`.
            let key = key.trim();
            let name = key.split('.').next().unwrap_or(key).trim_matches('"');
            let spec = if key.contains('.') {
                format!("{} = {}", key.split_once('.').map_or("", |(_, k)| k), spec)
            } else {
                spec.to_owned()
            };
            lines.push((name.to_owned(), spec));
        }
    }
    lines
}

/// Whether a normal build compiles the dependencies of manifest section `section`.
fn compiled_section(section: &str) -> bool {
    let os = std::env::consts::OS;
    match section {
        "dependencies" | "build-dependencies" => true,
        _ => {
            section.starts_with("target.")
                && (section.ends_with(".dependencies") || section.ends_with(".build-dependencies"))
                && (section.contains(os) || (os != "windows" && section.contains("unix")))
        }
    }
}

/// The first `"..."` value after `key =` in `spec`.
fn quoted_after<'a>(spec: &'a str, key: &str) -> Option<&'a str> {
    let rest = &spec[spec.find(&format!("{key} ="))? + key.len() + 2..];
    let rest = rest.trim_start().strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

#[cfg(test)]
mod tests;
