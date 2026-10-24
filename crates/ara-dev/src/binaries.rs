//! Cargo's artifact messages identify fresh peer executables across build directories.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use serde::Deserialize;

use crate::{check::Logs, workspace::Metadata};

pub(crate) type Binaries = BTreeMap<String, PathBuf>;

#[derive(Deserialize)]
struct Message {
    reason: String,
    #[serde(default)]
    package_id: String,
    executable: Option<PathBuf>,
    target: Option<crate::workspace::Target>,
    profile: Option<Profile>,
}

#[derive(Deserialize)]
struct Profile {
    test: bool,
}

fn artifacts(output: &str) -> Result<Binaries> {
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

/// Compile the selected tests, then prepare any peer binaries without integration tests.
pub(crate) fn prepare(
    root: &Path,
    metadata: &Metadata,
    affected: &BTreeSet<String>,
    test_args: &[String],
    logs: &mut Logs,
) -> Result<Option<Binaries>> {
    let mut compile = test_args.to_vec();
    compile.retain(|arg| arg != "--no-fail-fast");
    compile.extend(["--no-run".into(), "--message-format=json".into()]);
    if !logs.json_command(root, "test-build", &compile)? {
        return Ok(None);
    }
    let mut binaries = artifacts(&fs::read_to_string(logs.directory.join("test-build.json"))?)?;
    let missing: BTreeSet<_> = metadata
        .packages
        .iter()
        .filter(|p| affected.contains(&p.name))
        .filter(|p| {
            p.targets
                .iter()
                .any(|t| t.kind.iter().any(|kind| kind == "bin") && !binaries.contains_key(&t.name))
        })
        .map(|p| p.name.clone())
        .collect();
    if !missing.is_empty() {
        let mut build = vec![
            "build".into(),
            "--bins".into(),
            "--locked".into(),
            "--message-format=json".into(),
        ];
        for package in missing {
            build.extend(["-p".into(), package]);
        }
        if !logs.json_command(root, "test-binaries", &build)? {
            return Ok(None);
        }
        binaries.extend(artifacts(&fs::read_to_string(
            logs.directory.join("test-binaries.json"),
        )?)?);
    }
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
