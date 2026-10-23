//! Playtest specs for `test.run`: an ordered list of steps run against one player,
//! with tick-level assertions read from the live world.
//!
//! ```json
//! [{"do": "step", "ticks": 120, "inputs": [{"action": "Move", "value": [0, 1], "holdTicks": 120}]},
//!  {"do": "expect", "path": "e_fox.CharacterMotion.grounded", "compare": "eq", "value": true},
//!  {"do": "until", "path": "e_bot1.Wanderer.goals", "compare": "ge", "value": 1, "maxTicks": 600}]
//! ```
use ara_diag::Diagnostic;
use ara_play::{Compare, method};
use serde::Deserialize;
use serde_json::{Value, json};

/// One step.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "do", rename_all = "camelCase", deny_unknown_fields)]
pub enum Step {
    /// Queue injections for the next tick.
    Input {
        /// `{action, value, holdTicks, afterTicks, player}`.
        inputs: Vec<Value>,
    },
    /// Run exact ticks.
    Step {
        /// Tick count.
        ticks: u32,
        /// Injections relative to the first tick.
        #[serde(default)]
        inputs: Vec<Value>,
    },
    /// Tick until a comparison holds; failing to reach it fails the test.
    #[serde(rename_all = "camelCase")]
    Until {
        /// `entity.Component.field`.
        path: String,
        /// Comparison.
        compare: Compare,
        /// Right-hand value.
        value: Value,
        /// Tick budget.
        max_ticks: u32,
    },
    /// Assert a value at the current tick.
    Expect {
        /// `entity.Component.field`.
        path: String,
        /// Comparison.
        compare: Compare,
        /// Right-hand value.
        value: Value,
    },
    /// Live edit at the next tick.
    Patch {
        /// `{entity, path, value}` edits.
        sets: Vec<Value>,
    },
    /// Sample paths during later steps.
    Watch {
        /// Paths.
        paths: Vec<String>,
        /// Interval in ticks.
        #[serde(default = "one")]
        every: u32,
    },
    /// Labelled capture to a PNG.
    Capture {
        /// Output path.
        path: String,
    },
    /// Record the state hash.
    Hash,
}
fn one() -> u32 {
    1
}

/// Parses spec steps with located errors.
pub fn parse(steps: &[Value]) -> Result<Vec<Step>, String> {
    steps
        .iter()
        .enumerate()
        .map(|(i, s)| serde_json::from_value(s.clone()).map_err(|e| format!("steps[{i}]: {e}")))
        .collect()
}

/// Runs the steps through `call(method, params)`; returns the report (pass/fail per
/// assertion, samples, final hash). Engine errors abort the run.
pub fn run(
    steps: &[Step],
    mut call: impl FnMut(&str, Value) -> Result<Value, Vec<Diagnostic>>,
) -> Result<Value, Vec<Diagnostic>> {
    let mut results = Vec::new();
    let mut passed = true;
    let mut samples = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let entry = match step {
            Step::Input { inputs } => {
                let r = call(method::INPUT, json!({"inputs": inputs}))?;
                json!({"step": index, "do": "input", "tick": r["tick"]})
            }
            Step::Step { ticks, inputs } => {
                let r = call(method::STEP, json!({"ticks": ticks, "inputs": inputs}))?;
                samples.extend(r["samples"].as_array().cloned().unwrap_or_default());
                json!({"step": index, "do": "step", "tick": r["tick"], "hash": r["hash"]})
            }
            Step::Until {
                path,
                compare,
                value,
                max_ticks,
            } => {
                let r = call(
                    method::UNTIL,
                    json!({"path": path, "compare": compare, "value": value, "maxTicks": max_ticks}),
                )?;
                let ok = r["reached"] == json!(true);
                passed &= ok;
                samples.extend(r["samples"].as_array().cloned().unwrap_or_default());
                json!({"step": index, "do": "until", "path": path, "ok": ok, "tick": r["tick"], "ticks": r["ticks"]})
            }
            Step::Expect {
                path,
                compare,
                value,
            } => {
                let r = call(method::READ, json!({"paths": [path]}))?;
                let actual = r["values"][path].clone();
                let ok = !actual.is_null() && compare.test(&actual, value);
                passed &= ok;
                json!({"step": index, "do": "expect", "path": path, "compare": compare, "expected": value, "actual": actual, "ok": ok, "tick": r["tick"]})
            }
            Step::Patch { sets } => {
                let r = call(method::PATCH, json!({"sets": sets}))?;
                json!({"step": index, "do": "patch", "appliedAtTick": r["appliedAtTick"], "readBack": r["readBack"]})
            }
            Step::Watch { paths, every } => {
                call(method::WATCH, json!({"paths": paths, "every": every}))?;
                json!({"step": index, "do": "watch"})
            }
            Step::Capture { path } => {
                let r = call(method::CAPTURE, json!({"path": path}))?;
                json!({"step": index, "do": "capture", "image": r["image"], "marks": r["marks"].as_array().map_or(0, Vec::len)})
            }
            Step::Hash => {
                let r = call(method::CAPTURE, json!({}))?;
                json!({"step": index, "do": "hash", "tick": r["tick"], "hash": r["hash"]})
            }
        };
        results.push(entry);
    }
    let last = call(method::CAPTURE, json!({}))?;
    Ok(
        json!({"passed": passed, "tick": last["tick"], "hash": last["hash"], "results": results, "samples": samples}),
    )
}

/// One line naming the failed assertions.
pub fn summary(report: &Value) -> String {
    let mut failed = Vec::new();
    for run in report["runs"].as_array().into_iter().flatten() {
        for r in run["results"].as_array().into_iter().flatten() {
            if r["ok"] == json!(false) {
                failed.push(format!(
                    "{} {} (actual {})",
                    r["do"], r["path"], r["actual"]
                ));
            }
        }
    }
    if report["deterministic"] == json!(false) {
        failed.push("final state hashes differ between runs".into());
    }
    failed.join("; ")
}
