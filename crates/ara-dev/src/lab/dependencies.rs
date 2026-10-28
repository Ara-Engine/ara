//! The cook host supplies the exact dependency graph; the lab does not parse Slang.
use super::{session, variants, watch};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn normalized(path: &Path) -> String {
    let path = path.to_string_lossy();
    let text = path.trim_start_matches(r"\\?\").replace('\\', "/");
    if cfg!(windows) {
        text.to_ascii_lowercase()
    } else {
        text
    }
}
pub(super) fn affected(
    root: &Path,
    base: &Path,
    manifest: &Path,
    variant: &str,
    path: &Path,
) -> Result<bool> {
    let path = normalized(path);
    if path == normalized(manifest) {
        return Ok(true);
    }
    let output = watch::output(root, &variants::cache(base, variant), manifest)?;
    let Some(index) = session::read(&output.join(".ara-shader-dependencies.json"))
        .filter(|v| v["format"] == "ara.shader-dependencies/1")
    else {
        return Ok(true);
    };
    Ok(matches(&index, &path))
}
fn matches(index: &Value, path: &str) -> bool {
    index["files"]
        .as_array()
        .is_some_and(|files| files.iter().any(|p| p == path))
        || (index["conservative"] == true
            && index["root"]
                .as_str()
                .is_some_and(|root| path.starts_with(&format!("{root}/"))))
}
pub(super) struct IndexJob {
    pub child: crate::process::Tree,
    pub log: PathBuf,
}
pub(super) fn prime(
    root: &Path,
    base: &Path,
    executable: &Path,
    manifests: &[PathBuf],
    variants: &variants::Variants,
) -> Result<IndexJob> {
    let mut jobs = Vec::new();
    for manifest in manifests {
        jobs.push(json!({"manifest":manifest,"output":watch::output(root,&variants::cache(base,"baseline"),manifest)?}));
        for name in variants.affected(manifest.strip_prefix(root)?) {
            jobs.push(json!({"manifest":manifest,"output":watch::output(root,&variants::cache(base,&name),manifest)?,"variant":variants.recipe_path(base,&name,manifest.strip_prefix(root)?)?}));
        }
    }
    let directory = base.join("dependency-index").join(session::id());
    fs::create_dir_all(&directory)?;
    let input = directory.join("catalog.json");
    session::write(&input, &jobs)?;
    let log = directory.join("index.log");
    let mut command = Command::new(executable);
    command
        .current_dir(root)
        .arg("--dependencies")
        .arg(input)
        .stdout(fs::File::create(&log)?)
        .stderr(
            fs::File::options()
                .append(true)
                .open(&log)
                .context("dependency log")?,
        );
    super::build::background(&mut command);
    Ok(IndexJob {
        child: crate::process::Tree::spawn(&mut command)?,
        log,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_edges_include_cross_package_headers_but_not_experimental_siblings() {
        let index = json!({"root":"/repo/shaders","files":["/repo/shaders/baseline.slang","/repo/crates/ara-gpu/common.slangh"],"conservative":false});
        assert!(matches(&index, "/repo/crates/ara-gpu/common.slangh"));
        assert!(!matches(&index, "/repo/shaders/experiment.slang"));
        let index = json!({"root":"/repo/shaders","files":[],"conservative":true});
        assert!(matches(&index, "/repo/shaders/new-module.slang"));
        assert!(!matches(&index, "/repo/shaders-other/a.slang"));
    }
}
