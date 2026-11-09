//! Agent clients (Claude Code, Codex, Cursor, OpenCode): detect each one and
//! register Ara's global MCP server (`<ara> mcp`) with it at user scope, the way
//! that client expects: its own CLI (Claude Code) or its config file (Codex TOML,
//! Cursor and OpenCode JSON). Other settings are kept. The Hub, the editor and
//! `ara hub install-mcp` share this, so every surface connects agents the same way.
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// An agent client Ara can detect and connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Client {
    /// Claude Code.
    Claude,
    /// OpenAI Codex CLI.
    Codex,
    /// Cursor (editor).
    Cursor,
    /// OpenCode CLI.
    OpenCode,
}

impl Client {
    /// Every client, in display order.
    pub const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::Cursor, Self::OpenCode];

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::OpenCode => "OpenCode",
        }
    }

    /// The `--client` value and asset name.
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::OpenCode => "opencode",
        }
    }

    /// The client's mark (Simple Icons, CC0; 24-unit view box). Claude's carries
    /// its brand colour; the others use `currentColor` so they can be tinted.
    pub fn logo(self) -> &'static str {
        match self {
            Self::Claude => include_str!("../assets/claude.svg"),
            Self::Codex => include_str!("../assets/codex.svg"),
            Self::Cursor => include_str!("../assets/cursor.svg"),
            Self::OpenCode => include_str!("../assets/opencode.svg"),
        }
    }

    /// The caption for [`manual`]: where a person adds the server by hand.
    pub fn manual_hint(self) -> &'static str {
        match self {
            Self::Claude => "Run this in a terminal",
            Self::Codex => "Add this to ~/.codex/config.toml",
            Self::Cursor => "Add this server to ~/.cursor/mcp.json",
            Self::OpenCode => "Add this to ~/.config/opencode/opencode.json",
        }
    }
}

/// Whether a client is installed and has Ara registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setup {
    /// The client was not found; its manual steps are offered on request.
    Missing,
    /// Installed, Ara not registered yet.
    Available,
    /// Registering now.
    Working,
    /// Ara is registered with this client.
    Connected,
    /// Registration failed; the message says why.
    Failed(String),
}

/// The config entry that registers `ara mcp` with `client`: JSON for Claude Code,
/// Cursor and OpenCode, a TOML table for Codex.
pub fn snippet(ara: &Path, client: Client) -> String {
    let path = dunce::simplified(ara).to_string_lossy();
    match client {
        Client::Claude | Client::Cursor => ara_json::to_canonical_string(
            &json!({"mcpServers":{"ara":{"command":path,"args":["mcp"]}}}),
        ),
        Client::Codex => {
            // JSON string escaping is also valid for this TOML basic string.
            let quoted = serde_json::to_string(&path).unwrap_or_default();
            format!("[mcp_servers.ara]\ncommand = {quoted}\nargs = [\"mcp\"]\n")
        }
        Client::OpenCode => ara_json::to_canonical_string(&json!({
            "$schema": "https://opencode.ai/config.json",
            "mcp": {"ara": {"type": "local", "command": [path, "mcp"], "enabled": true}}
        })),
    }
}

/// The manual step for `client` (caption: [`Client::manual_hint`]): Claude Code's
/// one-line registration, or the config entry for the others.
pub fn manual(ara: &Path, client: Client) -> String {
    if client != Client::Claude {
        return snippet(ara, client);
    }
    let path = dunce::simplified(ara).to_string_lossy();
    let quoted = if cfg!(windows) {
        format!("'{}'", path.replace('\'', "''"))
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    };
    format!("claude mcp add --scope user ara -- {quoted} mcp")
}

/// Whether `client` is installed and already has Ara registered. Blocks on the
/// client's CLI; run it off the UI thread.
pub fn detect(client: Client) -> Setup {
    let installed = match client {
        Client::Claude => succeeds("claude", &["--version"]),
        Client::Codex => succeeds("codex", &["--version"]),
        Client::Cursor => cursor_installed(),
        Client::OpenCode => succeeds("opencode", &["--version"]),
    };
    if !installed {
        return Setup::Missing;
    }
    let connected = match client {
        Client::Claude => succeeds("claude", &["mcp", "get", "ara"]),
        Client::Codex => codex_config()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .is_some_and(|config| config.contains("[mcp_servers.ara]")),
        Client::Cursor => cursor_config().is_some_and(|path| has_json(&path, CURSOR_KEYS)),
        Client::OpenCode => opencode_config().is_some_and(|path| has_json(&path, OPENCODE_KEYS)),
    };
    if connected {
        Setup::Connected
    } else {
        Setup::Available
    }
}

/// Register `<ara> mcp` with `client` at user scope. Blocks; run it off the UI thread.
pub fn setup(client: Client, ara: &Path) -> Setup {
    let result = (|| match client {
        Client::Claude => {
            let output = hidden(
                client_command("claude")
                    .args(["mcp", "add", "--scope", "user", "ara", "--"])
                    .arg(dunce::simplified(ara))
                    .arg("mcp"),
            )
            .output()
            .map_err(|error| error.to_string())?;
            if output.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
            }
        }
        Client::Codex => append_toml(&codex_config().ok_or(NO_HOME)?, &snippet(ara, client)),
        Client::Cursor => add_json(
            &cursor_config().ok_or(NO_HOME)?,
            &snippet(ara, client),
            CURSOR_KEYS,
        ),
        Client::OpenCode => add_json(
            &opencode_config().ok_or(NO_HOME)?,
            &snippet(ara, client),
            OPENCODE_KEYS,
        ),
    })();
    match result {
        Ok(()) => Setup::Connected,
        Err(error) if error.is_empty() => Setup::Failed("the client reported an error".into()),
        Err(error) => Setup::Failed(error),
    }
}

