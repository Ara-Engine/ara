//! Runs tasks: each attempt gets a fresh `ara init` project in its own temp dir,
//! the agent CLI runs headless with its budgets enforced live, then the verifier
//! reads the finished project. Also the self-test, which runs every reference
//! solution instead of an agent.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::agent::AgentSpec;
use crate::fixture::{self, Init};
use crate::record::{RunResult, TaskResult, now_utc};
use crate::solve::{AUTHOR, Solver};
use crate::task::{Check, Checks, Task};
use crate::transcript::{ToolCall, Transcript};
use crate::world::World;

/// Longest string kept per field in saved failure transcripts.
const TRIM: usize = 1500;

/// How a suite run is set up.
pub struct RunOptions {
    /// The `ara` binary agents and `ara init` use.
    pub ara: PathBuf,
    /// Tasks run at once.
    pub jobs: usize,
    /// Keep each attempt's temp project.
    pub keep: bool,
    /// Where `transcripts/` of failed attempts go.
    pub out: PathBuf,
    /// Run label.
    pub label: String,
    /// Suite name, recorded.
    pub suite: String,
}

/// Runs `tasks` with `agent`, `opts.jobs` at a time; progress goes to stderr.
pub fn run(tasks: &[Task], agent: &AgentSpec, opts: &RunOptions) -> RunResult {
    let started = now_utc();
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, TaskResult)>> = Mutex::new(Vec::new());
    thread::scope(|s| {
        for _ in 0..opts.jobs.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(task) = tasks.get(i) else { break };
                    let r = attempt(task, agent, opts);
                    eprintln!(
                        "[{agent}] {} {} · {} turns · {:.0}s{}",
                        if r.pass { "PASS" } else { "FAIL" },
                        task.id,
                        r.turns,
                        r.wall_ms as f64 / 1000.0,
                        r.error
                            .as_deref()
                            .map(|e| format!(" · {e}"))
                            .unwrap_or_default()
                    );
                    done.lock().unwrap_or_else(|p| p.into_inner()).push((i, r));
                }
            });
        }
    });
    let mut results = done.into_inner().unwrap_or_else(|p| p.into_inner());
    results.sort_by_key(|(i, _)| *i);
    RunResult {
        label: opts.label.clone(),
        agent: agent.to_string(),
        suite: opts.suite.clone(),
        started,
        engine: engine_version(),
        results: results.into_iter().map(|(_, r)| r).collect(),
    }
}

/// The engine commit, `+dirty` when the work tree has changes.
fn engine_version() -> String {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let git = |args: &[&str]| crate::solve::git(&repo, args).map(|s| s.trim().to_owned());
    let head = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|_| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--", "crates"]).is_ok_and(|s| !s.is_empty());
    if dirty { format!("{head}+dirty") } else { head }
}

