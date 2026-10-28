#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Offline Slang compiler orchestration. The engine never links this crate.
mod dependency;
pub use dependency::write_index as write_dependency_index;
mod identity;
mod metal;
pub mod variant;
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
    /// Offline PSO definitions; absent from the baseline unless explicitly promoted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub defines: BTreeMap<String, String>,
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
/// Reachable includes invalidate their pipelines; ambiguous imports use a conservative tree hash.
pub fn cook(manifest: &Path, output: &Path, tools: &Toolchain) -> Result<usize> {
    cook_with_variant(manifest, output, tools, None)
}

/// Cook an experimental recipe into a separate output, preserving the baseline source.
pub fn cook_with_variant(
    manifest: &Path,
    output: &Path,
    tools: &Toolchain,
    variant: Option<&variant::Variant>,
) -> Result<usize> {
    let source_root = manifest.parent().context("Manifest has no parent")?;
    let mut manifest_bytes = std::fs::read(manifest)?;
    let mut pipelines: CookManifest = serde_json::from_slice(&manifest_bytes)?;
    if let Some(variant) = variant {
        variant.apply(&mut pipelines, source_root)?;
        manifest_bytes = serde_json::to_vec(&pipelines)?;
    }
    dependency::write(manifest, output, &pipelines)?;
    let identity = identity::load(tools, output)?;
    let compiler = identity.version;
    let compiler_key = identity.key;
    let converter = metal::Converter::discover()?;
    if converter.is_none() {
        metal::report_skipped();
    }
    // Unreferenced experimental siblings must not alter baseline provenance.
    // Each input still falls back to the full tree for unresolved module/includes.
    let fingerprints = pipelines
        .iter()
        .map(|(name, pipeline)| {
            Ok((
                name.clone(),
                pipeline_input_key(source_root, pipeline, &compiler_key)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut source_hash = Sha256::new();
    source_hash.update(&compiler_key);
    source_hash.update(manifest_bytes);
    for (name, input) in &fingerprints {
        source_hash.update(name.as_bytes());
        source_hash.update(input.as_bytes());
    }
    let key = hash(source_hash.finalize());
    std::fs::create_dir_all(output)?;
    let cache_path = output.join(".ara-pipeline-inputs.json");
    let previous: BTreeMap<String, String> = std::fs::read(&cache_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut inputs = BTreeMap::new();
    let mut publication = Vec::new();
    let mut pending = Vec::new();
    for (name, pipeline) in pipelines {
        variant::validate_defines(&pipeline.defines)?;
        ensure!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "Invalid pipeline name"
        );
        let path = output.join(format!("{name}.json"));
        let input_key = fingerprints
            .get(&name)
            .context("pipeline input fingerprint")?
            .clone();
        inputs.insert(name.clone(), input_key.clone());
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(mut old) = serde_json::from_slice::<CookedPipeline>(&bytes)
            && old.binding_layout_hash == binding_layout_hash()
            && (old.key == key
                || previous
                    .get(&name)
                    .is_some_and(|cached| *cached == format!("{input_key}:{}", hash(&bytes))))
            && old.stages.iter().all(|s| {
                std::fs::read(output.join(&s.spirv)).is_ok_and(|b| hash(b) == s.spirv_hash)
                    && std::fs::read(output.join(&s.dxil)).is_ok_and(|b| hash(b) == s.dxil_hash)
                    && output.join(&s.spirv).with_extension("spirv.json").exists()
                    && output.join(&s.dxil).with_extension("dxil.json").exists()
            })
        {
            let converted = convert_stages(converter.as_ref(), output, &mut old.stages)?;
            if old.key != key || converted {
                old.key = key.clone();
                publication.push((path, old));
            }
            continue;
        }
        pending.push((name, pipeline, path));
    }
    // Bound the expensive compiler lane to four BelowNormal-inherited workers.
    // All artifacts complete before publishing any metadata from the cohort.
    let rebuilt = pending.len();
    let compiler = Compiler {
        source_root,
        output,
        tools,
        key: &key,
        version: &compiler,
        converter: converter.as_ref(),
    };
    for batch in pending.chunks(4) {
        let compiled = std::thread::scope(|scope| -> Result<Vec<_>> {
            let jobs = batch
                .iter()
                .map(|(name, pipeline, path)| {
                    let compiler = &compiler;
                    scope.spawn(move || {
                        compiler
                            .pipeline(name, pipeline)
                            .map(|cooked| (path.clone(), cooked))
                    })
                })
                .collect::<Vec<_>>();
            jobs.into_iter()
                .map(|job| {
                    job.join()
                        .map_err(|_| anyhow::anyhow!("shader compiler worker panicked"))?
                })
                .collect()
        })?;
        publication.extend(compiled);
    }
    // A failed compiler invocation never publishes any part of the new cohort.
    let changed = !publication.is_empty();
    for (path, cooked) in publication {
        let temporary = path.with_extension("json.tmp");
        ara_gpu::cooked::write_json(&temporary, &cooked)?;
        std::fs::rename(temporary, path)?;
    }
    // Associate local input keys with the metadata actually published. Restoring
    // another checkout's metadata must not inherit this checkout's cache hits.
    for (name, input) in &mut inputs {
        let metadata = std::fs::read(output.join(format!("{name}.json")))?;
        *input = format!("{input}:{}", hash(metadata));
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

struct Compiler<'a> {
    source_root: &'a Path,
    output: &'a Path,
    tools: &'a Toolchain,
    key: &'a str,
    version: &'a str,
    converter: Option<&'a metal::Converter>,
}
impl Compiler<'_> {
    fn pipeline(&self, name: &str, pipeline: &PipelineSource) -> Result<CookedPipeline> {
        let Self {
            source_root,
            output,
            tools,
            key,
            version,
            converter,
        } = self;
        // Two permutations may use the same entry name. Their artifacts must not alias.
        let prefix = format!("{key}/{name}");
        let artifacts = output.join(&prefix);
        std::fs::create_dir_all(&artifacts)?;
        let mut stages = Vec::new();
        for (entry, stage) in &pipeline.entries {
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
                    entry,
                    "-stage",
                    stage.slang_name(),
                    "-target",
                    target,
                    "-profile",
                    profile,
                    "-O2",
                    "-matrix-layout-column-major",
                ]);
                for (name, value) in &pipeline.defines {
                    cmd.arg("-D").arg(format!("{name}={value}"));
                }
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
                stage: *stage,
                spirv: format!("{prefix}/{entry}.spv"),
                dxil: format!("{prefix}/{entry}.dxil"),
                spirv_hash: hash(std::fs::read(artifacts.join(format!("{entry}.spv")))?),
                dxil_hash: hash(std::fs::read(artifacts.join(format!("{entry}.dxil")))?),
                metal: None,
            });
        }
        convert_stages(*converter, output, &mut stages)?;
        Ok(CookedPipeline {
            version: 1,
            key: (*key).to_owned(),
            binding_layout_hash: binding_layout_hash(),
            compiler: version.trim().into(),
            stages,
            state: pipeline.state.clone(),
        })
    }
}

/// Fill missing or outdated Metal artifacts when the converter is installed; returns
/// whether any stage changed. Without a converter, existing artifacts are kept.
fn convert_stages(
    converter: Option<&metal::Converter>,
    output: &Path,
    stages: &mut [CookedStage],
) -> Result<bool> {
    let Some(converter) = converter else {
        return Ok(false);
    };
    let mut changed = false;
    for stage in stages {
        if !converter.current(output, stage) {
            stage.metal = Some(converter.convert(output, stage)?);
            changed = true;
        }
    }
    Ok(changed)
}

fn pipeline_input_key(root: &Path, pipeline: &PipelineSource, compiler: &str) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(compiler.as_bytes());
    digest.update(serde_json::to_vec(pipeline)?);
    if dependency::visit(
        &root.join(&pipeline.source),
        &mut BTreeSet::new(),
        &mut |source| {
            digest.update(source.as_bytes());
        },
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
