//! Markdown reports: one run in detail, or several runs side by side (before/after
//! per task and agent).

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::record::{RunResult, TaskResult};

fn pct(n: usize, of: usize) -> String {
    if of == 0 {
        "-".into()
    } else {
        format!("{:.0}%", 100.0 * n as f64 / of as f64)
    }
}

fn counts(m: &BTreeMap<String, u32>) -> String {
    if m.is_empty() {
        return "-".into();
    }
    m.iter()
        .map(|(k, n)| {
            if *n > 1 {
                format!("{k}×{n}")
            } else {
                k.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn tokens(r: &TaskResult) -> String {
    match r.tokens {
        Some(t) => format!(
            "{}k/{}k",
            (t.input + t.cache_read + t.cache_write).div_ceil(1000),
            t.output.div_ceil(1000)
        ),
        None => "-".into(),
    }
}

/// One run: summary, a row per task, then what failed.
pub fn run(r: &RunResult) -> String {
    let mut s = String::new();
    let n = r.results.len();
    let cost: f64 = r.results.iter().filter_map(|t| t.cost_usd).sum();
    let wall: u64 = r.results.iter().map(|t| t.wall_ms).sum();
    let _ = writeln!(s, "# arabench {} · {} · {}\n", r.suite, r.agent, r.label);
    let _ = writeln!(
        s,
        "Started {} on engine `{}`. **{}/{n} passed ({})**, {:.0} s of agent time{}.\n",
        r.started,
        r.engine,
        r.passed(),
        pct(r.passed(), n),
        wall as f64 / 1000.0,
        if cost > 0.0 {
            format!(", ${cost:.2}")
        } else {
            String::new()
        }
    );
    s.push_str("| Task | Area | Result | Turns | Tools | Time | Tokens in/out | ARA codes hit | Raw edits |\n");
    s.push_str("|---|---|---|---|---|---|---|---|---|\n");
    for t in &r.results {
        let result = if t.pass {
            "pass".to_owned()
        } else if let Some(stop) = &t.stopped {
            format!("FAIL ({stop})")
        } else {
            "FAIL".to_owned()
        };
        let tools: u32 = t.tool_calls.values().sum();
        let raw = t.raw_edits.len() + t.external_edits;
        let _ = writeln!(
            s,
            "| `{}` | {} | {result} | {} | {tools} | {:.0}s | {} | {} | {} |",
            t.task,
            t.area,
            t.turns,
            t.wall_ms as f64 / 1000.0,
            tokens(t),
            counts(&t.codes),
            if raw == 0 {
                "-".to_owned()
            } else {
                raw.to_string()
            }
        );
    }
    let failures: Vec<&TaskResult> = r.results.iter().filter(|t| !t.pass).collect();
    if !failures.is_empty() {
        s.push_str("\n## Failures\n");
        for t in failures {
            let _ = writeln!(s, "\n### `{}`\n", t.task);
            if let Some(e) = &t.error {
                let _ = writeln!(s, "- error: {e}");
            }
            for c in t.checks.iter().filter(|c| !c.ok) {
                let _ = writeln!(s, "- {}: {}", c.what, c.detail);
            }
            if !t.raw_edits.is_empty() {
                let _ = writeln!(
                    s,
                    "- raw edits: {}",
                    t.raw_edits.join("; ").replace('\n', " ")
                );
            }
        }
    }
    s
}

/// Several runs side by side: pass/fail and turns per task, pass rate per run.
pub fn compare(runs: &[RunResult]) -> String {
    let mut s = String::from("| Task |");
    let mut rule = String::from("|---|");
    for r in runs {
        let _ = write!(s, " {} · {} |", r.agent, r.label);
        rule.push_str("---|");
    }
    s.push('\n');
    s.push_str(&rule);
    s.push('\n');
    let mut tasks: Vec<&str> = Vec::new();
    for r in runs {
        for t in &r.results {
            if !tasks.contains(&t.task.as_str()) {
                tasks.push(&t.task);
            }
        }
    }
    for task in &tasks {
        let _ = write!(s, "| `{task}` |");
        for r in runs {
            let cell = match r.result(task) {
                Some(t) if t.pass => format!("pass ({})", t.turns),
                Some(t) => format!("**fail** ({})", t.turns),
                None => "-".into(),
            };
            let _ = write!(s, " {cell} |");
        }
        s.push('\n');
    }
    s.push_str("| **Passed** |");
    for r in runs {
        let _ = write!(
            s,
            " **{}/{} ({})** |",
            r.passed(),
            r.results.len(),
            pct(r.passed(), r.results.len())
        );
    }
    s.push('\n');
    s.push_str("| Median turns |");
    for r in runs {
        let mut turns: Vec<u32> = r.results.iter().map(|t| t.turns).collect();
        turns.sort_unstable();
        let median = turns
            .get(turns.len() / 2)
            .map_or("-".into(), u32::to_string);
        let _ = write!(s, " {median} |");
    }
    s.push('\n');
    s.push_str("| Failed tool calls with ARA codes |");
    for r in runs {
        let n: u32 = r.results.iter().flat_map(|t| t.codes.values()).sum();
        let _ = write!(s, " {n} |");
    }
    s.push('\n');
    s.push_str("| Raw document edits |");
    for r in runs {
        let n: usize = r
            .results
            .iter()
            .map(|t| t.raw_edits.len() + t.external_edits)
            .sum();
        let _ = write!(s, " {n} |");
    }
    s.push('\n');
    s
}
