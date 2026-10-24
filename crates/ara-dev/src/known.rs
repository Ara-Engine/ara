//! Explicit exceptions identify individual integration tests, never whole targets.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KnownFailure {
    pub(crate) package: String,
    pub(crate) target: String,
    pub(crate) test: String,
    pub(crate) output_contains: String,
    pub(crate) reason: String,
}

pub(crate) fn read(root: &Path, path: Option<&Path>) -> Result<Vec<KnownFailure>> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let path = root.join(path);
    let failures: Vec<KnownFailure> = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )?;
    for failure in &failures {
        ensure!(
            [
                &failure.package,
                &failure.target,
                &failure.test,
                &failure.output_contains,
                &failure.reason
            ]
            .iter()
            .all(|s| !s.trim().is_empty()),
            "known failures require a package, target, test, output_contains and reason"
        );
    }
    Ok(failures)
}

/// Only complete libtest failure lists with Cargo's matching target summary qualify.
/// Compilation errors, crashes, doctests and unfamiliar output always fail closed.
pub(crate) fn classify(log: &str, allowed: &[KnownFailure]) -> (bool, Vec<KnownFailure>) {
    let mut names = Vec::new();
    let mut listing = false;
    let mut failed = None;
    let mut targets = Vec::new();
    let mut summary = None;
    let mut summary_targets = Vec::new();
    let mut observed = Vec::new();
    let mut valid = true;
    let mut output = BTreeMap::<String, String>::new();
    let mut current_test = None;
    for line in log.lines().map(str::trim_end) {
        if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|s| s.strip_suffix(" stdout ----"))
        {
            current_test = Some(name.to_owned());
        } else if line == "failures:" {
            current_test = None;
        } else if let Some(name) = &current_test {
            let captured = output.entry(name.clone()).or_default();
            captured.push_str(line);
            captured.push('\n');
        }
        if line == "failures:" {
            names.clear();
            listing = true;
            failed = None;
        } else if line.starts_with("test result: FAILED.") {
            failed = line
                .split(';')
                .find_map(|part| part.trim().strip_suffix(" failed")?.parse::<usize>().ok());
            listing = false;
        } else if listing && !line.is_empty() {
            let name = line.trim();
            if line.starts_with("    ")
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || "_:".contains(c))
            {
                names.push(name.to_owned());
            } else {
                listing = false;
            }
        }
        if let Some(target) = line.strip_prefix("error: test failed, to rerun pass `") {
            let target = target.strip_suffix('`').unwrap_or_default();
            let parts: Vec<_> = target.split_whitespace().collect();
            if parts.len() != 4
                || parts[0] != "-p"
                || parts[2] != "--test"
                || failed != Some(names.len())
                || names.is_empty()
            {
                valid = false;
            } else {
                for name in &names {
                    if let Some(known) = allowed.iter().find(|known| {
                        known.package == parts[1]
                            && known.target == parts[3]
                            && known.test == *name
                            && output
                                .get(name)
                                .is_some_and(|text| text.contains(&known.output_contains))
                    }) {
                        if !observed.contains(known) {
                            observed.push(known.clone());
                        }
                    } else {
                        valid = false;
                    }
                }
            }
            targets.push(target.to_owned());
            names.clear();
            failed = None;
            listing = false;
            output.clear();
            current_test = None;
        } else if let Some(rest) = line.strip_prefix("error: ") {
            let count = rest
                .strip_suffix(" targets failed:")
                .or_else(|| rest.strip_suffix(" target failed:"))
                .and_then(|s| s.parse::<usize>().ok());
            if count.is_some() && summary.is_none() {
                summary = count;
            } else {
                valid = false;
            }
        } else if line.starts_with("error[") {
            valid = false;
        } else if summary.is_some() && !line.is_empty() {
            if let Some(target) = line
                .trim()
                .strip_prefix('`')
                .and_then(|s| s.strip_suffix('`'))
            {
                summary_targets.push(target.to_owned());
            } else {
                valid = false;
            }
        }
    }
    valid &= !targets.is_empty()
        && failed.is_none()
        && summary == Some(targets.len())
        && summary_targets == targets;
    (valid, observed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> Vec<KnownFailure> {
        vec![KnownFailure {
            package: "example".into(),
            target: "scene".into(),
            test: "known".into(),
            output_contains: "panic".into(),
            reason: "Owned upstream".into(),
        }]
    }

    fn log(names: &str, count: usize) -> String {
        format!(
            "failures:\n\n---- known stdout ----\npanic\n\nfailures:\n{names}\n\
            test result: FAILED. 0 passed; {count} failed; 0 ignored\n\
            error: test failed, to rerun pass `-p example --test scene`\n\
            error: 1 target failed:\n    `-p example --test scene`\n"
        )
    }

    #[test]
    fn exceptions_require_exact_tests_and_complete_cargo_failures() {
        let known = log("    known", 1);
        assert_eq!(classify(&known, &allowed()), (true, allowed()));
        assert!(!classify(&known, &[]).0);
        assert!(
            !classify(
                &known.replace("\npanic\n", "\ndifferent regression\n"),
                &allowed()
            )
            .0
        );
        assert!(!classify(&log("    known\n    unexpected", 2), &allowed()).0);
        assert!(!classify(&log("    known", 2), &allowed()).0);
        assert!(!classify(&format!("error[E0463]: missing crate\n{known}"), &allowed()).0);
        assert!(!classify(&format!("{known}error: doctest failed\n"), &allowed()).0);
        assert!(!classify(&known.replace("--test scene", "--lib"), &allowed()).0);
        assert!(
            !classify(
                &known.replace("    `-p example --test scene`", ""),
                &allowed()
            )
            .0
        );
        assert!(!classify("process exited abnormally", &allowed()).0);
    }
}
