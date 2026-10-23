//! Gameplay modules: one folder, one `ara.module.toml`, one assembly, one reload unit.
//!
//! ```toml
//! [module]
//! name = "Outpost.Sample"        # assembly and module name
//! hot = false                    # start as Debug IL with EnC (default: cold, optimized)
//! references = ["Outpost.Core"]  # modules this one uses (default: none)
//! profile = "gameplay"           # API allowlist: "gameplay" (default) or "editor"
//! ```
//!
//! Modules stay small (soft cap about 5k lines): L3 reload cost scales with assembly
//! size, not edit size (R10). The compile server reports modules past the cap. A module
//! that references another is rebuilt (or relinked) whenever the other gets a new
//! generation, and both swap at the same tick boundary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ara_diag::{Diagnostic, codes};
use serde::Deserialize;

use crate::codes as host_codes;

/// The module file's name.
pub const MODULE_FILE: &str = "ara.module.toml";

/// Which API allowlist a module compiles against (`ARA05xx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleProfile {
    /// Gameplay code: no files, network, reflection, threads, processes or native interop.
    #[default]
    Gameplay,
    /// Editor extensions: the gameplay allowlist plus files, processes, tasks and reflection.
    Editor,
}

impl ModuleProfile {
    /// The name the compile server and analyzers use (`gameplay`, `editor`).
    pub fn as_str(self) -> &'static str {
        match self {
            ModuleProfile::Gameplay => "gameplay",
            ModuleProfile::Editor => "editor",
        }
    }
}

/// One gameplay module to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSpec {
    /// Module and assembly name (`Outpost.Sample`).
    pub name: String,
    /// The module folder (every `*.cs` under it is compiled).
    pub dir: PathBuf,
    /// A prebuilt assembly to load instead of compiling (no reload without the compile server).
    pub assembly: Option<PathBuf>,
    /// Watch the folder and hot reload through the compile server.
    pub watch: bool,
    /// Start hot (Debug IL, EnC) instead of cold (optimized).
    pub hot: bool,
    /// Names of the modules this one references; they load first.
    pub references: Vec<String>,
    /// The API allowlist the module compiles against.
    pub profile: ModuleProfile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleFile {
    module: ModuleSection,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleSection {
    name: String,
    #[serde(default)]
    hot: bool,
    #[serde(default)]
    references: Vec<String>,
    #[serde(default)]
    profile: ModuleProfile,
}

impl ModuleSpec {
    /// Reads `dir/ara.module.toml`. The module is watched by default.
    pub fn from_dir(dir: &Path) -> Result<Self, Diagnostic> {
        let path = dir.join(MODULE_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            Diagnostic::error(codes::IO, format!("read `{}`: {e}", path.display()))
                .file(path.display().to_string())
        })?;
        let file: ModuleFile = toml::from_str(&text).map_err(|e| {
            Diagnostic::error(codes::INVALID_PARAMS, format!("`{}`: {e}", path.display()))
                .file(path.display().to_string())
        })?;
        Ok(ModuleSpec {
            name: file.module.name,
            dir: dir.to_path_buf(),
            assembly: None,
            watch: true,
            hot: file.module.hot,
            references: file.module.references,
            profile: file.module.profile,
        })
    }
}

/// Orders modules so every module comes after the modules it references (ties keep the
/// given order). Modules in a reference cycle, referencing an unknown module, or
/// depending on one of those are left out, each with a diagnostic.
pub fn load_order(specs: &[ModuleSpec]) -> (Vec<usize>, Vec<Diagnostic>) {
    let index: BTreeMap<&str, usize> = specs
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.as_str(), i))
        .collect();
    let mut problems = Vec::new();
    // 0 = unvisited, 1 = on the current path, 2 = ordered, 3 = refused.
    let mut state = vec![0u8; specs.len()];
    let mut order = Vec::with_capacity(specs.len());
    for root in 0..specs.len() {
        let mut path = Vec::new();
        visit(
            specs,
            &index,
            root,
            &mut state,
            &mut path,
            &mut order,
            &mut problems,
        );
    }
    (order, problems)
}

