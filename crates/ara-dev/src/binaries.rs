//! Cargo's artifact messages identify fresh peer executables across build directories.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use serde::Deserialize;

use crate::check::Logs;

pub(crate) type Binaries = BTreeMap<String, PathBuf>;

#[derive(Deserialize)]
pub(crate) struct Message {
    pub(crate) reason: String,
    #[serde(default)]
    package_id: String,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) target: Option<crate::workspace::Target>,
    pub(crate) profile: Option<Profile>,
    pub(crate) manifest_path: Option<PathBuf>,
}

#[derive(Deserialize)]
pub(crate) struct Profile {
    pub(crate) test: bool,
}

pub(crate) fn artifacts(output: &str) -> Result<Binaries> {
    let mut binaries = Binaries::new();
    let mut owners = BTreeMap::new();
    for line in output.lines() {
        let Ok(message) = serde_json::from_str::<Message>(line) else {
            continue;
        };
        if message.reason != "compiler-artifact" {
            continue;
        }
        let (Some(path), Some(target), Some(profile)) =
            (message.executable, message.target, message.profile)
        else {
            continue;
        };
        if profile.test || !target.kind.iter().any(|kind| kind == "bin") {
            continue;
        }
        if let Some(previous) = owners.insert(target.name.clone(), message.package_id.clone()) {
            ensure!(
                previous == message.package_id,
                "ambiguous binary name {}",
                target.name
            );
        }
        if let Some(previous) = binaries.insert(target.name.clone(), path.clone()) {
            ensure!(previous == path, "ambiguous binary path {}", target.name);
        }
    }
    Ok(binaries)
}

/// Compile tests and peer binaries in one invocation with the same feature union and profile.
pub(crate) fn prepare(
    root: &Path,
    test_args: &[String],
    metadata: &crate::workspace::Metadata,
    selected: &std::collections::BTreeSet<String>,
    logs: &mut Logs,
) -> Result<Option<Binaries>> {
    let mut compile = test_args.to_vec();
    compile[0] = "build".into();
    compile.retain(|arg| arg != "--no-fail-fast");
    if metadata
        .packages
        .iter()
        .filter(|p| selected.contains(&p.name))
        .any(|p| p.has_library())
    {
        compile.push("--lib".into());
    }
    // --tests keeps dev-dependency features active, even on library-only roots.
    // --examples preserves cargo test's default example-compilation coverage.
    compile.extend(
        [
            "--tests",
            "--bins",
            "--examples",
            "--profile",
            "test",
            "--message-format=json",
        ]
        .map(String::from),
    );
    if !logs.json_command(root, "test-build", &compile)? {
        return Ok(None);
    }
    let binaries = artifacts(&fs::read_to_string(logs.directory.join("test-build.json"))?)?;
    // Cargo can legitimately skip bins whose required features are inactive.
    for (name, path) in &binaries {
        ensure!(
            path.is_file(),
            "Cargo binary {name} is missing: {}",
            path.display()
        );
    }
    Ok(Some(binaries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_binary_names_and_paths_fail_closed() {
        let message = |package: &str, path: &str| {
            serde_json::json!({
                "reason": "compiler-artifact", "package_id": package,
                "target": {"name": "peer", "kind": ["bin"]},
                "profile": {"test": false}, "executable": path
            })
            .to_string()
        };
        let first = message("first@0.1.0", "final/peer.exe");
        for second in [
            message("other@0.1.0", "final/peer.exe"),
            message("first@0.1.0", "another/peer.exe"),
        ] {
            assert!(artifacts(&format!("{first}\n{second}")).is_err());
        }
    }

    #[test]
    fn only_regular_binary_artifacts_are_exported() {
        let message = |kind: &str, test: bool, executable: Option<&str>| {
            serde_json::json!({
                "reason": "compiler-artifact", "package_id": "peer@0.1.0",
                "target": {"name": "peer", "kind": [kind]},
                "profile": {"test": test}, "executable": executable, "fresh": true
            })
            .to_string()
        };
        let output = [
            message("bin", true, Some("intermediates/test-peer.exe")),
            message("example", false, Some("examples/peer.exe")),
            message("lib", false, None),
            message("bin", false, Some("final/peer.exe")),
            "cargo progress".into(),
        ]
        .join("\n");
        assert_eq!(
            artifacts(&output).unwrap(),
            BTreeMap::from([("peer".into(), "final/peer.exe".into())])
        );
    }
}
