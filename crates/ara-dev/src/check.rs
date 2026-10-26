//! One report combines cheap hygiene checks with logged Cargo gates.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::{
    git, hygiene, known, process,
    selection::{self, Selection},
    workspace::Metadata,
};

#[derive(Serialize)]
struct Row {
    check: String,
    passed: bool,
    known_failure: bool,
    timed_out: bool,
    seconds: f64,
    detail: String,
}

#[derive(Serialize)]
struct Report {
    passed: bool,
    elapsed_seconds: f64,
    compiler_cache_bypassed: bool,
    base: String,
    #[serde(flatten)]
    selection: Selection,
    known_failures: Vec<known::KnownFailure>,
    checks: Vec<Row>,
}

pub(crate) struct Logs {
    pub(crate) directory: PathBuf,
    rows: Vec<Row>,
    deadline: Option<Instant>,
    no_compiler_cache: bool,
    test_binaries: crate::binaries::Binaries,
    cargo_directories: crate::build_paths::Directories,
}

struct CargoStep {
    command: Command,
    deadline: Option<Instant>,
}

impl Logs {
    pub(crate) fn new(root: &Path, no_compiler_cache: bool) -> Result<Self> {
        let cargo_directories = crate::build_paths::Directories::from_env(root)?;
        let directory = root.join("target/ara-dev").join(format!(
            "run-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
        ));
        fs::create_dir_all(&directory)?;
        Ok(Self {
            directory,
            rows: Vec::new(),
            deadline: None,
            no_compiler_cache,
            test_binaries: Default::default(),
            cargo_directories,
        })
    }

    pub(crate) fn command(&mut self, root: &Path, name: &str, args: &[String]) -> Result<bool> {
        let log = self.directory.join(format!("{name}.log"));
        self.execute(root, name, args, &log, &log)
    }

    pub(crate) fn json_command(
        &mut self,
        root: &Path,
        name: &str,
        args: &[String],
    ) -> Result<bool> {
        let output = self.directory.join(format!("{name}.json"));
        let log = self.directory.join(format!("{name}.log"));
        self.execute(root, name, args, &output, &log)
    }

    fn capture(&mut self, root: &Path, name: &str, args: &[String]) -> Result<Vec<u8>> {
        let output = self.directory.join(format!("{name}.json"));
        let log = self.directory.join(format!("{name}.log"));
        if !self.execute(root, name, args, &output, &log)? {
            bail!(
                "{}; {}",
                self.rows.last().context("command result")?.detail,
                fs::read_to_string(log).unwrap_or_default()
            );
        }
        Ok(fs::read(output)?)
    }

    fn metadata(&mut self, root: &Path) -> Result<Metadata> {
        serde_json::from_slice(
            &self.capture(
                root,
                "metadata",
                &[
                    "metadata",
                    "--format-version",
                    "1",
                    "--no-deps",
                    "--locked",
                    "--offline",
                ]
                .map(String::from),
            )?,
        )
        .context("read workspace metadata")
    }

