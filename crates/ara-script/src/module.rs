//! Gameplay modules: one folder, one `ara.module.toml`, one assembly, one reload unit.
//!
//! ```toml
//! [module]
//! name = "Outpost.Sample"   # assembly and module name
//! hot = false               # start as Debug IL with EnC (default: cold, optimized)
//! ```
//!
//! Modules stay small (soft cap about 5k lines): L3 reload cost scales with assembly
//! size, not edit size (R10). The compile server reports modules past the cap.

use std::path::{Path, PathBuf};

use ara_diag::{Diagnostic, codes};
use serde::Deserialize;

/// The module file's name.
pub const MODULE_FILE: &str = "ara.module.toml";

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
}

impl ModuleSpec {
    /// Reads `dir/ara.module.toml`. The module is watched by default.
    pub fn from_dir(dir: &Path) -> Result<Self, Diagnostic> {
        let path = dir.join(MODULE_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Diagnostic::error(codes::IO, format!("read `{}`: {e}", path.display())).file(path.display().to_string()))?;
        let file: ModuleFile = toml::from_str(&text).map_err(|e| {
            Diagnostic::error(codes::INVALID_PARAMS, format!("`{}`: {e}", path.display())).file(path.display().to_string())
        })?;
        Ok(ModuleSpec {
            name: file.module.name,
            dir: dir.to_path_buf(),
            assembly: None,
            watch: true,
            hot: file.module.hot,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_sample_module() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dotnet/samples/Outpost.Sample");
        let spec = ModuleSpec::from_dir(&dir).expect("sample module file");
        assert_eq!((spec.name.as_str(), spec.hot, spec.watch), ("Outpost.Sample", false, true));
    }
}
