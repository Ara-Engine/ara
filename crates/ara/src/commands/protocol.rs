//! Commands that run Engine Protocol methods in a session of their own: `ara do`,
//! `query`, `inspect`, `history`, `undo`, `redo`, `review`, `accept`, `revert`,
//! `logs`. They use the daemon when it runs, else an in-process host.

use std::fs;
use std::io::Read;
use std::path::Path;

use ara_diag::{Diagnostic, codes};
use serde_json::{Map, Value, json};

use crate::engine::{Engine, print, print_diags};
use crate::relaxed;

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

/// The text `--args` names: itself, `@file`, or `-` (stdin).
fn args_text(base: &str) -> Result<String, String> {
    if base == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| format!("cannot read `--args` from stdin: {e}"))?;
        return Ok(text);
    }
    match base.strip_prefix('@') {
        Some(file) => {
            fs::read_to_string(file).map_err(|e| format!("cannot read `--args {base}`: {e}"))
        }
        None => Ok(base.to_owned()),
    }
}

/// `--args` plus `--param k=v` pairs (`a.b=1` nests). Both read strict JSON, else
/// [`relaxed`] JSON (unquoted keys and words, for shells that strip quotes); a
/// `--param` value that is neither is text.
pub fn args(base: Option<&str>, params: &[String]) -> Result<Value, Vec<Diagnostic>> {
    let bad = |msg: String| vec![Diagnostic::error(codes::INVALID_PARAMS, msg)];
    let mut obj: Map<String, Value> = match base {
        None => Map::new(),
        Some(base) => {
            let text = args_text(base).map_err(bad)?;
            let parsed = serde_json::from_str(&text).or_else(|_| relaxed::parse(&text));
            match parsed {
                Ok(Value::Object(o)) => o,
                Ok(_) => return Err(bad("`--args` must be an object: `{entity: e_8b21}`".into())),
                Err(e) => {
                    return Err(bad(format!(
                        "`--args` is not an object ({e}); write `{{entity: e_8b21, value: [1, 0, 0, 1]}}` (quotes optional), pass `--args @file.json`, or use `-p key=value`"
                    )));
                }
            }
        }
    };
    for p in params {
        let Some((key, raw)) = p.split_once('=') else {
            return Err(bad(format!("`--param {p}`: expected `key=value`")));
        };
        let value = serde_json::from_str(raw)
            .or_else(|_| relaxed::parse(raw))
            .unwrap_or_else(|_| Value::String(raw.to_owned()));
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

/// The calls of `ara do` without a command name.
pub struct Batch {
    /// `[{cmd, args, as}]`.
    pub commands: Value,
    /// `doc` from the batch object, if any.
    pub doc: Option<String>,
    /// `why` from the batch object, if any.
    pub why: Option<String>,
}

/// Reads `{commands: [{cmd, args, as}], doc?, why?}` (each call may also say
/// `command` / `params`, the registry's own names).
pub fn batch(args: Value) -> Result<Batch, Vec<Diagnostic>> {
    let bad = |msg: &str| vec![Diagnostic::error(codes::INVALID_PARAMS, msg)];
    let usage = "without a command name, `--args` is a batch: `{commands: [{cmd: entity.create, args: {name: Crate}, as: crate}], why: <reason>}`";
    let Value::Object(mut obj) = args else {
        return Err(bad(usage));
    };
    let Some(Value::Array(calls)) = obj.remove("commands") else {
        return Err(bad(usage));
    };
    let mut commands = Vec::new();
    for call in calls {
        let Value::Object(mut c) = call else {
            return Err(bad(usage));
        };
        for (alias, name) in [("command", "cmd"), ("params", "args")] {
            if !c.contains_key(name)
                && let Some(v) = c.remove(alias)
            {
                c.insert(name.into(), v);
            }
        }
        if !c.contains_key("cmd") {
            return Err(bad(usage));
        }
        commands.push(Value::Object(c));
    }
    let text = |v: Option<Value>| v.and_then(|v| v.as_str().map(str::to_owned));
    Ok(Batch {
        commands: Value::Array(commands),
        doc: text(obj.remove("doc")),
        why: text(obj.remove("why")),
    })
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