    fn execute(
        &mut self,
        root: &Path,
        name: &str,
        args: &[String],
        stdout: &Path,
        log: &Path,
    ) -> Result<bool> {
        eprintln!("{name}: cargo {} (log: {})", args.join(" "), log.display());
        let start = Instant::now();
        let output = File::create(stdout)?;
        let errors = if stdout == log {
            output.try_clone()?
        } else {
            File::create(log)?
        };
        let mut step = self.cargo_step(root, name, args);
        step.command
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(errors));
        let result = crate::cargo_wait::run(&mut step.command, &mut step.deadline, stdout, log);
        self.deadline = step.deadline;
        let waited = result.as_ref().map_or(Duration::ZERO, |(_, wait)| *wait);
        let status = result.map(|(outcome, _)| outcome);
        let passed = status.as_ref().is_ok_and(process::Outcome::success);
        let timed_out = matches!(status, Ok(process::Outcome::TimedOut));
        let mut detail = match status {
            Ok(process::Outcome::Exited(s)) => format!("{s}; {}", log.display()),
            Ok(process::Outcome::TimedOut) => format!(
                "timed out at the check deadline (package-cache waits excluded); {}",
                log.display()
            ),
            Err(e) => format!("{e}; {}", log.display()),
        };
        if !waited.is_zero() {
            detail.push_str(&format!(
                "; {:.2}s package-cache wait excluded",
                waited.as_secs_f64()
            ));
        }
        self.rows.push(Row {
            check: name.into(),
            passed,
            known_failure: false,
            timed_out,
            seconds: start.elapsed().as_secs_f64(),
            detail,
        });
        Ok(passed)
    }

    fn cargo_step(&self, root: &Path, name: &str, args: &[String]) -> CargoStep {
        let mut command = Command::new("cargo");
        self.cargo_directories.apply(&mut command);
        crate::compile_cache::configure(&mut command);
        command
            .current_dir(root)
            .args(args)
            .env("CARGO_TERM_COLOR", "never");
        if self.no_compiler_cache {
            // An empty wrapper overrides Cargo config; clippy's workspace wrapper stays active.
            command.env("RUSTC_WRAPPER", "");
        }
        if name == "tests" {
            for (binary, path) in &self.test_binaries {
                command.env(format!("CARGO_BIN_EXE_{binary}"), path);
            }
        }
        if matches!(name, "doctests" | "diagnostic-coverage") {
            command.env("RUST_TEST_THREADS", "2");
        }
        CargoStep {
            command,
            deadline: self.deadline,
        }
    }

    fn findings(&mut self, name: &str, findings: Vec<String>) {
        self.rows.push(Row {
            check: name.into(),
            passed: findings.is_empty(),
            known_failure: false,
            timed_out: false,
            seconds: 0.0,
            detail: if findings.is_empty() {
                "ok".into()
            } else {
                findings.join("\n")
            },
        });
    }

    /// Cargo fmt forwards target paths to rustfmt; batch to stay below Windows' limit.
    fn formatting(
        &mut self,
        root: &Path,
        metadata: &Metadata,
        selected: &std::collections::BTreeSet<String>,
        check: bool,
    ) -> Result<bool> {
        let start = Instant::now();
        let first = self.rows.len();
        let packages: Vec<_> = metadata
            .packages
            .iter()
            .filter(|p| selected.contains(&p.name))
            .collect();
        for (index, packages) in packages.chunks(4).enumerate() {
            let mut args = vec!["fmt".into()];
            for package in packages {
                args.extend(["-p".into(), package.name.clone()]);
            }
            if check {
                args.extend(["--".into(), "--check".into()]);
            }
            if !self.command(root, &format!("fmt-{}", index + 1), &args)? {
                break;
            }
        }
        let batches = self.rows.split_off(first);
        let passed = batches.iter().all(|row| row.passed);
        let timed_out = batches.iter().any(|row| row.timed_out);
        let detail = batches
            .iter()
            .find(|row| !row.passed)
            .map(|row| row.detail.clone())
            .unwrap_or_else(|| {
                format!(
                    "{} packages in {} batches; {}",
                    packages.len(),
                    batches.len(),
                    self.directory.display()
                )
            });
        self.rows.push(Row {
            check: "fmt".into(),
            passed,
            known_failure: false,
            timed_out,
            seconds: start.elapsed().as_secs_f64(),
            detail,
        });
        Ok(passed)
    }
}

/// Run the gate's formatter alone; plain `fmt` writes and `--check` only verifies.
pub(crate) fn format(root: &Path, check: bool) -> Result<bool> {
    let mut logs = Logs::new(root, false)?;
    let metadata = logs.metadata(root)?;
    let packages = metadata.packages.iter().map(|p| p.name.clone()).collect();
    let passed = logs.formatting(root, &metadata, &packages, check)?;
    eprintln!(
        "Formatting {}: {}",
        if passed { "passed" } else { "failed" },
        logs.rows.last().context("formatting result")?.detail
    );
    Ok(passed)
}

pub(crate) fn selection(
    command: &str,
    affected: &std::collections::BTreeSet<String>,
    full: bool,
) -> Vec<String> {
    let mut args = vec![command.into()];
    if full {
        args.push("--workspace".into());
    } else {
        for package in affected {
            args.extend(["-p".into(), package.clone()]);
        }
    }
    args
}

pub(crate) struct Options<'a> {
    pub(crate) base: &'a str,
    pub(crate) json: bool,
    pub(crate) full: bool,
    pub(crate) known_path: Option<&'a Path>,
    pub(crate) plan: bool,
    pub(crate) timeout_seconds: Option<u64>,
    pub(crate) no_compiler_cache: bool,
}

