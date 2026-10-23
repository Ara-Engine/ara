//! `ara why <subject|code|question>` and the diagnostics flags of `ara logs`: thin
//! views over the `why` and `logs` protocol methods (through the daemon when it
//! runs, else an in-process host that also reads `.ara/diagnostics.jsonl`).

use clap::Args;
use serde_json::{Value, json};

use super::protocol::{Call, pretty};

/// `ara why`: explain a problem with ranked causes, evidence and runnable fixes.
#[derive(Args)]
pub struct WhyArgs {
    /// What to explain: `name:Crate not visible`, `ARA1302`, `@meshes/crate.glb`,
    /// `did reload fall back to L3`, `is the frame slow`. Quotes are optional.
    pub question: Vec<String>,
    /// Document for entity names (default: the one holding the entity, else the
    /// main scene).
    #[arg(long)]
    pub doc: Option<String>,
    /// List what `ara why` can explain.
    #[arg(long)]
    pub list: bool,
}

impl WhyArgs {
    /// The `why` call.
    pub fn call(self) -> Call {
        let question = self.question.join(" ");
        Call {
            method: "why",
            params: drop_nulls(json!({
                "question": (!question.is_empty()).then_some(question),
                "doc": self.doc,
                "list": self.list.then_some(true),
            })),
            summary: pretty,
        }
    }
}

/// `ara logs`: diagnostics grouped by code (or one by one with `--all`).
#[derive(Args)]
pub struct LogsArgs {
    /// Only entries new or repeated after this cursor.
    #[arg(long)]
    pub since: Option<u64>,
    /// Only these codes (`ARA1101`, `ARA-EXP-AND-007`). Repeatable.
    #[arg(long = "code")]
    pub codes: Vec<String>,
    /// Minimum severity: error, warning, info.
    #[arg(long)]
    pub severity: Option<String>,
    /// Only entries about this entity.
    #[arg(long)]
    pub entity: Option<String>,
    /// Only entries about this subject (`asset:@meshes/crate.glb`, `module:Gameplay`,
    /// `system:physics.step`). Repeatable.
    #[arg(long = "subject")]
    pub subjects: Vec<String>,
    /// Only entries caused by `engine`, `human`, `agent` or `agent:<name>`.
    #[arg(long)]
    pub origin: Option<String>,
    /// Only entries whose message contains this text.
    #[arg(long)]
    pub grep: Option<String>,
    /// List entries one by one instead of grouping them by code.
    #[arg(long)]
    pub all: bool,
    /// Approximate size of the answer in characters (default 2000).
    #[arg(long)]
    pub budget: Option<u32>,
    /// Maximum entries.
    #[arg(long)]
    pub limit: Option<u32>,
}

impl LogsArgs {
    /// The `logs` call.
    pub fn call(self) -> Call {
        Call {
            method: "logs",
            params: drop_nulls(json!({
                "since": self.since,
                "severity": self.severity,
                "entity": self.entity,
                "origin": self.origin,
                "text": self.grep,
                "budget": self.budget,
                "limit": self.limit,
                "group": self.all.then_some(false),
                "codes": (!self.codes.is_empty()).then_some(self.codes),
                "subjects": (!self.subjects.is_empty()).then_some(self.subjects),
            })),
            summary: pretty,
        }
    }
}

/// Drops `null` fields so optional params stay absent.
fn drop_nulls(v: Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(o.into_iter().filter(|(_, v)| !v.is_null()).collect()),
        other => other,
    }
}
