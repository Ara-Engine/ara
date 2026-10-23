//! `arabench`: run the agent-ergonomics suite locally (no CI, D9).
//!
//! - `arabench run --agent claude:sonnet-5.5 [--agent codex:gpt-6-luna] --suite v0`
//!   writes `results/<label>-<agent>/results.json` and `report.md` (plus trimmed
//!   transcripts of failures).
//! - `arabench selftest`: every reference solution against its verifier, no agents.
//! - `arabench compare <results.json>..`: before/after table.
//! - `arabench list`: the tasks.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arabench::agent::AgentSpec;
use arabench::fixture::Init;
use arabench::record::RunResult;
use arabench::report;
use arabench::runner::{self, RunOptions};
use arabench::suite::{self, SUITES};
use arabench::task::Task;
use clap::{Parser, Subcommand};

/// Ara's agent-ergonomics benchmark.
#[derive(Parser)]
#[command(name = "arabench", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a suite with one or more agents.
    Run {
        /// `claude:<model>[@effort]` or `codex:<model>[@effort]`. Repeatable.
        #[arg(long = "agent", required = true)]
        agents: Vec<String>,
        /// Suite name.
        #[arg(long, default_value = "v0")]
        suite: String,
        /// Only these task IDs. Repeatable.
        #[arg(long = "task")]
        tasks: Vec<String>,
        /// Tasks run at once per agent.
        #[arg(long, default_value_t = 2)]
        jobs: usize,
        /// Run label, used in the output directory name.
        #[arg(long, default_value = "run")]
        label: String,
        /// Results directory (default: this crate's `results/`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// The `ara` binary (default: build it and use the one next to arabench).
        #[arg(long)]
        ara: Option<PathBuf>,
        /// Keep each attempt's temp project.
        #[arg(long)]
        keep: bool,
    },
    /// Run every reference solution against its verifier (no agents).
    Selftest {
        /// Suite name.
        #[arg(long, default_value = "v0")]
        suite: String,
        /// Only these task IDs. Repeatable.
        #[arg(long = "task")]
        tasks: Vec<String>,
        /// Create projects with the real `ara init` (builds `ara`) instead of a
        /// minimal stand-in.
        #[arg(long)]
        binary: bool,
    },
    /// Compare runs: a table of pass/fail and turns per task.
    Compare {
        /// `results.json` files, in column order.
        #[arg(required = true)]
        runs: Vec<PathBuf>,
        /// Write the table here instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// List a suite's tasks.
    List {
        /// Suite name.
        #[arg(long, default_value = "v0")]
        suite: String,
    },
}

fn tasks_of(suite: &str, only: &[String]) -> Result<Vec<Task>, String> {
    let all = suite::suite(suite)
        .ok_or_else(|| format!("no suite `{suite}`; suites: {}", SUITES.join(", ")))?;
    if let Some(bad) = only
        .iter()
        .find(|id| !all.iter().any(|t| t.id == id.as_str()))
    {
        return Err(format!("no task `{bad}` in suite `{suite}`"));
    }
    Ok(all
        .into_iter()
        .filter(|t| only.is_empty() || only.iter().any(|id| id == t.id))
        .collect())
}

/// Builds `ara` (same profile as this binary) and returns its path.
fn build_ara() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("arabench has no directory")?;
    let release = dir.file_name().is_some_and(|n| n == "release");
    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo
        .args(["build", "-q", "-p", "ara"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")));
    if release {
        cargo.arg("--release");
    }
    let status = cargo
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err("`cargo build -p ara` failed".into());
    }
    let ara = dir.join(format!("ara{}", std::env::consts::EXE_SUFFIX));
    ara.is_file()
        .then_some(ara)
        .ok_or_else(|| format!("no ara binary next to `{}`", exe.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    fs::write(path, text).map_err(|e| format!("cannot write `{}`: {e}", path.display()))
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("arabench: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(cmd: Cmd) -> Result<bool, String> {
    match cmd {
        Cmd::Run {
            agents,
            suite,
            tasks,
            jobs,
            label,
            out,
            ara,
            keep,
        } => {
            let tasks = tasks_of(&suite, &tasks)?;
            let agents: Vec<AgentSpec> =
                agents.iter().map(|a| a.parse()).collect::<Result<_, _>>()?;
            let ara = match ara {
                Some(p) => p,
                None => build_ara()?,
            };
            let ara = fs::canonicalize(&ara).map_err(|e| format!("`{}`: {e}", ara.display()))?;
            let base = out.unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("results"));
            let mut all_passed = true;
            // Agents run one after the other; tasks within an agent run `jobs` at once.
            for agent in &agents {
                let dir = base.join(format!("{label}-{}", agent.slug()));
                let _ = fs::remove_dir_all(dir.join("transcripts"));
                let opts = RunOptions {
                    ara: ara.clone(),
                    jobs,
                    keep,
                    out: dir.clone(),
                    label: label.clone(),
                    suite: suite.clone(),
                };
                let result = runner::run(&tasks, agent, &opts);
                let json = serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?;
                write(&dir.join("results.json"), &(json + "\n"))?;
                write(&dir.join("report.md"), &report::run(&result))?;
                eprintln!(
                    "{agent}: {}/{} passed · {}",
                    result.passed(),
                    result.results.len(),
                    dir.display()
                );
                all_passed &= result.passed() == result.results.len();
            }
            Ok(all_passed)
        }
        Cmd::Selftest {
            suite,
            tasks,
            binary,
        } => {
            let tasks = tasks_of(&suite, &tasks)?;
            let ara = if binary { Some(build_ara()?) } else { None };
            let init = ara.as_deref().map_or(Init::Minimal, Init::Binary);
            let mut ok = true;
            for task in &tasks {
                let st = runner::self_test(task, init);
                ok &= st.ok();
                println!("{} {}", if st.ok() { "ok  " } else { "FAIL" }, task.id);
                if let Some(e) = &st.error {
                    println!("     error: {e}");
                }
                if !st.before.iter().any(|c| !c.ok) && st.error.is_none() {
                    println!("     the untouched fixture already passes");
                }
                for c in st.after.iter().filter(|c| !c.ok) {
                    println!("     after solution: {}: {}", c.what, c.detail);
                }
            }
            Ok(ok)
        }
        Cmd::Compare { runs, out } => {
            let runs: Vec<RunResult> = runs
                .iter()
                .map(|p| {
                    let text =
                        fs::read_to_string(p).map_err(|e| format!("`{}`: {e}", p.display()))?;
                    serde_json::from_str(&text).map_err(|e| format!("`{}`: {e}", p.display()))
                })
                .collect::<Result<_, String>>()?;
            let table = report::compare(&runs);
            match out {
                Some(p) => write(&p, &table)?,
                None => print!("{table}"),
            }
            Ok(true)
        }
        Cmd::List { suite } => {
            for t in tasks_of(&suite, &[])? {
                println!("{:<30} {:<12} {}", t.id, format!("{:?}", t.area), t.prompt);
            }
            Ok(true)
        }
    }
}
