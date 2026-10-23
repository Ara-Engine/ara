//! Agent CLIs and how to drive them headless against a project.
//!
//! An agent is `<cli>:<model>[@<effort>]`: `claude:sonnet-5.5`, `codex:gpt-6-luna@medium`.
//! Both get only the project's Ara MCP server (the `.mcp.json` that `ara init`
//! wrote, or Codex's equivalent `mcp_servers.ara` entry), with `ara` first on `PATH`,
//! permissions bypassed (each run lives in its own temp project), and the prompt on
//! stdin. User-level MCP servers, plugins and settings are left out so runs are
//! comparable; the user's own memory file (CLAUDE.md / AGENTS.md) still loads.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use crate::transcript::Format;

/// Which agent CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cli {
    /// Claude Code (`claude -p`).
    Claude,
    /// Codex (`codex exec`).
    Codex,
}

/// An agent CLI with a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    /// The CLI.
    pub cli: Cli,
    /// Model as written (`sonnet-5.5`, `gpt-6-luna`).
    pub model: String,
    /// Reasoning effort (`low`, `medium`, `high`); the CLI default when absent, except
    /// Codex, which defaults to `medium`.
    pub effort: Option<String>,
}

impl FromStr for AgentSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let (cli, rest) = s
            .split_once(':')
            .ok_or_else(|| format!("`{s}`: expected `claude:<model>` or `codex:<model>`"))?;
        let cli = match cli {
            "claude" => Cli::Claude,
            "codex" => Cli::Codex,
            other => return Err(format!("unknown agent CLI `{other}`; use claude or codex")),
        };
        let (model, effort) = match rest.split_once('@') {
            Some((m, e)) => (m, Some(e.to_owned())),
            None => (rest, None),
        };
        if model.is_empty() {
            return Err(format!("`{s}`: the model is missing"));
        }
        Ok(AgentSpec {
            cli,
            model: model.to_owned(),
            effort,
        })
    }
}

impl fmt::Display for AgentSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cli = match self.cli {
            Cli::Claude => "claude",
            Cli::Codex => "codex",
        };
        write!(f, "{cli}:{}", self.model)?;
        if let Some(e) = &self.effort {
            write!(f, "@{e}")?;
        }
        Ok(())
    }
}

impl AgentSpec {
    /// A file-name-safe form (`claude-sonnet-5.5`).
    pub fn slug(&self) -> String {
        self.to_string().replace([':', '@'], "-")
    }

    /// The event format of this CLI's stream.
    pub fn format(&self) -> Format {
        match self.cli {
            Cli::Claude => Format::Claude,
            Cli::Codex => Format::Codex,
        }
    }

    /// The model ID the CLI takes: `sonnet-5.5` → `claude-sonnet-5-5`; aliases
    /// (`sonnet`) and full IDs pass through.
    pub fn model_id(&self) -> String {
        match self.cli {
            Cli::Claude if self.model.starts_with("claude-") || !self.model.contains('-') => {
                self.model.clone()
            }
            Cli::Claude => format!("claude-{}", self.model.replace('.', "-")),
            Cli::Codex => self.model.clone(),
        }
    }

    /// The headless command for a run in `project`, with `ara_dir` first on `PATH`.
    /// The prompt goes to its stdin.
    pub fn command(&self, project: &Path, ara_dir: &Path) -> Result<Command, String> {
        let mut cmd = match self.cli {
            Cli::Claude => {
                let mut c = program("claude")?;
                let mcp = project.join(".mcp.json");
                c.args(["-p", "--output-format", "stream-json", "--verbose"])
                    .args(["--model", &self.model_id()])
                    .arg("--mcp-config")
                    .arg(mcp)
                    .args(["--strict-mcp-config", "--setting-sources", "project,local"])
                    .args(["--disable-slash-commands", "--no-session-persistence"])
                    .args(["--permission-mode", "bypassPermissions"]);
                if let Some(e) = &self.effort {
                    c.args(["--effort", e]);
                }
                c
            }
            Cli::Codex => {
                let mut c = program("codex")?;
                let effort = self.effort.as_deref().unwrap_or("medium");
                c.args(["exec", "--json", "--ephemeral", "--ignore-user-config"])
                    .args(["--ignore-rules", "--skip-git-repo-check"])
                    .arg("--dangerously-bypass-approvals-and-sandbox")
                    .args(["-m", &self.model_id()])
                    .args(["-c", &format!("model_reasoning_effort=\"{effort}\"")])
                    .args(["-c", "mcp_servers.ara.command=\"ara\""])
                    .args(["-c", "mcp_servers.ara.args=[\"mcp\"]"])
                    .arg("-C")
                    .arg(project)
                    .arg("-");
                c
            }
        };
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![ara_dir.to_path_buf()];
        dirs.extend(std::env::split_paths(&path));
        let joined = std::env::join_paths(dirs).map_err(|e| e.to_string())?;
        cmd.env("PATH", joined).current_dir(project);
        Ok(cmd)
    }
}

/// A command for CLI `name` found on `PATH`. npm shims (`codex.cmd`) are run through
/// `node` directly, so arguments are not re-parsed by `cmd.exe`.
fn program(name: &str) -> Result<Command, String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let exe = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if exe.is_file() {
            return Ok(Command::new(exe));
        }
        let script = dir.join("node_modules/@openai/codex/bin/codex.js");
        if cfg!(windows) && name == "codex" && dir.join("codex.cmd").is_file() && script.is_file() {
            let node = which("node").ok_or("`codex` needs `node` on PATH")?;
            let mut c = Command::new(node);
            c.arg(script);
            return Ok(c);
        }
    }
    Err(format!("`{name}` is not on PATH"))
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse_and_name_models() {
        let a: AgentSpec = "claude:sonnet-5.5".parse().expect("valid");
        assert_eq!(
            (a.cli, a.model_id().as_str()),
            (Cli::Claude, "claude-sonnet-5-5")
        );
        assert_eq!(a.slug(), "claude-sonnet-5.5");
        let b: AgentSpec = "codex:gpt-6-luna@low".parse().expect("valid");
        assert_eq!(
            (b.model_id().as_str(), b.effort.as_deref()),
            ("gpt-6-luna", Some("low"))
        );
        assert_eq!(b.to_string(), "codex:gpt-6-luna@low");
        assert!(
            "gemini:x".parse::<AgentSpec>().is_err() && "claude:".parse::<AgentSpec>().is_err()
        );
    }
}
