//! Optional Metal lane: cooked DXIL -> Apple Metal Shader Converter -> metallib plus
//! converter reflection, with Ara's fixed global root signature
//! (`metal/root-signature.json`, mirroring the Vulkan set-0 layout). It runs on the
//! canonical cook host (Windows) when the converter is installed; without it the cook
//! still succeeds and reports `ARA_GPU_METAL_COOK_SKIPPED` once per process.
//!
//! Flags, root-signature format and reflection were checked against converter 4.0.3
//! (`metal-shaderconverter --help`, its headers and manual); re-check them whenever
//! the converter is upgraded.
use anyhow::{Context, Result, ensure};
use ara_gpu::cooked::{CookedMetalStage, CookedStage};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// Ara's global root signature for every converted stage.
const ROOT_SIGNATURE: &str = include_str!("../metal/root-signature.json");
/// Converted stages target the Metal 4 OS baseline (D15).
const MINIMUM_OS: &str = "26.0";

/// An installed converter and its exact version string.
pub struct Converter {
    executable: PathBuf,
    version: String,
    root_signature: PathBuf,
}

/// Hash recorded in `CookedMetalStage::root_signature_hash`.
pub fn root_signature_hash() -> String {
    format!("{:x}", Sha256::digest(ROOT_SIGNATURE.as_bytes()))
}

impl Converter {
    /// `ARA_METAL_SHADERCONVERTER` (an executable path, or `off`), then `PATH`, then
    /// Apple's default install location. None when the converter is not installed.
    pub fn discover() -> Result<Option<Self>> {
        let name = format!("metal-shaderconverter{}", std::env::consts::EXE_SUFFIX);
        let executable = match std::env::var_os("ARA_METAL_SHADERCONVERTER") {
            Some(value) if value.is_empty() || value == "off" => return Ok(None),
            Some(value) => Some(PathBuf::from(value)),
            None => std::env::var_os("PATH")
                .and_then(|path| {
                    std::env::split_paths(&path)
                        .map(|dir| dir.join(&name))
                        .find(|candidate| candidate.is_file())
                })
                .or_else(|| {
                    [
                        "/opt/metal-shaderconverter/bin",
                        r"C:\Program Files\Metal Shader Converter\bin",
                    ]
                    .into_iter()
                    .map(|dir| Path::new(dir).join(&name))
                    .find(|candidate| candidate.is_file())
                }),
        };
        let Some(executable) = executable else {
            return Ok(None);
        };
        let output = Command::new(&executable)
            .arg("--version")
            .output()
            .with_context(|| format!("Starting {}", executable.display()))?;
        ensure!(
            output.status.success(),
            "{} --version failed",
            executable.display()
        );
        let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        // The converter reads the root signature from a file; it is written outside the
        // tracked cook output, keyed by content.
        let root_signature = std::env::temp_dir().join(format!(
            "ara-root-signature-{}.json",
            &root_signature_hash()[..16]
        ));
        if !root_signature.is_file() {
            std::fs::write(&root_signature, ROOT_SIGNATURE)?;
        }
        Ok(Some(Self {
            executable,
            version,
            root_signature,
        }))
    }

    /// Whether `stage` already carries intact artifacts from this converter and ABI.
    pub fn current(&self, output: &Path, stage: &CookedStage) -> bool {
        stage.metal.as_ref().is_some_and(|metal| {
            metal.converter == self.version
                && metal.root_signature_hash == root_signature_hash()
                && output.join(&metal.reflection).is_file()
                && std::fs::read(output.join(&metal.metallib)).is_ok_and(|bytes| {
                    format!("{:x}", Sha256::digest(bytes)) == metal.metallib_hash
                })
        })
    }

    /// Convert one cooked stage's DXIL; artifacts land beside it under the same key.
    pub fn convert(&self, output: &Path, stage: &CookedStage) -> Result<CookedMetalStage> {
        let base = stage
            .dxil
            .strip_suffix(".dxil")
            .context("Cooked DXIL path has no .dxil suffix")?;
        let metallib = format!("{base}.metallib");
        let reflection = format!("{base}.metal.json");
        let result = Command::new(&self.executable)
            .arg(output.join(&stage.dxil))
            .arg("--root-signature")
            .arg(&self.root_signature)
            .args(["--minimum-os-build-version", MINIMUM_OS])
            .args(["--rename-entry-point", &stage.source_entry])
            .arg("--output-reflection-file")
            .arg(output.join(&reflection))
            .arg("-o")
            .arg(output.join(&metallib))
            .output()
            .with_context(|| format!("Starting {}", self.executable.display()))?;
        ensure!(
            result.status.success(),
            "Metal Shader Converter failed on {}:\n{}{}",
            stage.dxil,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(CookedMetalStage {
            metallib_hash: format!(
                "{:x}",
                Sha256::digest(std::fs::read(output.join(&metallib))?)
            ),
            metallib,
            reflection,
            converter: self.version.clone(),
            root_signature_hash: root_signature_hash(),
        })
    }
}

/// Report the skipped lane once per process (watch loops and editor previews cook often).
pub fn report_skipped() {
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if !REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "{}",
            ara_diag::Diagnostic::warning(
                ara_gpu::GPU_METAL_COOK_SKIPPED,
                "Metal Shader Converter not found; metallib artifacts were not cooked. \
                 Install it on the canonical cook host or set ARA_METAL_SHADERCONVERTER \
                 (docs/dev/macos.md)",
            )
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn root_signature_mirrors_the_vulkan_set_zero_layout() {
        let json: serde_json::Value = serde_json::from_str(super::ROOT_SIGNATURE).unwrap();
        let signature = &json["RootSignature"];
        assert_eq!(signature["Parameters"].as_array().unwrap().len(), 5);
        assert_eq!(signature["StaticSamplers"].as_array().unwrap().len(), 3);
        assert_eq!(signature["Parameters"][0]["Constants"]["Num32BitValues"], 8);
        assert_eq!(super::root_signature_hash().len(), 64);
    }
}
