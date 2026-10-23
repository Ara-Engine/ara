//! What a run records: one [`TaskResult`] per task attempt, grouped in a
//! [`RunResult`] that is written as `results.json` (and read back by `compare`).

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::task::Check;
use crate::transcript::Tokens;

/// One task attempt by one agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    /// Task ID.
    pub task: String,
    /// The task's area (`prefabs`).
    pub area: String,
    /// Every check passed.
    pub pass: bool,
    /// The verifier's checks.
    pub checks: Vec<Check>,
    /// Tool-using steps.
    pub turns: u32,
    /// Calls per normalized tool (`ara_apply`, `shell`, `edit`).
    pub tool_calls: BTreeMap<String, u32>,
    /// Wall time in milliseconds.
    pub wall_ms: u64,
    /// Token usage, when the CLI reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
    /// Cost in USD, when the CLI reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// ARA codes in failed tool calls, with counts.
    pub codes: BTreeMap<String, u32>,
    /// Raw document edits seen in the transcript (paths or commands).
    pub raw_edits: Vec<String>,
    /// Transactions the engine made from raw file edits it noticed.
    pub external_edits: usize,
    /// Dry runs the agent made.
    pub dry_runs: usize,
    /// Why the agent was stopped early (`turn budget`, `time budget`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    /// A harness or CLI error (the attempt did not run properly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The agent's last message, truncated.
    #[serde(default)]
    pub final_message: String,
}

/// One suite run by one agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    /// Run label (`baseline`, `after`).
    pub label: String,
    /// Agent (`claude:sonnet-5.5`).
    pub agent: String,
    /// Suite name.
    pub suite: String,
    /// UTC start time (`2026-10-23T04:12:00Z`).
    pub started: String,
    /// Engine commit the run used (`git describe`-style, `+dirty` if modified).
    pub engine: String,
    /// One result per task, in suite order.
    pub results: Vec<TaskResult>,
}

impl RunResult {
    /// Passed tasks.
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|r| r.pass).count()
    }

    /// The result for `task`.
    pub fn result(&self, task: &str) -> Option<&TaskResult> {
        self.results.iter().find(|r| r.task == task)
    }
}

/// The current UTC time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil-from-days (Howard Hinnant's algorithm), valid for the Unix era.
    let z = days as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::now_utc;

    #[test]
    fn timestamps_are_iso_utc() {
        let t = now_utc();
        assert_eq!(t.len(), 20, "{t}");
        assert!(t.starts_with("20") && t.ends_with('Z') && t.as_bytes()[10] == b'T');
    }
}