/// Depth-first visit; returns false if `i` (or something it needs) cannot load.
fn visit(
    specs: &[ModuleSpec],
    index: &BTreeMap<&str, usize>,
    i: usize,
    state: &mut [u8],
    path: &mut Vec<usize>,
    order: &mut Vec<usize>,
    problems: &mut Vec<Diagnostic>,
) -> bool {
    match state[i] {
        2 => return true,
        3 => return false,
        1 => {
            let start = path.iter().position(|&p| p == i).unwrap_or(0);
            let mut cycle: Vec<&str> = path[start..]
                .iter()
                .map(|&p| specs[p].name.as_str())
                .collect();
            cycle.push(&specs[i].name);
            problems.push(Diagnostic::error(
                host_codes::MODULE_CYCLE,
                format!(
                    "modules reference each other in a cycle: {}; remove one reference (move the shared types into a module both reference)",
                    cycle.join(" -> ")
                ),
            ));
            for &p in &path[start..] {
                state[p] = 3;
            }
            return false;
        }
        _ => {}
    }
    state[i] = 1;
    path.push(i);
    let mut ok = true;
    let mut failed = None;
    for r in &specs[i].references {
        match index.get(r.as_str()) {
            Some(&j) => {
                if !visit(specs, index, j, state, path, order, problems) {
                    ok = false;
                    failed.get_or_insert(r.as_str());
                }
            }
            None => {
                problems.push(Diagnostic::error(
                    host_codes::UNKNOWN_MODULE,
                    format!(
                        "module `{}` references `{r}`, which is not loaded; add it to the module list or remove the reference",
                        specs[i].name
                    ),
                ));
                ok = false;
            }
        }
    }
    path.pop();
    if state[i] == 3 {
        return false;
    }
    if let Some(dep) = failed {
        problems.push(Diagnostic::error(
            host_codes::DEPENDENCY_FAILED,
            format!(
                "module `{}` is not loaded because `{dep}` cannot load",
                specs[i].name
            ),
        ));
    }
    if ok {
        state[i] = 2;
        order.push(i);
    } else {
        state[i] = 3;
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, refs: &[&str]) -> ModuleSpec {
        ModuleSpec {
            name: name.into(),
            dir: PathBuf::new(),
            assembly: None,
            watch: false,
            hot: false,
            references: refs.iter().map(|r| (*r).to_owned()).collect(),
            profile: ModuleProfile::Gameplay,
        }
    }

    fn names(specs: &[ModuleSpec], order: &[usize]) -> Vec<String> {
        order.iter().map(|&i| specs[i].name.clone()).collect()
    }

    #[test]
    fn reads_the_sample_module() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dotnet/samples/Outpost.Sample");
        let spec = ModuleSpec::from_dir(&dir).expect("sample module file");
        assert_eq!(
            (spec.name.as_str(), spec.hot, spec.watch, spec.profile),
            ("Outpost.Sample", false, true, ModuleProfile::Gameplay)
        );
    }

    #[test]
    fn dependencies_load_first() {
        let specs = [spec("B", &["A"]), spec("C", &["B", "A"]), spec("A", &[])];
        let (order, problems) = load_order(&specs);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(names(&specs, &order), ["A", "B", "C"]);
    }

    #[test]
    fn cycles_and_unknown_references_are_refused() {
        let specs = [
            spec("A", &["B"]),
            spec("B", &["A"]),
            spec("C", &["A"]),
            spec("D", &["Missing"]),
            spec("E", &[]),
        ];
        let (order, problems) = load_order(&specs);
        assert_eq!(names(&specs, &order), ["E"]);
        let codes: Vec<_> = problems.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            [
                host_codes::MODULE_CYCLE,
                host_codes::DEPENDENCY_FAILED,
                host_codes::UNKNOWN_MODULE
            ]
        );
        assert!(problems[0].message.contains("A -> B -> A"), "{problems:?}");
    }
}
