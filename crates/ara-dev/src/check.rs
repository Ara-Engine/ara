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
}

impl Logs {
    pub(crate) fn new(root: &Path, no_compiler_cache: bool) -> Result<Self> {
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
        })
    }

    pub(crate) fn command(&mut self, root: &Path, name: &str, args: &[String]) -> Result<bool> {
        let log = self.directory.join(format!("{name}.log"));
        let file = File::create(&log)?;
        self.execute(root, name, args, &file, &file, &log)
    }

    fn capture(&mut self, root: &Path, name: &str, args: &[String]) -> Result<Vec<u8>> {
        let output = self.directory.join(format!("{name}.json"));
        let log = self.directory.join(format!("{name}.log"));
        if !self.execute(
            root,
            name,
            args,
            &File::create(&output)?,
            &File::create(&log)?,
            &log,
        )? {
            bail!(
                "{}; {}",
                self.rows.last().context("command result")?.detail,
                fs::read_to_string(log).unwrap_or_default()
            );
        }
        Ok(fs::read(output)?)
    }

    fn execute(
        &mut self,
        root: &Path,
        name: &str,
        args: &[String],
        stdout: &File,
        stderr: &File,
        log: &Path,
    ) -> Result<bool> {
        eprintln!("{name}: cargo {} (log: {})", args.join(" "), log.display());
        let start = Instant::now();
        let mut command = Command::new("cargo");
        command
            .current_dir(root)
            .args(args)
            .env("CARGO_BUILD_JOBS", "2")
            .env("CARGO_TERM_COLOR", "never")
            .stdout(Stdio::from(stdout.try_clone()?))
            .stderr(Stdio::from(stderr.try_clone()?));
        if self.no_compiler_cache {
            // An empty wrapper overrides Cargo config; clippy's workspace wrapper stays active.
            command.env("RUSTC_WRAPPER", "");
        }
        let status = process::run(&mut command, self.deadline);
        let passed = status.as_ref().is_ok_and(process::Outcome::success);
        let timed_out = matches!(status, Ok(process::Outcome::TimedOut));
        let detail = match status {
            Ok(process::Outcome::Exited(s)) => format!("{s}; {}", log.display()),
            Ok(process::Outcome::TimedOut) => format!(
                "timed out at the check deadline (including Cargo cache waits); {}",
                log.display()
            ),
            Err(e) => format!("{e}; {}", log.display()),
        };
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
    fn formatting(&mut self, root: &Path, metadata: &Metadata) -> Result<bool> {
        let start = Instant::now();
        let first = self.rows.len();
        for (index, packages) in metadata.packages.chunks(4).enumerate() {
            let mut args = vec!["fmt".into()];
            for package in packages {
                args.extend(["-p".into(), package.name.clone()]);
            }
            args.extend(["--".into(), "--check".into()]);
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
                    metadata.packages.len(),
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
    let metadata: Metadata = serde_json::from_slice(
        &logs.capture(
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
    )?;
    eprintln!("Cargo target: {}", metadata.target_directory.display());
    let mut scope = selection::compute(root, &base, &changed, &metadata)?;
    if full {
        scope.affected = metadata.packages.iter().map(|p| p.name.clone()).collect();
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
            for reason in &scope.reasons {
                println!("  {reason}");
            }
        }
        return Ok(true);
    }
    let affected = &scope.affected;
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
    let fmt = logs.formatting(root, &metadata)?;
    let ready = fmt && logs.rows.iter().all(|row| row.passed);
    let codes = changed.iter().any(|p| {
        p.starts_with("dotnet") && p.extension().is_some_and(|e| e == "cs")
            || p.starts_with("crates/ara-diag/codes")
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
    let early_coverage = ready && codes && !affected.contains("ara-diag");
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
        let mut tests = selection("test", affected, whole_workspace);
        tests.extend(["--locked".into(), "--no-fail-fast".into()]);
        if clippy_passed
            && !logs.command(root, "tests", &tests)?
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
    }
    let covered = affected.contains("ara-diag")
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
