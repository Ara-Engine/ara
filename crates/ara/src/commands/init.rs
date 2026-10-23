//! `ara init [dir]`: a new project with the files agents and git need.
//!
//! Writes `ara.project.json` (name and main scene), `AGENTS.md` (engine conventions), `.gitattributes`
//! (`ara merge` as the merge driver for documents), `.gitignore` (`.ara/` local
//! state), `.mcp.json` (the MCP server for Claude Code / Codex) and a starter scene.
//! Existing files are kept; `.gitignore`/`.gitattributes` get missing lines
//! appended. Inside a git work tree the `merge.ara` driver is configured too.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ara_diag::{Diagnostic, codes};
use ara_doc::IdGen;
use ara_host::project::MARKER;
use serde_json::json;

const AGENTS: &str = r#"# AGENTS.md: {name}

An Ara game project. These are the engine's conventions; follow them in every
edit.

## Conventions

- **Axes and units:** Y-up, right-handed. Lengths in meters, time in seconds,
  mass in kilograms, angles in **degrees** (`Transform.rotation` is XYZ Euler
  degrees). Each field's unit is in its schema (`x-unit`; `ara schema`).
- **Documents** (`*.scene`, `*.prefab`) are canonical JSON: a flat `records` map
  keyed by short IDs, defaults omitted. Never hand-order keys; `ara fmt` owns the
  layout.
- **IDs:** `<prefix>_<base32>`: entities `e_8b21`, scenes `sc_…`, prefabs `pf_…`.
  An entity inside a prefab instance is addressed through it: `e_4f9a/e_m4tz`.
- **Naming entities in commands:** an ID or address, a batch symbol (`$crate`),
  or a name: `name:Crate`, or `name:Floor/Crate` when the name alone matches
  several entities.
- **References:** `@path#id` (`@prefabs/crate.prefab#pf_crate001`). Either half
  repairs the other.
- **Use commands, not raw file surgery, for graphs**, hierarchy (parents, order)
  and prefab overrides: `ara_apply` over MCP or `ara do <command> -p key=value`.
  `doc` is optional: the engine picks the document holding the entities you name,
  else the main scene (`levels/main.scene`). Batches are atomic; mint IDs with
  `"as": "$crate"` and use them in later calls. Preview with `dryRun`. Small
  value edits straight in the JSON are fine: the engine validates them.
- **Review:** every change is a transaction with a `why` (agents must give one).
  Agent changes apply live but stay unreviewed until a human runs `ara accept` or
  `ara revert`.
- **When something is wrong, ask the engine first:** `ara why name:Crate not
  visible`, `ara why ARA1302`, `ara why "is the frame slow"` (MCP: `ara_why`). It
  answers with ranked causes, their evidence and fixes you can run as is.

## Tools

- MCP: `.mcp.json` starts `ara mcp` (13 tools). Over `ara mcp` the session is
  optional (one begins on first use); `ara_session {"action": "begin", "client":
  "<you>"}` names yours. Codex: add `[mcp_servers.ara]` with `command = "ara"`,
  `args = ["mcp"]` to its config.
- CLI: `ara check` (validate), `ara query "Light"` / `ara query Crate`,
  `ara inspect` (main scene) or `ara inspect --doc outpost name:Crate`,
  `ara do property.set -p entity=name:Crate -p path=Light.intensity -p value=500
  --why "<reason>"`, `ara history`, `ara undo`, `ara logs`, `ara why`, `ara schema`. Add
  `--json` to any. `--args` takes JSON with or without quotes (`{entity: e_8b21}`),
  `@file.json` or `-` (stdin). The CLI attributes changes to the agent running it
  (Codex, Claude Code) or to `ARA_AUTHOR`.
- Reference for this engine version: `ara docs --llms` writes
  `.ara/docs/llms.txt` and per-type pages.
- `ara serve` runs the project daemon; the CLI and `ara mcp` use it when it runs.
"#;

const GITATTRIBUTES: &[&str] = &[
    "# Ara documents merge record by record (`ara merge`).",
    "*.scene merge=ara",
    "*.prefab merge=ara",
    "* text=auto eol=lf",
];

const GITIGNORE: &[&str] = &[
    "# Ara local state: journal, token, daemon file, generated docs.",
    ".ara/",
];

const MCP: &str = r#"{
  "mcpServers": {
    "ara": {
      "command": "ara",
      "args": ["mcp"]
    }
  }
}
"#;

/// What `init` did.
struct Report {
    created: Vec<String>,
    kept: Vec<String>,
}

