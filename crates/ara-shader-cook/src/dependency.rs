//! Literal dependencies share the cook fingerprint's traversal and fallback policy.
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(super) fn visit(
    file: &Path,
    seen: &mut BTreeSet<PathBuf>,
    consume: &mut impl FnMut(&str),
) -> Result<()> {
    let file = file.canonicalize()?;
    if !seen.insert(file.clone()) {
        return Ok(());
    }
    let source = std::fs::read_to_string(&file)?;
    consume(&source);
    for line in source.lines().map(str::trim) {
        if let Some(include) = line.strip_prefix("#include") {
            if let Some(name) = include
                .trim()
                .strip_prefix('"')
                .and_then(|s| s.split('"').next())
            {
                visit(
                    &file.parent().context("include parent")?.join(name),
                    seen,
                    consume,
                )?;
            } else {
                bail!("non-literal shader include: {line}");
            }
        } else if line.starts_with("import ") || line.starts_with("# include") {
            bail!("shader dependency requires conservative invalidation");
        }
    }
    Ok(())
}
fn normalized(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text.trim_start_matches(r"\\?\").replace('\\', "/");
    if cfg!(windows) {
        text.to_ascii_lowercase()
    } else {
        text
    }
}
#[derive(Serialize)]
struct Index {
    format: &'static str,
    root: String,
    files: BTreeSet<String>,
    conservative: bool,
}
pub(super) fn write(manifest: &Path, output: &Path, pipelines: &super::CookManifest) -> Result<()> {
    let root = manifest.parent().context("shader root")?.canonicalize()?;
    let mut seen = BTreeSet::new();
    let mut conservative = false;
    for pipeline in pipelines.values() {
        conservative |= visit(&root.join(&pipeline.source), &mut seen, &mut |_| {}).is_err();
    }
    let index = Index {
        format: "ara.shader-dependencies/1",
        root: normalized(&root),
        files: seen.iter().map(|p| normalized(p)).collect(),
        conservative,
    };
    std::fs::create_dir_all(output)?;
    ara_gpu::cooked::write_json(&output.join(".ara-shader-dependencies.json"), &index)
}

/// Write dependency metadata without loading or invoking any compiler SDK tool.
pub fn write_index(
    manifest: &Path,
    output: &Path,
    variant: Option<&super::variant::Variant>,
) -> Result<()> {
    let mut pipelines = serde_json::from_slice(&std::fs::read(manifest)?)?;
    if let Some(variant) = variant {
        variant.apply(&mut pipelines, manifest.parent().context("shader root")?)?;
    }
    write(manifest, output, &pipelines)
}
