//! Reference solutions run through [`Solver`]: an in-process host with an agent
//! session, so their calls go through the same Engine Protocol methods an agent's
//! MCP calls do. Each call is recorded as a normalized [`ToolCall`], so verifiers
//! that look at calls (dry runs) see the same shape as from a transcript.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ara_host::{Host, HostConfig, Project};
use ara_proto::ConnId;
use serde_json::{Value, json};

use crate::transcript::ToolCall;

/// The author reference solutions (and setup hooks) commit as, unless overridden.
pub const AUTHOR: &str = "agent:reference";

/// A session on a project's engine, recording its calls.
pub struct Solver {
    root: PathBuf,
    host: Option<Host>,
    session: String,
    /// The calls made, in order.
    pub calls: Vec<ToolCall>,
}

impl Solver {
    /// Opens the project and begins a session attributed to `author`.
    pub fn open(root: &Path, author: &str) -> Result<Solver, String> {
        let project = Project::open(root).map_err(|d| d.to_string())?;
        let mut host = Host::open(project, HostConfig::default()).map_err(|d| d.to_string())?;
        let begun = host
            .call(
                ConnId::LOCAL,
                "session.begin",
                json!({ "client": "arabench", "author": author }),
            )
            .map_err(|e| e.to_string())?;
        let session = begun["session"].as_str().unwrap_or_default().to_owned();
        Ok(Solver {
            root: root.to_path_buf(),
            host: Some(host),
            session,
            calls: Vec::new(),
        })
    }

    /// The project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Calls `method` with `params` plus the session.
    pub fn call(&mut self, method: &str, mut params: Value) -> Result<Value, String> {
        let host = self.host.as_mut().ok_or("the solver is closed")?;
        if let Some(o) = params.as_object_mut() {
            o.insert("session".into(), self.session.clone().into());
        }
        host.call(ConnId::LOCAL, method, params)
            .map_err(|e| format!("{method}: {e}"))
    }

    fn batch(&mut self, doc: &str, why: &str, commands: Value, dry: bool) -> Result<Value, String> {
        let params = json!({ "doc": doc, "why": why, "commands": commands, "dryRun": dry });
        let result = self.call("cmd.apply", params.clone());
        self.calls.push(ToolCall {
            tool: "ara_apply".into(),
            input: params,
            ok: result.is_ok(),
            ..ToolCall::default()
        });
        result
    }

    /// Applies `commands` (`[{cmd, args, as?}]`) to `doc` as one transaction.
    pub fn apply(&mut self, doc: &str, why: &str, commands: Value) -> Result<Value, String> {
        self.batch(doc, why, commands, false)
    }

    /// Previews `commands` without applying them.
    pub fn dry_run(&mut self, doc: &str, why: &str, commands: Value) -> Result<Value, String> {
        self.batch(doc, why, commands, true)
    }

    /// The address of the one entity of `doc` named `name`.
    pub fn find(&mut self, doc: &str, name: &str) -> Result<String, String> {
        let found = self.call("query", json!({ "doc": doc, "q": format!("name:{name}") }))?;
        let items: Vec<&str> = found["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["name"] == name)
            .filter_map(|i| i["entity"].as_str())
            .collect();
        match items.as_slice() {
            [one] => Ok((*one).to_owned()),
            other => Err(format!("{} entities named `{name}`", other.len())),
        }
    }

    /// Writes every changed document to disk now (before editing a file by hand).
    pub fn flush(&mut self) {
        if let Some(host) = self.host.as_mut() {
            host.flush();
        }
    }

    /// Writes a project file (outside the engine, like an agent's file edit).
    pub fn write(&mut self, rel: &str, text: &str) -> Result<(), String> {
        let path = self.root.join(rel);
        fs::write(&path, text).map_err(|e| format!("cannot write `{rel}`: {e}"))?;
        self.calls.push(ToolCall {
            tool: "edit".into(),
            input: json!({ "path": rel }),
            ok: true,
            ..ToolCall::default()
        });
        Ok(())
    }

    /// Writes an authored-root-relative file, recording its actual storage path.
    pub fn write_authored(&mut self, rel: &str, text: &str) -> Result<(), String> {
        self.write(&format!("{}/{rel}", ara_doc::AUTHORING_ROOT), text)
    }

    /// Runs `git <args>` in the project.
    pub fn git(&mut self, args: &[&str]) -> Result<String, String> {
        git(&self.root, args)
    }

    /// Ends the session and closes the host (flushing documents to disk). Returns the
    /// recorded calls.
    pub fn close(mut self) -> Vec<ToolCall> {
        let _ = self.call("session.end", json!({}));
        if let Some(mut host) = self.host.take() {
            host.flush();
        }
        std::mem::take(&mut self.calls)
    }
}

/// Runs `git <args>` in `root`; its stdout, or an error with stderr.
pub fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}
