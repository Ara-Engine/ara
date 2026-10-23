//! Local integration and machine hygiene commands; no daemon or engine startup.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod check;
mod git;
mod gpu;
mod hygiene;
mod merge;
mod platform;
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
    },
    /// Merge locally, validate before committing, and optionally push.
    Merge {
        branch: String,
        #[arg(long)]
        push: bool,
        #[arg(long)]
        full: bool,
    },
    /// Serialize GPU benchmarks/captures across every worktree on this machine.
    Gpu {
        #[arg(long, default_value_t = 3600)]
        timeout: u64,
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
    if let Command::Disk { warn_gib } = cli.command {
        return platform::disk(warn_gib);
    }
    if let Command::Gpu { timeout, command } = cli.command {
        return gpu::run(timeout, &command);
    }
    let root = PathBuf::from(git::text(
        std::path::Path::new("."),
        ["rev-parse", "--show-toplevel"],
    )?);
    match cli.command {
        Command::Check { base, json, full } => check::run(&root, &base, json, full),
        Command::Merge { branch, push, full } => merge::run(&root, &branch, push, full),
        Command::CleanWorktrees {
            yes, hours, base, ..
        } => worktrees::run(&root, yes, hours, &base),
        Command::Disk { .. } | Command::Gpu { .. } => unreachable!("handled above"),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("ara-dev: {error:#}");
            ExitCode::FAILURE
        }
    }
}
