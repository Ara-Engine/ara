//! `build`: authors a sample project through Ara's own tools, as an agent would.
//! Content edits are typed command batches applied by the `ara` CLI (`ara do --args
//! @batch.json`), so the documents carry the same history and review state as any
//! agent session. Where no command exists yet (new empty documents, import sidecars,
//! animation controllers) the build writes canonical documents with the engine's own
//! writers, and each such gap is listed in `samples/FINDINGS.md`.
pub mod anim;
pub mod hangar;
pub mod kits;
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
    worker: PathBuf,
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
    /// A context for `samples/<project>`, initializing the project with `ara init`
    /// when it does not exist yet.
    pub fn open(samples: &Path, project: &str, title: &str, lock: Lock) -> Result<Self> {
        let root = samples.join(project);
        let ara = tool("ara", "ARA_BIN")?;
        let worker = tool("ara-import-worker", "ARA_IMPORT_WORKER")?;
        if !root.join("ara.project.json").is_file() {
            run(Command::new(&ara)
                .arg("init")
                .arg(&root)
                .args(["--name", title]))?;
            // `ara init` seeds an empty main scene; the samples name their own levels.
            let _ = std::fs::remove_file(root.join("levels/main.scene"));
        }
        Ok(Self {
            root,
            lock,
            ara,
            worker,
            batches: Cell::new(0),
        })
    }

    /// Writes `ara.project.json` (name and main scene).
    pub fn project_settings(&self, name: &str, main: &str) -> Result<()> {
        let text = serde_json::to_string_pretty(&json!({"name": name, "main": main}))?;
        std::fs::write(self.root.join("ara.project.json"), text + "\n")?;
        Ok(())
    }

    /// Creates an empty canonical document (`ara init` style) so commands can fill it:
    /// the engine has no document-creation command yet. Replaces an existing file.
    pub fn new_document(&self, path: &str, kind: &str, id: &str) -> Result<()> {
        let file = self.root.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = format!(
            "{{\n  \"$schema\": \"ara://schema/{kind}/1\",\n  \"id\": \"{id}\",\n  \"records\": {{}}\n}}\n"
        );
        std::fs::write(file, text)?;
        Ok(())
    }

    /// Runs `ara` in the project with JSON output; fails on `ok: false`.
    pub fn ara(&self, args: &[&str]) -> Result<Value> {
        let output = Command::new(&self.ara)
            .arg("--root")
            .arg(&self.root)
            .arg("--json")
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
            let dir = self.root.join(".scratch/batches");
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
            std::fs::write(&file, serde_json::to_string_pretty(&batch)?)?;
            let arg = format!("@{}", file.display());
            self.ara(&["do", "--args", &arg])
                .with_context(|| format!("batch {}", file.display()))?;
        }
        Ok(())
    }

    /// Writes an import sidecar for `source` (project-relative) through the asset
    /// crate's typed `SidecarCommand` and the canonical writer, recording the
    /// installed importer version. No engine command creates sidecars outside the editor.
    pub fn sidecar(&self, source: &str, id: &str, importer: &str, settings: Value) -> Result<()> {
        ensure!(
            self.root.join(source).is_file(),
            "{source} is missing; run `ara-samples fetch` first"
        );
        let versions = ara_asset::ProcessWorker::new(&self.worker)
            .versions()
            .map_err(|e| anyhow::anyhow!("asking the import worker for versions: {e}"))?;
        let version = versions
            .get(importer)
            .with_context(|| format!("the import worker has no {importer}"))?;
        let mut db = ara_schema::SchemaDb::new();
        ara_doc::register(&mut db);
        ara_asset::register_schema(&mut db);
        let asset = ara_doc::AssetId::parse(id).map_err(anyhow::Error::msg)?;
        let mut document = ara_doc::Document::new("import", asset.clone());
        let edit = ara_asset::SidecarCommand::Initialize(ara_asset::ImportSettings {
            importer: importer.into(),
            version: version.clone(),
            settings,
            ..Default::default()
        })
        .edit(asset)?;
        document
            .apply(edit, &db)
            .map_err(|d| anyhow::anyhow!("{d}"))?;
        std::fs::write(
            self.root.join(format!("{source}.import.json")),
            ara_doc::write(&document, &db),
        )?;
        Ok(())
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

/// Removes generated content so a build starts from the same state every time.
pub fn clean(root: &Path, paths: &[&str]) -> Result<()> {
    for path in paths {
        let path = root.join(path);
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else if path.exists() {
            std::fs::remove_file(&path)?;
        }
    }
    // A fresh journal: history describes this build only.
    let state = root.join(".ara");
    if state.is_dir() {
        for entry in std::fs::read_dir(&state)?.flatten() {
            let name = entry.file_name();
            if name == "journal" || name == "lock" {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}
