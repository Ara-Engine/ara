//! Parses agent CLI event streams into one normalized record.
//!
//! - Claude Code: `claude -p --output-format stream-json --verbose` (one JSON event
//!   per line: `assistant` messages with `tool_use` blocks, `user` messages with
//!   `tool_result` blocks, a final `result` with turns, usage and cost).
//! - Codex: `codex exec --json` (`item.started` / `item.completed` for
//!   `mcp_tool_call`, `command_execution`, `file_change`, `agent_message`, and
//!   `turn.completed` with usage).
//!
//! Tool names are normalized: Ara MCP tools keep their name (`ara_apply`), shells
//! become `shell` (`{command}`), file writes become `edit` (`{path}`); anything else
//! keeps the CLI's name.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Kept per tool output, so transcripts stay small.
const OUTPUT_CAP: usize = 4000;

/// Which event format a stream uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Claude Code `stream-json`.
    Claude,
    /// Codex `exec --json`.
    Codex,
}

/// One tool call by the agent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Normalized tool name (`ara_apply`, `shell`, `edit`, `Read`).
    pub tool: String,
    /// Arguments (`shell`: `{command}`, `edit`: `{path}`).
    pub input: Value,
    /// False if the tool reported an error (MCP `isError`, a non-zero exit).
    pub ok: bool,
    /// ARA codes in the output of a failed call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codes: Vec<String>,
    /// The output, truncated.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
}

impl ToolCall {
    /// The shell command, for `shell` calls.
    pub fn command(&self) -> Option<&str> {
        (self.tool == "shell")
            .then(|| self.input.get("command").and_then(Value::as_str))
            .flatten()
    }

    /// True for an `ara_apply` with `dryRun`, or `ara do --dry-run`.
    pub fn is_dry_run(&self) -> bool {
        (self.tool == "ara_apply" && self.input.get("dryRun") == Some(&json!(true)))
            || self
                .command()
                .is_some_and(|c| c.contains("ara") && c.contains("--dry-run"))
    }

    /// True if this call ran `command` (an Ara command such as `entity.delete`) for
    /// real, through MCP or the CLI.
    pub fn applies(&self, command: &str) -> bool {
        if self.is_dry_run() || !self.ok {
            return false;
        }
        match self.tool.as_str() {
            "ara_apply" => self.input.to_string().contains(&format!("\"{command}\"")),
            "shell" => self
                .command()
                .is_some_and(|c| c.contains("ara") && c.contains(command)),
            _ => false,
        }
    }

    /// The document this call wrote outside the engine, if it looks like one.
    pub fn raw_edit(&self) -> Option<String> {
        let is_doc = |s: &str| s.contains(".scene") || s.contains(".prefab");
        match self.tool.as_str() {
            "edit" => self
                .input
                .get("path")
                .and_then(Value::as_str)
                .filter(|p| is_doc(p))
                .map(str::to_owned),
            "shell" => {
                let c = self.command()?;
                const WRITES: [&str; 14] = [
                    "Set-Content",
                    "Out-File",
                    "WriteAllText",
                    "sed -i",
                    "perl -i",
                    "apply_patch",
                    "> ",
                    ">>",
                    "tee ",
                    "Copy-Item",
                    "Move-Item",
                    "cp ",
                    "mv ",
                    ".write(",
                ];
                (is_doc(c) && WRITES.iter().any(|w| c.contains(w)))
                    .then(|| c.chars().take(160).collect())
            }
            _ => None,
        }
    }
}

/// Token usage as the CLI reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    /// Uncached input tokens.
    pub input: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read: u64,
    /// Input tokens written to the prompt cache.
    pub cache_write: u64,
    /// Output tokens.
    pub output: u64,
}

/// The parsed stream so far. Feed it lines with [`Transcript::push`].
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    /// Tool calls, in order.
    pub calls: Vec<ToolCall>,
    /// Tool-using steps (Claude: assistant messages with tool calls; Codex: tool
    /// items).
    pub turns: u32,
    /// Usage, when reported.
    pub tokens: Option<Tokens>,
    /// Cost in USD, when reported (Claude).
    pub cost_usd: Option<f64>,
    /// The agent's last message.
    pub final_message: String,
    /// True once the CLI reported the end of the run.
    pub done: bool,
    /// An error the CLI itself reported (auth, model, crash).
    pub cli_error: Option<String>,
    pending: BTreeMap<String, usize>,
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `ARA####` codes in `text`, in order of appearance, without repeats.
pub fn codes_in(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    for (i, _) in text.match_indices("ARA") {
        let digits = bytes.get(i + 3..i + 7);
        let next = bytes.get(i + 7);
        if let Some(d) = digits
            && d.iter().all(u8::is_ascii_digit)
            && !next.is_some_and(u8::is_ascii_digit)
        {
            let code = text[i..i + 7].to_owned();
            if !out.contains(&code) {
                out.push(code);
            }
        }
    }
    out
}

