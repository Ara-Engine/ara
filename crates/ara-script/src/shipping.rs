//! Exported NativeAOT images use the gameplay ABI, with no development loader.
use crate::{abi, codes};
use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Portable metadata adjacent to a project's linked NativeAOT image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptManifest {
    /// Manifest contract (`ara.script.nativeaot/1`).
    pub format: String,
    /// Generated host ABI version.
    pub abi_version: u32,
    /// Generated engine/ABI layout hash, as sixteen hexadecimal digits.
    pub layout_hash: String,
    /// Image path relative to this manifest; never an author-machine path.
    pub library: String,
    /// Linked gameplay modules, in dependency order.
    pub modules: Vec<String>,
}

impl ScriptManifest {
    /// Describes a linked image built for the supplied engine layouts.
    pub fn new(library: String, modules: Vec<String>, layout_hash: u64) -> Self {
        Self {
            format: "ara.script.nativeaot/1".into(),
            abi_version: abi::ABI_VERSION,
            layout_hash: format!("{layout_hash:016x}"),
            library,
            modules,
        }
    }

    /// Reads metadata without starting a runtime or looking for .NET tools.
    pub fn read(path: &Path) -> Result<Self, Diagnostic> {
        let fail = |e| {
            Diagnostic::error(
                codes::LOAD_FAILED,
                format!("shipping scripts `{}`: {e}", path.display()),
            )
        };
        let bytes = std::fs::read(path).map_err(|e| fail(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| fail(e.to_string()))
    }

    /// Checks the host identity before any native image is loaded.
    pub fn validate(&self, expected_layout: u64) -> Result<(), Diagnostic> {
        if self.format != "ara.script.nativeaot/1"
            || self.abi_version != abi::ABI_VERSION
            || self.layout_hash != format!("{expected_layout:016x}")
        {
            return Err(Diagnostic::error(
                codes::STALE_BINDINGS,
                format!(
                    "shipping script ABI/layout mismatch: image metadata {}/{}/{}, player ara.script.nativeaot/1/{}/{expected_layout:016x}; rebuild the gameplay scripts and player with the same Ara SDK and export again",
                    self.format,
                    self.abi_version,
                    self.layout_hash,
                    abi::ABI_VERSION,
                ),
            ));
        }
        Ok(())
    }

    /// Resolves only a contained export-relative library, including after relocation.
    pub fn library_path(&self, manifest: &Path) -> Result<PathBuf, Diagnostic> {
        let fail = |message| Diagnostic::error(codes::LOAD_FAILED, message);
        let relative = Path::new(&self.library);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || self.library.contains([':', '\\'])
        {
            return Err(fail(
                "shipping script library must be a relative export path",
            ));
        }
        let base = manifest
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let root = std::fs::canonicalize(base)
            .map_err(|_| fail("shipping script directory is missing"))?;
        let image = std::fs::canonicalize(base.join(relative)).map_err(|_| {
            fail("shipping script image is missing; restore the export's game directory")
        })?;
        if !image.starts_with(root) {
            return Err(fail("shipping script image resolves outside the export"));
        }
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_rejects_stale_abi_and_layout_before_loading() {
        let valid = ScriptManifest::new("Game.dll".into(), vec!["Game".into()], 42);
        valid.validate(42).expect("matching ABI and layout");
        let mut wrong = valid.clone();
        wrong.abi_version += 1;
        assert_eq!(
            wrong.validate(42).expect_err("stale ABI").code,
            codes::STALE_BINDINGS
        );
        assert_eq!(
            valid.validate(43).expect_err("stale layout").code,
            codes::STALE_BINDINGS
        );
        for path in [
            "../Game.dll",
            "C:/author/Game.dll",
            "/Game.dll",
            "..\\Game.dll",
        ] {
            let mut wrong = valid.clone();
            wrong.library = path.into();
            assert_eq!(
                wrong
                    .library_path(Path::new("game/scripts.json"))
                    .expect_err("non-portable image path")
                    .code,
                codes::LOAD_FAILED
            );
        }
    }
}
