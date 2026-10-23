//! Commands that run Engine Protocol methods in a session of their own: `ara do`,
//! `query`, `inspect`, `history`, `undo`, `redo`, `review`, `accept`, `revert`,
//! `logs`. They use the daemon when it runs, else an in-process host.

use std::path::Path;

use ara_diag::{Diagnostic, codes};
use serde_json::{Map, Value, json};

use crate::engine::{Engine, print, print_diags};

/// A protocol call: method, params and how to summarize a result without `text`.
pub struct Call {
    /// Method name.
    pub method: &'static str,
    /// Params (without `session`).
    pub params: Value,
    /// One-line summary for results without a `text` field.
    pub summary: fn(&Value) -> String,
}

/// Runs one call in a fresh session attributed to `author`.
pub fn run(root: &Path, author: &str, json: bool, call: Call) -> Result<bool, String> {
    let mut engine = match Engine::connect(root, author, &format!("ara {}", call.method)) {
        Ok(e) => e,
        Err(diags) => {
            print_diags(&diags, json);
            return Ok(false);
        }
    };
    Ok(print(
        engine.call(call.method, call.params),
        json,
        call.summary,
    ))
}

/// `--args <json>` plus `--param k=v` pairs (`a.b=1` nests; values parse as JSON,
/// else they are strings).
pub fn args(base: Option<&str>, params: &[String]) -> Result<Value, Vec<Diagnostic>> {
    let bad = |msg: String| vec![Diagnostic::error(codes::INVALID_PARAMS, msg)];
    let mut obj: Map<String, Value> = match base {
        None => Map::new(),
        Some(text) => match serde_json::from_str(text) {
            Ok(Value::Object(o)) => o,
            Ok(_) => return Err(bad("`--args` must be a JSON object".into())),
            Err(e) => return Err(bad(format!("`--args` is not JSON: {e}"))),
        },
    };
    for p in params {
        let Some((key, raw)) = p.split_once('=') else {
            return Err(bad(format!("`--param {p}`: expected `key=value`")));
        };
        let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
        let mut parts: Vec<&str> = key.split('.').collect();
        let last = parts.pop().unwrap_or(key);
        let mut cur = &mut obj;
        for part in parts {
            let next = cur
                .entry(part.to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if !next.is_object() {
                *next = Value::Object(Map::new());
            }
            let Value::Object(inner) = next else {
                return Err(bad(format!("`--param {p}`: `{part}` is not an object")));
            };
            cur = inner;
        }
        cur.insert(last.to_owned(), value);
    }
    Ok(Value::Object(obj))
}

/// Summary of a `cmd.apply` result.
pub fn applied(v: &Value) -> String {
    let list = |k: &str| {
        v["diff"][k]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let mut parts = Vec::new();
    for (k, verb) in [
        ("created", "created"),
        ("modified", "modified"),
        ("deleted", "deleted"),
    ] {
        let l = list(k);
        if !l.is_empty() {
            parts.push(format!("{verb} {l}"));
        }
    }
    let changes = if parts.is_empty() {
        "no changes".to_owned()
    } else {
        parts.join(" · ")
    };
    let mut out = if v["dryRun"] == json!(true) {
        format!("dry run (nothing applied): {changes}")
    } else {
        match v["txn"].as_str() {
            Some(t) => format!("applied {t} (revision {}) · {changes}", v["revision"]),
            None => "nothing changed".to_owned(),
        }
    };
    if let Some(ids) = v["ids"].as_object().filter(|m| !m.is_empty()) {
        let pairs: Vec<String> = ids
            .iter()
            .map(|(k, id)| format!("{k} = {}", id.as_str().unwrap_or_default()))
            .collect();
        out.push_str(&format!("\nsymbols: {}", pairs.join(", ")));
    }
    out
}

/// Summary of `history.undo` / `history.redo`.
pub fn undone(v: &Value) -> String {
    format!(
        "{} reverting {} (revision {})",
        v["txn"].as_str().unwrap_or_default(),
        v["reverts"].as_str().unwrap_or("-"),
        v["revision"]
    )
}

/// Summary of `review.accept` / `review.revert`.
pub fn reviewed(v: &Value) -> String {
    let items = v["accepted"]
        .as_array()
        .or_else(|| v["reverted"].as_array())
        .cloned()
        .unwrap_or_default();
    let verb = if v.get("accepted").is_some() {
        "accepted"
    } else {
        "reverted"
    };
    items
        .iter()
        .map(|i| {
            let ents: Vec<&str> = i["entities"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            format!(
                "{verb} {}: {}",
                i["txn"].as_str().unwrap_or_default(),
                ents.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A pretty JSON fallback summary.
pub fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}