const NO_HOME: &str = "no home folder for the agent's settings";
const CURSOR_KEYS: [&str; 2] = ["mcpServers", "ara"];
const OPENCODE_KEYS: [&str; 2] = ["mcp", "ara"];

/// A console tool that never flashes a window from a GUI process.
fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW.
        command.creation_flags(0x0800_0000);
    }
    command
}

/// A client CLI by name. On Windows npm installs `claude.cmd`/`codex.cmd` shims,
/// which only `cmd /C` resolves.
fn client_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", program]);
        command
    } else {
        Command::new(program)
    }
}

fn succeeds(program: &str, args: &[&str]) -> bool {
    hidden(client_command(program).args(args))
        .output()
        .is_ok_and(|output| output.status.success())
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// `~/.codex/config.toml` (`CODEX_HOME` overrides the folder).
fn codex_config() -> Option<PathBuf> {
    let folder = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".codex")))?;
    Some(folder.join("config.toml"))
}

/// Cursor is a desktop app without a CLI requirement: its per-user folder or
/// its default install location shows it is present.
fn cursor_installed() -> bool {
    let local = std::env::var_os("LOCALAPPDATA")
        .map(|folder| PathBuf::from(folder).join("Programs/cursor/Cursor.exe"));
    home().is_some_and(|home| home.join(".cursor").is_dir())
        || local.is_some_and(|exe| exe.is_file())
        || Path::new("/Applications/Cursor.app").is_dir()
}

/// `~/.cursor/mcp.json`, Cursor's global MCP servers.
fn cursor_config() -> Option<PathBuf> {
    Some(home()?.join(".cursor").join("mcp.json"))
}

/// OpenCode's global config: `$XDG_CONFIG_HOME/opencode` (default `~/.config`,
/// on Windows too), preferring an existing `opencode.json` or `opencode.jsonc`.
fn opencode_config() -> Option<PathBuf> {
    let folder = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".config")))?
        .join("opencode");
    let jsonc = folder.join("opencode.jsonc");
    let json = folder.join("opencode.json");
    Some(if !json.is_file() && jsonc.is_file() {
        jsonc
    } else {
        json
    })
}

/// Whether a JSON config has `keys` (`mcpServers.ara`). Files with comments fall
/// back to a plain text search for an `ara` entry.
fn has_json(path: &Path, [section, name]: [&str; 2]) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(config) => config[section].get(name).is_some(),
        Err(_) => text.contains(&format!("\"{name}\"")) && text.contains(section),
    }
}

/// Add Ara's entry from `snippet` under `keys` of a JSON config, keeping every
/// other setting and its order. A missing file becomes the whole snippet.
fn add_json(path: &Path, snippet: &str, [section, name]: [&str; 2]) -> Result<(), String> {
    let snippet: Value = serde_json::from_str(snippet).map_err(|error| error.to_string())?;
    let mut config = match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => json!({}),
        Ok(text) => serde_json::from_str(&text).map_err(|_| {
            format!(
                "{} is not plain JSON (comments?); add Ara with its manual steps",
                path.display()
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => snippet.clone(),
        Err(error) => return Err(error.to_string()),
    };
    let invalid = || format!("{} has an unexpected layout", path.display());
    config
        .as_object_mut()
        .ok_or_else(invalid)?
        .entry(section)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(invalid)?
        .insert(name.into(), snippet[section][name].clone());
    write(
        path,
        &(serde_json::to_string_pretty(&config).map_err(|e| e.to_string())? + "\n"),
    )
}

/// Append the `[mcp_servers.ara]` table unless Codex already has one.
fn append_toml(path: &Path, entry: &str) -> Result<(), String> {
    let mut config = std::fs::read_to_string(path).unwrap_or_default();
    if config.contains("[mcp_servers.ara]") {
        return Ok(());
    }
    if !config.is_empty() && !config.ends_with('\n') {
        config.push('\n');
    }
    config.push('\n');
    config.push_str(entry.trim_end());
    config.push('\n');
    write(path, &config)
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, text).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_setup_keeps_other_servers_and_detects_the_entry() {
        let folder = std::env::temp_dir().join(format!("ara-agents-{}", std::process::id()));
        let path = folder.join("mcp.json");
        let snippet = snippet(Path::new("C:/Ara/ara.exe"), Client::Cursor);
        write(
            &path,
            r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}}}"#,
        )
        .expect("seed");
        assert!(!has_json(&path, CURSOR_KEYS));
        add_json(&path, &snippet, CURSOR_KEYS).expect("add");
        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(config["theme"], "dark");
        assert_eq!(config["mcpServers"]["other"]["command"], "x");
        assert_eq!(config["mcpServers"]["ara"]["args"], json!(["mcp"]));
        assert!(has_json(&path, CURSOR_KEYS));
        // Commented JSON is left untouched; the manual steps cover it.
        write(&path, "// mine\n{}").expect("seed");
        assert!(add_json(&path, &snippet, CURSOR_KEYS).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "// mine\n{}");
        let _ = std::fs::remove_dir_all(folder);
    }
}
