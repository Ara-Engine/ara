//! Explicit offline PSO permutations; source and baseline manifests stay independent.
use crate::CookManifest;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

/// Named pipeline overrides in a separate experimental recipe.
#[derive(Default, Deserialize, Serialize)]
pub struct Variant {
    /// Only these pipelines change; other baseline pipelines are reused.
    pub pipelines: BTreeMap<String, Override>,
}

/// An offline compiler permutation or sibling shader with the same pipeline ABI.
#[derive(Default, Deserialize, Serialize)]
pub struct Override {
    /// Optional Slang sibling, relative to the baseline manifest's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Preprocessor definitions, included in artifact identity for both targets.
    #[serde(default)]
    pub defines: BTreeMap<String, String>,
}

impl Variant {
    pub(crate) fn apply(&self, manifest: &mut CookManifest, directory: &Path) -> Result<()> {
        ensure!(
            !self.pipelines.is_empty(),
            "variant has no pipeline overrides"
        );
        let directory = directory.canonicalize()?;
        for (name, change) in &self.pipelines {
            let pipeline = manifest
                .get_mut(name)
                .with_context(|| format!("unknown variant pipeline {name}"))?;
            if let Some(source) = &change.source {
                let path = directory.join(source).canonicalize()?;
                ensure!(
                    path.starts_with(&directory) && path.extension().is_some_and(|e| e == "slang"),
                    "variant sources must be Slang files inside the baseline source directory"
                );
                pipeline.source = source.clone();
            }
            validate_defines(&change.defines)?;
            for (name, value) in &change.defines {
                pipeline.defines.insert(name.clone(), value.clone());
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_defines(defines: &BTreeMap<String, String>) -> Result<()> {
    for (name, value) in defines {
        ensure!(
            identifier(name) && name != "VULKAN",
            "invalid or reserved shader definition {name}"
        );
        ensure!(
            value.len() <= 1024 && !value.chars().any(|c| c == '\0' || c == '\n' || c == '\r'),
            "invalid value for shader definition {name}"
        );
    }
    Ok(())
}

fn identifier(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// Write the selected overrides into a baseline after a successful experimental cook.
/// The caller explicitly opts into this authoring change; the canonical writer is shared.
pub fn promote(manifest: &Path, variant: &Variant) -> Result<()> {
    let mut source: CookManifest = serde_json::from_slice(&std::fs::read(manifest)?)?;
    variant.apply(&mut source, manifest.parent().context("manifest parent")?)?;
    let temporary = manifest.with_extension("promote.tmp");
    ara_gpu::cooked::write_json(&temporary, &source)?;
    std::fs::rename(temporary, manifest)?;
    Ok(())
}