fn cap(text: &str) -> String {
    if text.len() <= OUTPUT_CAP {
        return text.to_owned();
    }
    let mut end = OUTPUT_CAP;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} bytes)", &text[..end], text.len())
}

fn tool_call(tool: String, input: Value) -> ToolCall {
    ToolCall {
        tool,
        input,
        ok: true,
        ..ToolCall::default()
    }
}

/// Claude Code's tool name and input, normalized.
fn claude_tool(name: &str, input: &Value) -> (String, Value) {
    if let Some(rest) = name.strip_prefix("mcp__") {
        let tool = rest.split_once("__").map_or(rest, |(_, t)| t);
        return (tool.to_owned(), input.clone());
    }
    match name {
        "Bash" | "PowerShell" => ("shell".into(), json!({ "command": input["command"] })),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => (
            "edit".into(),
            json!({ "path": input.get("file_path").or_else(|| input.get("notebook_path")) }),
        ),
        other => (other.to_owned(), input.clone()),
    }
}

impl Transcript {
    /// Parses one line of `format`. Lines that are not JSON events are ignored.
    pub fn push(&mut self, format: Format, line: &str) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match format {
            Format::Claude => self.claude(&event),
            Format::Codex => self.codex(&event),
        }
    }

    fn finish_call(&mut self, index: usize, ok: bool, output: &str) {
        if let Some(call) = self.calls.get_mut(index) {
            call.ok = ok;
            if !ok {
                call.codes = codes_in(output);
            }
            call.output = cap(output);
        }
    }

    fn claude(&mut self, e: &Value) {
        let content = e.pointer("/message/content").and_then(Value::as_array);
        match e["type"].as_str() {
            Some("assistant") => {
                let mut used_tool = false;
                for block in content.into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("tool_use") => {
                            used_tool = true;
                            let name = block["name"].as_str().unwrap_or_default();
                            let (tool, input) = claude_tool(name, &block["input"]);
                            let id = block["id"].as_str().unwrap_or_default().to_owned();
                            self.pending.insert(id, self.calls.len());
                            self.calls.push(tool_call(tool, input));
                        }
                        Some("text") => {
                            self.final_message = block["text"].as_str().unwrap_or("").to_owned();
                        }
                        _ => {}
                    }
                }
                self.turns += u32::from(used_tool);
            }
            Some("user") => {
                for block in content.into_iter().flatten() {
                    if block["type"] == "tool_result"
                        && let Some(i) = block["tool_use_id"]
                            .as_str()
                            .and_then(|id| self.pending.remove(id))
                    {
                        let ok = block["is_error"] != json!(true);
                        self.finish_call(i, ok, &text_of(&block["content"]));
                    }
                }
            }
            Some("result") => {
                self.done = true;
                if let Some(r) = e["result"].as_str() {
                    self.final_message = r.to_owned();
                }
                self.cost_usd = e["total_cost_usd"].as_f64();
                let u = &e["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                self.tokens = Some(Tokens {
                    input: n("input_tokens"),
                    cache_read: n("cache_read_input_tokens"),
                    cache_write: n("cache_creation_input_tokens"),
                    output: n("output_tokens"),
                });
                if e["is_error"] == json!(true) {
                    self.cli_error = Some(self.final_message.clone());
                }
            }
            _ => {}
        }
    }

    fn codex(&mut self, e: &Value) {
        let item = &e["item"];
        let id = item["id"].as_str().unwrap_or_default().to_owned();
        match (e["type"].as_str(), item["type"].as_str()) {
            (Some("item.started"), Some("mcp_tool_call" | "command_execution")) => {
                let (tool, input) = if item["type"] == "mcp_tool_call" {
                    let tool = item["tool"].as_str().unwrap_or_default().to_owned();
                    (tool, item["arguments"].clone())
                } else {
                    ("shell".to_owned(), json!({ "command": item["command"] }))
                };
                self.pending.insert(id, self.calls.len());
                self.calls.push(tool_call(tool, input));
                self.turns += 1;
            }
            (Some("item.completed"), Some("mcp_tool_call")) => {
                let index = self.pending.remove(&id).unwrap_or_else(|| {
                    let tool = item["tool"].as_str().unwrap_or_default().to_owned();
                    self.calls.push(tool_call(tool, item["arguments"].clone()));
                    self.turns += 1;
                    self.calls.len() - 1
                });
                let result = &item["result"];
                let failed = !item["error"].is_null()
                    || item["status"] == "failed"
                    || result["isError"] == json!(true)
                    || result["is_error"] == json!(true);
                let mut out = text_of(&result["content"]);
                if let Some(err) = item["error"].as_str().or(item["error"]["message"].as_str()) {
                    out.push_str(err);
                }
                self.finish_call(index, !failed, &out);
            }
            (Some("item.completed"), Some("command_execution")) => {
                let index = self.pending.remove(&id).unwrap_or_else(|| {
                    let input = json!({ "command": item["command"] });
                    self.calls.push(tool_call("shell".into(), input));
                    self.turns += 1;
                    self.calls.len() - 1
                });
                let ok = item["exit_code"].as_i64() == Some(0);
                let out = item["aggregated_output"].as_str().unwrap_or_default();
                self.finish_call(index, ok, out);
            }
            (Some("item.completed"), Some("file_change")) => {
                self.turns += 1;
                for change in item["changes"].as_array().into_iter().flatten() {
                    let mut call = tool_call("edit".into(), json!({ "path": change["path"] }));
                    call.ok = item["status"] != "failed";
                    self.calls.push(call);
                }
            }
            (Some("item.completed"), Some("agent_message")) => {
                self.final_message = item["text"].as_str().unwrap_or_default().to_owned();
            }
            (Some("turn.completed"), _) => {
                self.done = true;
                let u = &e["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                let t = self.tokens.get_or_insert_with(Tokens::default);
                t.input += n("input_tokens").saturating_sub(n("cached_input_tokens"));
                t.cache_read += n("cached_input_tokens");
                t.output += n("output_tokens");
            }
            (Some("turn.failed" | "error"), _) => {
                let msg = e["error"]["message"]
                    .as_str()
                    .or(e["message"].as_str())
                    .unwrap_or("codex reported an error");
                self.cli_error = Some(msg.to_owned());
            }
            _ => {}
        }
    }

    /// Calls per normalized tool name.
    pub fn tool_counts(&self) -> BTreeMap<String, u32> {
        let mut out = BTreeMap::new();
        for c in &self.calls {
            *out.entry(c.tool.clone()).or_insert(0) += 1;
        }
        out
    }

    /// How often each ARA code appeared in a failed call.
    pub fn code_counts(&self) -> BTreeMap<String, u32> {
        let mut out = BTreeMap::new();
        for code in self.calls.iter().flat_map(|c| &c.codes) {
            *out.entry(code.clone()).or_insert(0) += 1;
        }
        out
    }

    /// Documents (or commands touching them) the agent wrote outside the engine.
    pub fn raw_edits(&self) -> Vec<String> {
        self.calls.iter().filter_map(ToolCall::raw_edit).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_stream_normalizes_tools_codes_and_usage() {
        let mut t = Transcript::default();
        for line in [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__ara__ara_apply","input":{"doc":"levels/a.scene","dryRun":true}},{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"C:/p/levels/a.scene"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":[{"type":"text","text":"error[ARA1102]: expected an entity ID (ARA1102)"}]},{"type":"tool_result","tool_use_id":"t2","content":"ok"}]}}"#,
            r#"{"type":"result","num_turns":2,"result":"done","total_cost_usd":0.5,"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100}}"#,
        ] {
            t.push(Format::Claude, line);
        }
        assert_eq!(t.turns, 1);
        assert_eq!(t.calls[0].tool, "ara_apply");
        assert!(t.calls[0].is_dry_run() && !t.calls[0].ok);
        assert_eq!(t.code_counts(), BTreeMap::from([("ARA1102".into(), 1)]));
        assert_eq!(t.raw_edits(), vec!["C:/p/levels/a.scene".to_owned()]);
        assert_eq!(t.tokens.map(|k| k.cache_read), Some(100));
        assert!(t.done && t.final_message == "done");
    }

    #[test]
    fn codex_stream_pairs_items_and_counts_usage() {
        let mut t = Transcript::default();
        for line in [
            r#"{"type":"item.started","item":{"id":"i1","type":"mcp_tool_call","tool":"ara_session","arguments":{"action":"begin"}}}"#,
            r#"{"type":"item.completed","item":{"id":"i1","type":"mcp_tool_call","tool":"ara_session","arguments":{"action":"begin"},"result":{"content":[{"type":"text","text":"ok"}]},"error":null,"status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"i2","type":"command_execution","command":"ara do entity.delete --doc x.scene","aggregated_output":"error[ARA1204]: no entity","exit_code":1}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":120,"cached_input_tokens":100,"output_tokens":7}}"#,
        ] {
            t.push(Format::Codex, line);
        }
        assert_eq!(t.turns, 2);
        assert!(t.calls[0].ok);
        assert_eq!(t.calls[1].codes, vec!["ARA1204".to_owned()]);
        assert_eq!(t.tokens.map(|k| (k.input, k.cache_read)), Some((20, 100)));
    }
}