fn area_name(task: &Task) -> String {
    serde_json::to_value(task.area)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn failed(task: &Task, error: String, wall_ms: u64) -> TaskResult {
    TaskResult {
        task: task.id.into(),
        area: area_name(task),
        pass: false,
        checks: Vec::new(),
        turns: 0,
        tool_calls: Default::default(),
        wall_ms,
        tokens: None,
        cost_usd: None,
        codes: Default::default(),
        raw_edits: Vec::new(),
        external_edits: 0,
        dry_runs: 0,
        stopped: None,
        error: Some(error),
        final_message: String::new(),
    }
}

/// One attempt at `task` by `agent`.
fn attempt(task: &Task, agent: &AgentSpec, opts: &RunOptions) -> TaskResult {
    let start = Instant::now();
    let ms = |s: Instant| s.elapsed().as_millis() as u64;
    let dir = match fixture::scratch(&format!("{}-{}", agent.slug(), task.id)) {
        Ok(d) => d,
        Err(e) => return failed(task, e, 0),
    };
    let root = dir.join("proj");
    if let Err(e) = fixture::create(&root, task, Init::Binary(&opts.ara)) {
        return failed(task, format!("setup: {e}"), ms(start));
    }
    let raw = dir.join("transcript.jsonl");
    let (t, stopped, exit) = match drive(task, agent, opts, &root, &dir, &raw) {
        Ok(v) => v,
        Err(e) => return failed(task, e, ms(start)),
    };
    let wall_ms = ms(start);
    let mut checks = Checks::default();
    let verified = World::open(&root, t.calls.clone()).map(|w| {
        (task.verify)(&w, &mut checks);
        w.external_edits()
    });
    let (external_edits, verify_error) = match verified {
        Ok(n) => (n, None),
        Err(e) => (0, Some(format!("verify: {e}"))),
    };
    let error = verify_error
        .or_else(|| t.cli_error.clone().map(|e| format!("cli: {e}")))
        .or_else(|| {
            (!t.done && stopped.is_none()).then(|| format!("cli exited ({exit}) without finishing"))
        });
    let pass = checks.passed() && stopped.is_none() && error.is_none();
    let result = TaskResult {
        task: task.id.into(),
        area: area_name(task),
        pass,
        checks: checks.list,
        turns: t.turns,
        tool_calls: t.tool_counts(),
        wall_ms,
        tokens: t.tokens,
        cost_usd: t.cost_usd,
        codes: t.code_counts(),
        raw_edits: t.raw_edits(),
        external_edits,
        dry_runs: t.calls.iter().filter(|c| c.is_dry_run()).count(),
        stopped,
        error,
        final_message: t.final_message.chars().take(600).collect(),
    };
    if !result.pass {
        let _ = save_trimmed(
            &raw,
            &opts
                .out
                .join("transcripts")
                .join(format!("{}.jsonl", task.id)),
        );
    }
    if !opts.keep {
        let _ = fs::remove_dir_all(&dir);
    }
    result
}

/// Runs the agent CLI until it exits or a budget runs out. Returns the transcript,
/// why it was stopped (if it was) and its exit status.
fn drive(
    task: &Task,
    agent: &AgentSpec,
    opts: &RunOptions,
    root: &Path,
    dir: &Path,
    raw: &Path,
) -> Result<(Transcript, Option<String>, String), String> {
    let ara_dir = opts.ara.parent().ok_or("the ara binary has no directory")?;
    let stderr = File::create(dir.join("stderr.txt")).map_err(|e| e.to_string())?;
    let mut child = agent
        .command(root, ara_dir)?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("cannot start {agent}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(task.prompt.as_bytes())
            .map_err(|e| format!("cannot send the prompt: {e}"))?;
    }
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut file = File::create(raw).map_err(|e| e.to_string())?;
    let mut t = Transcript::default();
    let start = Instant::now();
    let limit = Duration::from_secs(u64::from(task.budget.seconds));
    let mut stopped = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let _ = writeln!(file, "{line}");
                t.push(agent.format(), &line);
                if t.turns > task.budget.turns && stopped.is_none() {
                    stopped = Some(format!("turn budget ({})", task.budget.turns));
                    kill_tree(&mut child);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if start.elapsed() > limit && stopped.is_none() {
            stopped = Some(format!("time budget ({}s)", task.budget.seconds));
            kill_tree(&mut child);
        }
    }
    let exit = child
        .wait()
        .map_or_else(|e| e.to_string(), |s| s.to_string());
    Ok((t, stopped, exit))
}

/// Stops the agent and everything it started (its `ara mcp` server included).
fn kill_tree(child: &mut Child) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

fn trim_value(v: &mut Value) {
    match v {
        Value::String(s) if s.len() > TRIM => {
            let mut end = TRIM;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            *s = format!("{}… [{} bytes]", &s[..end], s.len());
        }
        Value::Array(items) => items.iter_mut().for_each(trim_value),
        Value::Object(o) => o.values_mut().for_each(trim_value),
        _ => {}
    }
}

/// Copies a transcript with long strings cut, for `results/<run>/transcripts/`.
fn save_trimmed(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(d) = to.parent() {
        fs::create_dir_all(d)?;
    }
    let text = fs::read_to_string(from)?;
    let mut out = String::new();
    for line in text.lines() {
        match serde_json::from_str::<Value>(line) {
            Ok(mut v) => {
                trim_value(&mut v);
                out.push_str(&v.to_string());
            }
            Err(_) => out.push_str(&line.chars().take(TRIM).collect::<String>()),
        }
        out.push('\n');
    }
    fs::write(to, out)
}

/// The self-test result of one task.
#[derive(Debug)]
pub struct SelfTest {
    /// Task ID.
    pub task: &'static str,
    /// The checks on the untouched fixture (some must fail).
    pub before: Vec<Check>,
    /// The checks after the reference solution (all must pass).
    pub after: Vec<Check>,
    /// Setup or solution errors.
    pub error: Option<String>,
}

impl SelfTest {
    /// The verifier rejected the fixture and accepted the solution.
    pub fn ok(&self) -> bool {
        self.error.is_none()
            && self.before.iter().any(|c| !c.ok)
            && !self.after.is_empty()
            && self.after.iter().all(|c| c.ok)
    }
}

fn verify(task: &Task, root: &Path, calls: Vec<ToolCall>) -> Result<Vec<Check>, String> {
    let world = World::open(root, calls)?;
    let mut checks = Checks::default();
    (task.verify)(&world, &mut checks);
    Ok(checks.list)
}

/// Runs `task`'s reference solution on a fresh project and verifies before and after.
pub fn self_test(task: &Task, init: Init<'_>) -> SelfTest {
    let mut st = SelfTest {
        task: task.id,
        before: Vec::new(),
        after: Vec::new(),
        error: None,
    };
    let run = |st: &mut SelfTest| -> Result<(), String> {
        let dir = fixture::scratch(&format!("selftest-{}", task.id))?;
        let root = dir.join("proj");
        fixture::create(&root, task, init)?;
        st.before = verify(task, &root, Vec::new())?;
        let mut solver = Solver::open(&root, AUTHOR)?;
        (task.solve)(&mut solver)?;
        let calls = solver.close();
        st.after = verify(task, &root, calls)?;
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    };
    if let Err(e) = run(&mut st) {
        st.error = Some(e);
    }
    st
}
