//! Local integration gates, performance iteration and machine hygiene commands.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod binaries;
mod build_paths;
mod cargo_wait;
mod check;
mod compile_cache;
mod d26;
mod editor;
mod git;
mod gpu;
mod gpu_guard;
mod history;
mod hygiene;
mod known;
mod manifest;
mod merge;
mod perf;
mod platform;
mod process;
mod selection;
mod test_runner;
mod workspace;
mod worktrees;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Ara developer gate and machine hygiene")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Format every workspace package in batches that fit Windows command lines.
    Fmt {
        /// Check formatting without changing source files.
        #[arg(long)]
        check: bool,
    },
    /// Rehearse attribution-only history cleanup in a newly created, isolated mirror.
    HistoryRewrite(history::Options),
    /// Find a unique rewritten fork by full cleaned message and unchanged metadata/tree.
    HistoryMatch(history::MatchOptions),
    /// Focused Cargo command using optional shared intermediates and bounded concurrency.
    Cargo {
        #[arg(last = true, required = true, num_args = 1..)]
        args: Vec<OsString>,
    },
    /// Build and measure a frozen packed scene, or control a warm perf session.
    Perf(perf::Options),
    /// Measure every D26/D27 release target; writes target/d26/<timestamp>/report.html.
    D26(d26::Options),
    /// Check changed crates and their reverse dependencies before handing off work.
    Check {
        #[arg(long, default_value = "main")]
        base: String,
        /// Emit a single JSON report (logs are written under target/ara-dev).
        #[arg(long)]
        json: bool,
        /// Whole workspace clippy and tests, for nightly integration.
        #[arg(long)]
        full: bool,
        /// Explicit JSON exceptions for individually documented main test failures.
        #[arg(long)]
        known_failures: Option<PathBuf>,
        /// Show affected packages without running build/test gates.
        #[arg(long, conflicts_with = "full")]
        plan: bool,
        /// Seconds excluding Cargo package-cache waits (default 2400; --full/0 unlimited).
        #[arg(long)]
        timeout: Option<u64>,
        /// Bypass RUSTC_WRAPPER when a shared compiler cache is contended.
        #[arg(long)]
        no_compiler_cache: bool,
    },
    /// Merge locally, validate before committing, and optionally push.
    Merge {
        branch: String,
        #[arg(long)]
        push: bool,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        known_failures: Option<PathBuf>,
        #[arg(long)]
        timeout: Option<u64>,
        #[arg(long)]
        no_compiler_cache: bool,
    },
    /// Serialize GPU benchmarks/captures across every worktree on this machine.
    Gpu {
        #[arg(long, default_value_t = 3600)]
        timeout: u64,
        /// Wait for foreign games/GPU work to clear instead of refusing the timed run.
        #[arg(long)]
        wait_for_quiet: bool,
        #[arg(last = true, required = true, num_args = 1..)]
        command: Vec<OsString>,
    },
    /// List targets; deletion is opt-in and restricted to merged, stale, clean worktrees.
    CleanWorktrees {
        #[arg(long, conflicts_with = "yes")]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value_t = 6)]
        hours: u64,
        #[arg(long, default_value = "main")]
        base: String,
    },
    /// Report free space on all drives; fail below the warning threshold.
    Disk {
        #[arg(long, default_value_t = 20)]
        warn_gib: u64,
    },
}

fn run() -> Result<bool> {
    let cli = Cli::parse();
    if let Command::HistoryRewrite(options) = cli.command {
        return history::run(options);
    }
    if let Command::HistoryMatch(options) = cli.command {
        return history::match_base(options);
    }
    if let Command::Disk { warn_gib } = cli.command {
        let mut healthy = platform::disk(warn_gib)?;
        if let Ok(root) = git::text(std::path::Path::new("."), ["rev-parse", "--show-toplevel"])
            && PathBuf::from(&root).join("Cargo.toml").is_file()
        {
            healthy &= build_paths::disk_check(std::path::Path::new(&root))?;
        }
        return Ok(healthy);
    }
    if let Command::Gpu {
        timeout,
        wait_for_quiet,
        command,
    } = cli.command
    {
        return gpu::run(timeout, wait_for_quiet, &command);
    }
    let root = PathBuf::from(git::text(
        std::path::Path::new("."),
        ["rev-parse", "--show-toplevel"],
    )?);
    match cli.command {
        Command::Fmt { check } => check::format(&root, check),
        Command::Cargo { args } => compile_cache::cargo(&root, &args),
        Command::Perf(options) => perf::run(&root, options),
        Command::D26(options) => d26::run(&root, options),
        Command::Check {
            base,
            json,
            full,
            known_failures,
            plan,
            timeout,
            no_compiler_cache,
        } => {
            let result = check::run(
                &root,
                check::Options {
                    base: &base,
                    json,
                    full,
                    known_path: known_failures.as_deref(),
                    plan,
                    timeout_seconds: timeout,
                    no_compiler_cache,
                },
            );
            if json && let Err(error) = &result {
                println!(
                    "{}",
                    serde_json::json!({"passed": false, "error": format!("{error:#}")})
                );
            }
            result
        }
        Command::Merge {
            branch,
            push,
            full,
            known_failures,
            timeout,
            no_compiler_cache,
        } => merge::run(
            &root,
            &branch,
            push,
            full,
            known_failures.as_deref(),
            timeout,
            no_compiler_cache,
        ),
        Command::CleanWorktrees {
            yes, hours, base, ..
        } => worktrees::run(&root, yes, hours, &base),
        Command::Disk { .. }
        | Command::Gpu { .. }
        | Command::HistoryRewrite(_)
        | Command::HistoryMatch(_) => {
            unreachable!("handled above")
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("ara-dev: {error:#}");
            if gpu_guard::is_invalid(&error) {
                ExitCode::from(3)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
