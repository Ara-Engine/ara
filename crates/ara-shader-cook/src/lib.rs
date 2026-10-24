#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Offline Slang compiler orchestration. The engine never links this crate.
use anyhow::{Context, Result, ensure};
use ara_gpu::cooked::{CookedPipeline, CookedStage, RenderState, Stage, binding_layout_hash};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Command,
};

/// One input pipeline in a shader manifest.
#[derive(Deserialize, serde::Serialize)]
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

impl Toolchain {
    /// Prefer bundled tools; SDK and PATH support development checkouts.
    pub fn discover() -> Self {
        let tool = |name: &str| {
            let name = format!("{name}{}", std::env::consts::EXE_SUFFIX);
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("tools").join(&name)))
                .filter(|path| path.is_file())
                .or_else(|| {
                    std::env::var_os("VULKAN_SDK").map(|sdk| {
                        PathBuf::from(sdk)
                            .join(if cfg!(windows) { "Bin" } else { "bin" })
                            .join(&name)
                    })
                })
                .unwrap_or_else(|| PathBuf::from(name))
        };
        Self {
            slangc: tool("slangc"),
            spirv_val: tool("spirv-val"),
        }
    }
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
    let mut compiler_hash = Sha256::new();
    let recipe = b"ara-cook-3|sm_6_6+spirv_1_6|sm_6_6|-O2|column-major|VULKAN|invert-y-vertex-mesh|vulkan1.3-validation";
    source_hash.update(recipe);
    compiler_hash.update(recipe);
    let executable = std::fs::read(&tools.slangc)?;
    source_hash.update(&executable);
    compiler_hash.update(&executable);
    if let Some(bin) = tools.slangc.parent() {
        for name in [
            "slang.dll",
            "slang-compiler.dll",
            "slang-glslang.dll",
            "slang-glsl-module.dll",
            "slang-rt.dll",
            "dxcompiler.dll",
        ] {
            if let Ok(bytes) = std::fs::read(bin.join(name)) {
                source_hash.update(name.as_bytes());
                source_hash.update(&bytes);
                compiler_hash.update(name.as_bytes());
                compiler_hash.update(&bytes);
            }
        }
    }
    source_hash.update(compiler.as_bytes());
    source_hash.update(binding_layout_hash());
    source_hash.update(manifest_bytes);
    compiler_hash.update(compiler.as_bytes());
    compiler_hash.update(binding_layout_hash());
    let compiler_key = hash(compiler_hash.finalize());
    let key = hash(source_hash.finalize());
    std::fs::create_dir_all(output)?;
    let artifacts = output.join(&key);
    std::fs::create_dir_all(&artifacts)?;
    let mut rebuilt = 0;
    let cache_path = output.join(".ara-pipeline-inputs.json");
    let previous: BTreeMap<String, String> = std::fs::read(&cache_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut inputs = BTreeMap::new();
    let mut publication = Vec::new();
    for (name, pipeline) in pipelines {
        ensure!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "Invalid pipeline name"
        );
        let path = output.join(format!("{name}.json"));
        let input_key = pipeline_input_key(source_root, &pipeline, &compiler_key)?;
        inputs.insert(name.clone(), input_key.clone());
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(mut old) = serde_json::from_slice::<CookedPipeline>(&bytes)
            && (old.key == key || previous.get(&name) == Some(&input_key))
            && old.stages.iter().all(|s| {
                std::fs::read(output.join(&s.spirv)).is_ok_and(|b| hash(b) == s.spirv_hash)
                    && std::fs::read(output.join(&s.dxil)).is_ok_and(|b| hash(b) == s.dxil_hash)
                    && output.join(&s.spirv).with_extension("spirv.json").exists()
                    && output.join(&s.dxil).with_extension("dxil.json").exists()
            })
        {
            if old.key != key {
                old.key = key.clone();
                publication.push((path, old));
            }
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
        publication.push((path, cooked));
        rebuilt += 1;
    }
    // A failed compiler invocation never publishes any part of the new cohort.
    let changed = !publication.is_empty();
    for (path, cooked) in publication {
        let temporary = path.with_extension("json.tmp");
        ara_gpu::cooked::write_json(&temporary, &cooked)?;
        std::fs::rename(temporary, path)?;
    }
    ara_gpu::cooked::write_json(&cache_path, &inputs)?;
    if changed {
        let signal = output.join(".ara-shader-reload");
        let temporary = signal.with_extension("tmp");
        std::fs::write(&temporary, &key)?;
        std::fs::rename(temporary, signal)?;
    }
    Ok(rebuilt)
}

fn pipeline_input_key(root: &Path, pipeline: &PipelineSource, compiler: &str) -> Result<String> {
    fn includes(file: &Path, seen: &mut BTreeSet<PathBuf>, digest: &mut Sha256) -> Result<()> {
        let file = file.canonicalize()?;
        if !seen.insert(file.clone()) {
            return Ok(());
        }
        let source = std::fs::read_to_string(&file)?;
        digest.update(source.as_bytes());
        for line in source.lines().map(str::trim) {
            if let Some(include) = line.strip_prefix("#include") {
                let include = include.trim();
                if let Some(name) = include.strip_prefix('"').and_then(|s| s.split('"').next()) {
                    includes(
                        &file.parent().context("include parent")?.join(name),
                        seen,
                        digest,
                    )?;
                } else {
                    anyhow::bail!("non-literal shader include: {line}");
                }
            } else if line.starts_with("import ") || line.starts_with("# include") {
                anyhow::bail!("shader dependency requires conservative invalidation");
            }
        }
        Ok(())
    }
    let mut digest = Sha256::new();
    digest.update(compiler.as_bytes());
    digest.update(serde_json::to_vec(pipeline)?);
    if includes(
        &root.join(&pipeline.source),
        &mut BTreeSet::new(),
        &mut digest,
    )
    .is_err()
    {
        // Macro/system/module imports may hide dependencies. Hash the entire source tree.
        let mut files = Vec::new();
        collect_sources(root, &mut files)?;
        files.sort();
        for file in files {
            digest.update(std::fs::read(file)?);
        }
    }
    Ok(hash(digest.finalize()))
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

/// Content fingerprint for a watch loop, including recursive Slang headers and the manifest.
pub fn source_fingerprint(manifest: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_sources(manifest.parent().context("manifest parent")?, &mut files)?;
    files.push(manifest.to_owned());
    files.sort();
    let mut digest = Sha256::new();
    for file in files {
        digest.update(file.to_string_lossy().as_bytes());
        digest.update(std::fs::read(file)?);
    }
    Ok(hash(digest.finalize()))
}
