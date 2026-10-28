//! `build`: authors a sample project through Ara's own tools, as an agent would.
//! Content edits are typed command batches applied by the `ara` CLI (`ara do --args
//! @batch.json`), so the documents carry the same history and review state as any
//! agent session. Documents, import sidecars and animation controllers use the same
//! typed commands available to agents through the CLI and MCP.
pub mod anim;
mod bounds;
pub mod hangar;
pub mod kits;
mod materials;
mod native;
pub mod outpost;
pub mod place;
pub mod showcase;

use crate::lock::Lock;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    process::Command,
};

/// One project being built.
pub struct Ctx {
    /// Project root.
    pub root: PathBuf,
    /// The pinned assets.
    pub lock: Lock,
    ara: PathBuf,
    batches: Cell<usize>,
}

/// An engine executable: `ARA_<NAME>` (e.g. `ARA_BIN`), next to this binary, or the
/// workspace's release then debug build.
pub fn tool(name: &str, env: &str) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(env) {
        return Ok(PathBuf::from(path));
    }
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(&file));
    }
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    candidates.push(target.join("release").join(&file));
    candidates.push(target.join("debug").join(&file));
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .with_context(|| {
            format!("{name} not found: `cargo build --release -p {name}` or set {env}")
        })
}

impl Ctx {
    /// Physical source/document path; authoring commands and references use short paths.
    pub fn path(&self, path: impl AsRef<Path>) -> PathBuf {
        crate::authored_path(&self.root, path)
    }

    /// A context for `samples/<project>`, initializing the project with `ara init`
    /// when it does not exist yet.
    pub fn open(samples: &Path, project: &str, title: &str, lock: Lock) -> Result<Self> {
        let root = samples.join(project);
        let ara = tool("ara", "ARA_BIN")?;
        if !root.join("ara.project.json").is_file() {
            run(Command::new(&ara)
                .arg("init")
                .arg(&root)
                .args(["--name", title]))?;
        }
        Ok(Self {
            root,
            lock,
            ara,
            batches: Cell::new(0),
        })
    }

    /// Selects the completed main scene through the project settings command.
    pub fn set_main(&self, main: &str, id: &str) -> Result<()> {
        self.apply(
            "ara.project.json",
            "Select the sample's main scene",
            vec![("project.setMain", json!({"scene": format!("@{main}#{id}")}))],
        )
    }

    /// Creates an empty canonical document through the shared authoring command.
    pub fn new_document(&self, path: &str, kind: &str, id: &str) -> Result<()> {
        self.apply(
            "ara.project.json",
            "Create a sample document",
            vec![("doc.create", json!({"path": path, "kind": kind, "id": id}))],
        )
    }

    /// Runs `ara` in the project with JSON output; fails on `ok: false`.
    pub fn ara(&self, args: &[&str]) -> Result<Value> {
        let output = Command::new(&self.ara)
            .arg("--root")
            .arg(&self.root)
            .arg("--json")
            .args(["--author", "agent:ara-samples"])
            .args(args)
            .output()
            .context("running ara")?;
        let text = String::from_utf8_lossy(&output.stdout);
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.to_string()));
        if !output.status.success() || value["ok"] == json!(false) {
            bail!(
                "ara {} failed ({}):\n{}\n{}",
                args.join(" "),
                output.status,
                serde_json::to_string_pretty(&value).unwrap_or_default(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(value)
    }

    /// Applies `commands` (`[cmd, args]` pairs) to `doc` as one transaction through
    /// `ara do`. Large batches are split so each stays reviewable.
    pub fn apply(&self, doc: &str, why: &str, commands: Vec<(&str, Value)>) -> Result<()> {
        for chunk in commands.chunks(120) {
            let n = self.batches.get() + 1;
            self.batches.set(n);
            let dir = self.root.join(".ara/samples/batches");
            std::fs::create_dir_all(&dir)?;
            let stem = Path::new(doc)
                .file_name()
                .map_or_else(|| "doc".into(), |s| s.to_string_lossy().replace('.', "-"));
            let file = dir.join(format!("{n:03}-{stem}.json"));
            let batch = json!({
                "doc": doc,
                "why": why,
                "commands": chunk.iter().map(|(cmd, args)| json!({"cmd": cmd, "args": args})).collect::<Vec<_>>(),
            });
            std::fs::write(&file, ara_json::to_canonical_string(&batch))?;
            let arg = format!("@{}", file.display());
            self.ara(&["do", "--args", &arg])
                .with_context(|| format!("batch {}", file.display()))?;
        }
        Ok(())
    }

    /// Initializes or reconfigures a sidecar with engine-selected import/version data.
    pub fn sidecar(&self, source: &str, id: &str, settings: Value) -> Result<()> {
        ensure!(
            self.path(source).is_file(),
            "{source} is missing; run `ara-samples fetch` first"
        );
        let settings = if settings.is_null() {
            json!({})
        } else {
            settings
        };
        let sidecar = self.path(format!("{source}.import.json"));
        let (command, args) = if sidecar.is_file() {
            let existing: Value = serde_json::from_slice(&std::fs::read(&sidecar)?)?;
            ensure!(
                existing["id"] == id,
                "{} has a different asset identity",
                sidecar.display()
            );
            (
                "import.configure",
                json!({"source":source, "settings":settings}),
            )
        } else {
            (
                "import.initialize",
                json!({"source":source, "id":id, "settings":settings}),
            )
        };
        self.apply(
            "ara.project.json",
            "Configure sample asset import",
            vec![(command, args)],
        )
    }

    /// `ara check` over every document; errors fail the build.
    pub fn check(&self) -> Result<()> {
        self.ara(&["check"]).map(drop)
    }
}

/// Runs a command, failing with its output.
fn run(command: &mut Command) -> Result<()> {
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "{command:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Regenerates documents through typed lifecycle ops, retaining the build journal.
/// Only non-document generated assets and scratch files are removed directly.
pub fn clean(ctx: &Ctx, paths: &[&str]) -> Result<()> {
    fn collect(dir: &Path, documents: &mut std::collections::BTreeSet<String>) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                collect(&path, documents)?;
                continue;
            }
            let name = path.to_string_lossy();
            if ![
                ".scene",
                ".prefab",
                ".terrain",
                ".foliage",
                ".animation",
                ".music",
                ".graph",
                ".materialinstance",
                ".import.json",
            ]
            .iter()
            .any(|extension| name.ends_with(extension))
            {
                continue;
            }
            let value: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            if let Some(id) = value["id"].as_str() {
                documents.insert(id.to_owned());
            }
        }
        Ok(())
    }
    let mut documents = std::collections::BTreeSet::new();
    for path in paths {
        collect(&ctx.path(path), &mut documents)?;
    }
    collect(&ctx.path("third_party"), &mut documents)?;
    let calls = documents
        .iter()
        .map(|id| ("doc.delete", json!({"document":format!("@#{id}")})))
        .collect();
    ctx.apply(
        "ara.project.json",
        "Regenerate sample documents with history and undo",
        calls,
    )?;
    let root = std::fs::canonicalize(&ctx.root)?;
    for path in paths {
        let path = ctx.path(path);
        if path.exists() {
            ensure!(
                std::fs::canonicalize(&path)?.starts_with(&root),
                "generated cleanup must stay inside the sample project"
            );
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
    }
    Ok(())
}
