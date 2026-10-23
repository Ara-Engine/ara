#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Offline Slang compiler orchestration. The engine never links this crate.
use anyhow::{Context, Result, ensure};
use ara_gpu::cooked::{CookedPipeline, CookedStage, RenderState, Stage, binding_layout_hash};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

/// One input pipeline in a shader manifest.
#[derive(Deserialize)]
pub struct PipelineSource {
    /// Slang source path relative to the manifest.
    pub source: String,
    /// Source entry point to stage mapping, sorted canonically.
    pub entries: BTreeMap<String, Stage>,
    /// Formats and graphics state.
    pub state: RenderState,
}
/// A manifest maps pipeline names to independent offline cooks.
pub type CookManifest = BTreeMap<String, PipelineSource>;

/// Paths to the SDK tools. Compilation and validation are both mandatory.
pub struct Toolchain {
    /// Slang compiler executable.
    pub slangc: PathBuf,
    /// SPIR-V validator executable.
    pub spirv_val: PathBuf,
}
fn run(command: &mut Command) -> Result<Vec<u8>> {
    let output = command
        .output()
        .with_context(|| format!("Starting {command:?}"))?;
    ensure!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut bytes = output.stdout;
    bytes.extend(output.stderr);
    Ok(bytes)
}
fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

/// Cook all reachable pipelines; unchanged, intact artifacts are reused.
/// Source directories are hashed recursively so changes to includes invalidate cooks.
pub fn cook(manifest: &Path, output: &Path, tools: &Toolchain) -> Result<usize> {
    let source_root = manifest.parent().context("Manifest has no parent")?;
    let manifest_bytes = std::fs::read(manifest)?;
    let pipelines: CookManifest = serde_json::from_slice(&manifest_bytes)?;
    let compiler = String::from_utf8(run(Command::new(&tools.slangc).arg("-version"))?)?;
    let mut source_hash = Sha256::new();
    let mut files = Vec::new();
    collect_sources(source_root, &mut files)?;
    files.sort();
    for file in files {
        source_hash.update(file.strip_prefix(source_root)?.to_string_lossy().as_bytes());
        source_hash.update(std::fs::read(file)?);
    }
    source_hash.update(b"ara-cook-2|sm_6_6+spirv_1_6|sm_6_6|-O2|column-major|VULKAN|invert-y-vertex|vulkan1.3-validation");
    source_hash.update(std::fs::read(&tools.slangc)?);
    if let Some(bin) = tools.slangc.parent() {
        for name in ["slang.dll", "slang-glslang.dll", "dxcompiler.dll"] {
            if let Ok(bytes) = std::fs::read(bin.join(name)) {
                source_hash.update(name.as_bytes());
                source_hash.update(bytes);
            }
        }
    }
    source_hash.update(compiler.as_bytes());
    source_hash.update(binding_layout_hash());
    source_hash.update(manifest_bytes);
    let key = hash(source_hash.finalize());
    std::fs::create_dir_all(output)?;
    let artifacts = output.join(&key);
    std::fs::create_dir_all(&artifacts)?;
    let mut rebuilt = 0;
    for (name, pipeline) in pipelines {
        ensure!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "Invalid pipeline name"
        );
        let path = output.join(format!("{name}.json"));
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(old) = serde_json::from_slice::<CookedPipeline>(&bytes)
            && old.key == key
            && old.stages.iter().all(|s| {
                std::fs::read(output.join(&s.spirv)).is_ok_and(|b| hash(b) == s.spirv_hash)
                    && std::fs::read(output.join(&s.dxil)).is_ok_and(|b| hash(b) == s.dxil_hash)
                    && output.join(&s.spirv).with_extension("spirv.json").exists()
                    && output.join(&s.dxil).with_extension("dxil.json").exists()
            })
        {
            continue;
        }
        let mut stages = Vec::new();
        for (entry, stage) in pipeline.entries {
            ensure!(
                entry.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "Invalid shader entry"
            );
            for (target, suffix, profile) in [
                ("spirv", "spv", "sm_6_6+spirv_1_6"),
                ("dxil", "dxil", "sm_6_6"),
            ] {
                let mut cmd = Command::new(&tools.slangc);
                cmd.arg(source_root.join(&pipeline.source)).args([
                    "-entry",
                    &entry,
                    "-stage",
                    stage.slang_name(),
                    "-target",
                    target,
                    "-profile",
                    profile,
                    "-O2",
                    "-matrix-layout-column-major",
                ]);
                if target == "spirv" {
                    cmd.args(["-D", "VULKAN=1"]);
                    if matches!(stage, Stage::Vertex | Stage::Mesh) {
                        cmd.arg("-fvk-invert-y");
                    }
                }
                cmd.arg("-reflection-json")
                    .arg(artifacts.join(format!("{entry}.{target}.json")))
                    .arg("-o")
                    .arg(artifacts.join(format!("{entry}.{suffix}")));
                run(&mut cmd)?;
                if target == "spirv" {
                    run(Command::new(&tools.spirv_val)
                        .args(["--target-env", "vulkan1.3"])
                        .arg(artifacts.join(format!("{entry}.spv"))))?;
                }
            }
            stages.push(CookedStage {
                source_entry: entry.clone(),
                spirv_entry: "main".into(),
                stage,
                spirv: format!("{key}/{entry}.spv"),
                dxil: format!("{key}/{entry}.dxil"),
                spirv_hash: hash(std::fs::read(artifacts.join(format!("{entry}.spv")))?),
                dxil_hash: hash(std::fs::read(artifacts.join(format!("{entry}.dxil")))?),
                metal: None,
            });
        }
        let cooked = CookedPipeline {
            version: 1,
            key: key.clone(),
            binding_layout_hash: binding_layout_hash(),
            compiler: compiler.trim().into(),
            stages,
            state: pipeline.state,
        };
        // Struct field order and BTreeMap ordering define this derived manifest writer.
        let temporary = path.with_extension("json.tmp");
        ara_gpu::cooked::write_json(&temporary, &cooked)?;
        std::fs::rename(temporary, path)?;
        rebuilt += 1;
    }
    Ok(rebuilt)
}
fn collect_sources(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_sources(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|e| e == "slang" || e == "h" || e == "slangh")
        {
            files.push(path);
        }
    }
    Ok(())
}