impl Report {
    fn write(&mut self, root: &Path, rel: &str, text: &str) -> Result<(), String> {
        let path = root.join(rel);
        if path.exists() {
            self.kept.push(rel.to_owned());
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create `{}`: {e}", dir.display()))?;
        }
        fs::write(&path, text).map_err(|e| format!("cannot write `{rel}`: {e}"))?;
        self.created.push(rel.to_owned());
        Ok(())
    }

    /// Appends the lines `rel` lacks (creating it if needed).
    fn lines(&mut self, root: &Path, rel: &str, lines: &[&str]) -> Result<(), String> {
        let path = root.join(rel);
        let existing = fs::read_to_string(&path).unwrap_or_default();
        let missing: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|l| !existing.lines().any(|e| e.trim() == *l))
            .collect();
        if missing.is_empty() {
            self.kept.push(rel.to_owned());
            return Ok(());
        }
        let mut text = existing.clone();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&(missing.join("\n") + "\n"));
        fs::write(&path, text).map_err(|e| format!("cannot write `{rel}`: {e}"))?;
        if existing.is_empty() {
            self.created.push(rel.to_owned());
        } else {
            self.kept.push(format!("{rel} (lines added)"));
        }
        Ok(())
    }
}

/// Configures the `merge.ara` driver if `root` is inside a git work tree.
fn configure_git(root: &Path) -> String {
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(root).args(args).output();
    let top = match git(&["rev-parse", "--show-toplevel"]) {
        Ok(o) if o.status.success() => PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()),
        _ => {
            return "not a git work tree; in one, run `git config merge.ara.driver \"ara merge %O %A %B\"`".into();
        }
    };
    // Git runs merge drivers from the top of the work tree.
    let rel = match (fs::canonicalize(&top), fs::canonicalize(root)) {
        (Ok(t), Ok(r)) => r
            .strip_prefix(&t)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let rel = if rel.is_empty() { ".".to_owned() } else { rel };
    let driver = format!("ara --root \"{rel}\" merge %O %A %B");
    let ok = git(&["config", "merge.ara.name", "Ara semantic merge"])
        .is_ok_and(|o| o.status.success())
        && git(&["config", "merge.ara.driver", &driver]).is_ok_and(|o| o.status.success());
    if ok {
        format!("configured git merge driver `{driver}`")
    } else {
        "could not configure the git merge driver".into()
    }
}

/// Creates a project in `dir` (default: the current directory).
pub fn init(dir: Option<&Path>, name: Option<&str>, json_out: bool) -> Result<bool, String> {
    let root = dir.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    fs::create_dir_all(&root).map_err(|e| format!("cannot create `{}`: {e}", root.display()))?;
    let root =
        fs::canonicalize(&root).map_err(|e| format!("cannot open `{}`: {e}", root.display()))?;
    if root.join(MARKER).exists() {
        let d = Diagnostic::error(
            codes::PROJECT_EXISTS,
            format!("`{}` already has `{MARKER}`", root.display()),
        );
        crate::engine::print_diags(&[d], json_out);
        return Ok(false);
    }
    let name = name.map(str::to_owned).unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "game".into())
    });
    let mut r = Report {
        created: Vec::new(),
        kept: Vec::new(),
    };
    let marker =
        ara_json::to_canonical_string(&json!({ "name": name, "main": "levels/main.scene" }));
    r.write(&root, MARKER, &marker)?;
    r.write(&root, "AGENTS.md", &AGENTS.replace("{name}", &name))?;
    r.lines(&root, ".gitattributes", GITATTRIBUTES)?;
    r.lines(&root, ".gitignore", GITIGNORE)?;
    r.write(&root, ".mcp.json", MCP)?;
    let scene = json!({
        "$schema": "ara://schema/scene/1",
        "id": IdGen::from_entropy().asset_id("sc").to_string(),
        "records": {}
    });
    r.write(
        &root,
        "levels/main.scene",
        &ara_json::to_canonical_string(&scene),
    )?;
    let git = configure_git(&root);
    if json_out {
        print!(
            "{}",
            ara_json::to_canonical_string(&json!({
                "root": root.display().to_string(),
                "created": r.created,
                "kept": r.kept,
                "git": git,
            }))
        );
    } else {
        println!("initialized Ara project `{name}` in {}", root.display());
        for f in &r.created {
            println!("  created {f}");
        }
        for f in &r.kept {
            println!("  kept    {f}");
        }
        println!("  {git}");
        println!("next: `ara check`, then `ara serve` (or let `ara mcp` start the engine)");
    }
    Ok(true)
}