pub(crate) fn run(root: &Path, options: Options<'_>) -> Result<bool> {
    let started = Instant::now();
    let Options {
        base,
        json,
        full,
        known_path,
        plan,
        timeout_seconds,
        no_compiler_cache,
    } = options;
    let budget = timeout_seconds
        .or(if full { None } else { Some(2400) })
        .filter(|s| *s != 0);
    let allowed = known::read(root, known_path)?;
    let mut known_failures = Vec::new();
    // Capture before metadata, too: Cargo itself must not silently rewrite Cargo.lock.
    let before = git::TreeState::read(root)?;
    let base = git::text(root, ["merge-base", "HEAD", base])
        .context("resolve --base (use an existing local ref)")?;
    let changed = git::changed(root, &base)?;
    let mut logs = Logs::new(root, no_compiler_cache)?;
    if no_compiler_cache {
        eprintln!("Compiler cache bypass: RUSTC_WRAPPER disabled for Cargo gates.");
    }
    logs.deadline = budget
        .map(|seconds| {
            started
                .checked_add(Duration::from_secs(seconds))
                .context("timeout is too large")
        })
        .transpose()?;
    let metadata = logs.metadata(root)?;
    eprintln!("Cargo target: {}", metadata.target_directory.display());
    let mut scope = selection::compute(root, &base, &changed, &metadata)?;
    if full {
        scope.affected = metadata.packages.iter().map(|p| p.name.clone()).collect();
        scope.test_packages = scope.affected.clone();
        scope
            .reasons
            .push("--full requested the whole workspace".into());
    }
    if plan {
        before.verify(&git::TreeState::read(root)?)?;
        if json {
            #[derive(Serialize)]
            struct Plan<'a> {
                planned: bool,
                base: &'a str,
                #[serde(flatten)]
                selection: &'a Selection,
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&Plan {
                    planned: true,
                    base: &base,
                    selection: &scope
                })?
            );
        } else {
            println!(
                "Plan: {} affected packages: {}",
                scope.affected.len(),
                scope
                    .affected
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!(
                "Tests: {} source/direct-consumer packages (transitive consumers compile only)",
                scope.test_packages.len()
            );
            for reason in &scope.reasons {
                println!("  {reason}");
            }
        }
        return Ok(true);
    }
    let affected = &scope.affected;
    let _build_lease = crate::compile_cache::acquire(logs.deadline)?;
    if affected.contains("ara-dev") || full {
        crate::platform::release_executable_path(&metadata.target_directory)?;
    }
    let files = git::files(root)?;
    logs.findings("exact pins", hygiene::manifests(root, &changed)?);
    logs.findings("AGENTS.md", hygiene::agents(root, &files)?);
    logs.findings(
        "importer versions",
        hygiene::importer_versions(root, &files)?,
    );
    // Formatting-only edits still receive fmt verification without cascading builds.
    let mut format_packages = scope.affected.clone();
    format_packages.extend(
        changed
            .iter()
            .filter_map(|p| metadata.owner(root, p).map(|p| p.name.clone())),
    );
    let fmt = logs.formatting(root, &metadata, &format_packages, true)?;
    let ready = fmt && logs.rows.iter().all(|row| row.passed);
    let codes = changed.iter().any(|p| {
        p.starts_with("dotnet") && p.extension().is_some_and(|e| e == "cs")
            || metadata
                .owner(root, p)
                .is_some_and(|owner| owner.name == "ara-diag")
                && p.components().any(|part| part.as_os_str() == "codes")
    }) || metadata
        .packages
        .iter()
        .filter(|p| affected.contains(&p.name))
        .any(|p| p.name == "ara-diag" || p.dependencies.iter().any(|d| d.name == "ara-diag"))
        || changed
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .any(|p| {
                fs::read_to_string(root.join(p))
                    .is_ok_and(|text| text.contains("Code(") || text.contains("\"ARA"))
            });
    let early_coverage = ready && codes && !scope.test_packages.contains("ara-diag");
    let coverage_ready = !early_coverage
        || logs.command(
            root,
            "diagnostic-coverage",
            &["test", "-p", "ara-diag", "--test", "coverage", "--locked"].map(String::from),
        )?;
    if ready && coverage_ready && (full || !affected.is_empty()) {
        let whole_workspace = full || affected.len() == metadata.packages.len();
        let mut clippy = selection("clippy", affected, whole_workspace);
        clippy.extend([
            "--all-targets".into(),
            "--locked".into(),
            "--".into(),
            "-D".into(),
            "warnings".into(),
        ]);
        let clippy_passed = logs.command(root, "clippy", &clippy)?;
        let mut tests = selection("test", &scope.test_packages, full);
        // Clippy fetched every selected test dependency; avoid another online
        // registry transaction competing with other worktrees.
        tests.extend([
            "--locked".into(),
            "--offline".into(),
            "--no-fail-fast".into(),
        ]);
        let tests_ready = if clippy_passed && !scope.test_packages.is_empty() {
            if let Some(paths) =
                crate::binaries::prepare(root, &tests, &metadata, &scope.test_packages, &mut logs)?
            {
                logs.test_binaries = paths;
                true
            } else {
                false
            }
        } else {
            false
        };
        let tests_passed = if tests_ready {
            let execution = crate::test_runner::run(
                root,
                &logs.directory,
                &metadata,
                &scope.test_packages,
                &logs.test_binaries,
                logs.deadline,
            )?;
            logs.rows.push(Row {
                check: "tests".into(),
                passed: execution.passed,
                known_failure: false,
                timed_out: execution.timed_out,
                seconds: execution.seconds,
                detail: format!(
                    "{} harnesses, {} workers, {} MiB budget; {}",
                    execution.suites,
                    execution.jobs,
                    execution.memory_mib,
                    logs.directory.join("tests.log").display()
                ),
            });
            execution.passed
        } else {
            false
        };
        if tests_ready
            && !tests_passed
            && !allowed.is_empty()
            && !logs.rows.last().is_some_and(|row| row.timed_out)
        {
            let output = fs::read_to_string(logs.directory.join("tests.log"))?;
            let (accepted, observed) = known::classify(&output, &allowed);
            known_failures = observed;
            if accepted
                && !known_failures
                    .iter()
                    .any(|known| known.package == "ara-diag" && known.target == "coverage")
            {
                let row = logs
                    .rows
                    .last_mut()
                    .expect("tests command appended its row");
                row.passed = true;
                row.known_failure = true;
                row.detail = format!(
                    "{} explicit known failure(s); {}",
                    known_failures.len(),
                    row.detail
                );
            }
        }
        if tests_ready
            && metadata
                .packages
                .iter()
                .any(|p| scope.test_packages.contains(&p.name) && p.has_library())
        {
            let mut docs = selection("test", &scope.test_packages, full);
            docs.extend([
                "--doc".into(),
                "--locked".into(),
                "--offline".into(),
                "--no-fail-fast".into(),
            ]);
            logs.command(root, "doctests", &docs)?;
        }
    }
    let covered = scope.test_packages.contains("ara-diag")
        && logs
            .rows
            .iter()
            .any(|row| row.check == "tests" && row.passed);
    if ready && codes && !early_coverage && covered {
        logs.findings("diagnostic-coverage", Vec::new());
        logs.rows.last_mut().expect("coverage row").detail = "passed in affected tests".into();
    } else if ready && codes && !early_coverage {
        logs.command(
            root,
            "diagnostic-coverage",
            &["test", "-p", "ara-diag", "--test", "coverage", "--locked"].map(String::from),
        )?;
    }
    let after = git::TreeState::read(root)?;
    logs.findings(
        "unchanged tree",
        before
            .verify(&after)
            .err()
            .map(|e| e.to_string())
            .into_iter()
            .collect(),
    );
    let mut snapshots: Vec<_> = git::files(root)?
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with(".snap.new"))
        .map(|p| p.display().to_string())
        .collect();
    // Include ignored new snapshots too; insta can inherit ignore rules.
    snapshots.extend(
        git::paths(&git::bytes(
            root,
            [
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--",
                "crates/**/*.snap.new",
            ],
        )?)?
        .into_iter()
        .map(|p| p.display().to_string()),
    );
    snapshots.sort();
    snapshots.dedup();
    logs.findings("pending snapshots", snapshots);
    let report = Report {
        passed: logs.rows.iter().all(|r| r.passed),
        elapsed_seconds: started.elapsed().as_secs_f64(),
        compiler_cache_bypassed: no_compiler_cache,
        base,
        selection: scope,
        known_failures,
        checks: logs.rows,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Affected ({}): {}",
            report.selection.affected.len(),
            report
                .selection
                .affected
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!(
            "Tests: {} source/direct-consumer packages{}",
            report.selection.test_packages.len(),
            if full { " (--full)" } else { "" }
        );
        if report.selection.affected.len() > 8 {
            println!(
                "  ... {} more (use --json for the complete set)",
                report.selection.affected.len() - 8
            );
        }
        println!("{:<22} {:<6} {:>8}  DETAIL", "CHECK", "RESULT", "SECONDS");
        for row in &report.checks {
            println!(
                "{:<22} {:<6} {:>8.1}  {}",
                row.check,
                if row.timed_out {
                    "TIMEOUT"
                } else if row.known_failure {
                    "KNOWN"
                } else if row.passed {
                    "PASS"
                } else {
                    "FAIL"
                },
                row.seconds,
                row.detail
            );
        }
        for known in &report.known_failures {
            println!(
                "KNOWN {} --test {} / {}: {}",
                known.package, known.target, known.test, known.reason
            );
        }
        println!("Total: {:.1}s", report.elapsed_seconds);
    }
    Ok(report.passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_steps_preserve_unlimited_and_explicit_gate_deadlines() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut logs = Logs::new(&root, true).unwrap();
        let now = Instant::now();
        for deadline in [None, Some(now + Duration::from_secs(600))] {
            logs.deadline = deadline;
            for name in ["doctests", "diagnostic-coverage", "test-build", "clippy"] {
                let step = logs.cargo_step(&root, name, &[]);
                assert_eq!(step.deadline, deadline, "{name}");
            }
        }
    }
}
