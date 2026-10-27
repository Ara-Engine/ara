//! Shipped analyzer fixes prepared off-thread, committed only as typed source.replace ops.
use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::{FileState, PreparedInventory};
use ara_host::{
    Cx, Method, Service,
    file_state::FileHistory,
    params::{ApplyParams, CallSpec},
    parse,
};
use ara_proto::Scope;
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};

/// Analyze installed C# code fixes and apply or preview their ordinary typed commands.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceFixParams {
    /// Live protocol session; attribution and edit-code scope are preserved.
    pub session: String,
    /// Source reference or storage paths; absent analyzes all inventoried sources.
    pub paths: Option<Vec<String>>,
    /// Analyzer codes; absent selects ARA0001, ARA0101 and ARA0506.
    pub codes: Option<Vec<String>>,
    /// Apply the replacements (default true); false returns a dry-run diff and commands.
    pub apply: Option<bool>,
    /// Authoring document anchoring the transaction; default is the project main scene.
    pub doc: Option<String>,
    /// Why these fixes are being made; required for agent applications.
    pub why: Option<String>,
}

struct Prepared {
    inventory: PreparedInventory,
    commands: Vec<CallSpec>,
    fixes: Vec<Value>,
}
struct Pending {
    params: SourceFixParams,
    revision: u64,
    job: String,
    ready: Receiver<Result<Prepared, Vec<Diagnostic>>>,
    revalidated: bool,
}

/// A windowless SDK tool service; usable in the daemon, offline CLI and native editor.
pub struct SourceFixService {
    root: PathBuf,
    pending: Vec<Pending>,
    committed: Vec<(String, u64, Value)>,
}
impl SourceFixService {
    /// Register through Host::register alongside the other project services.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            pending: Vec::new(),
            committed: Vec::new(),
        }
    }
}
fn failure(error: impl std::fmt::Display) -> Vec<Diagnostic> {
    vec![Diagnostic::error(codes::IO, error.to_string())]
}

fn prepare(
    root: &Path,
    paths: Option<&[String]>,
    codes: &[String],
) -> Result<Prepared, Vec<Diagnostic>> {
    let inventory = ara_script::project::inventory(root).map_err(|d| vec![d])?;
    if !inventory.diagnostics.is_empty() {
        return Err(inventory.diagnostics);
    }
    let mut selected = BTreeSet::new();
    if let Some(paths) = paths {
        for path in paths {
            let matches: Vec<_> = inventory
                .sources
                .iter()
                .filter(|s| {
                    ara_doc::file_state::same_path(&s.path, path)
                        || ara_doc::file_state::same_path(&s.storage_path, path)
                })
                .collect();
            if matches.len() != 1 {
                return Err(vec![
                    Diagnostic::error(
                        codes::INVALID_PARAMS,
                        format!("source `{path}` must identify exactly one inventoried file"),
                    )
                    .file(path),
                ]);
            }
            selected.insert(matches[0].storage_path.clone());
        }
    } else {
        selected.extend(inventory.sources.iter().map(|s| s.storage_path.clone()));
    }
    let history = FileHistory::new(root);
    let mut prepared = Prepared {
        inventory: PreparedInventory::default(),
        commands: Vec::new(),
        fixes: Vec::new(),
    };
    for module in &inventory.modules {
        let mut files = Vec::new();
        for source in module
            .sources
            .iter()
            .filter(|s| selected.contains(&s.storage_path))
        {
            let state = history
                .snapshot(&source.storage_path)
                .map_err(|d| vec![d])?
                .ok_or_else(|| failure("source disappeared during analysis"))?;
            let bytes = history.read(&state).map_err(|d| vec![d])?;
            let text = String::from_utf8(bytes).map_err(failure)?;
            prepared
                .inventory
                .entries
                .insert(source.storage_path.clone(), Some(state));
            if let Some(parent) = Path::new(&source.storage_path).parent() {
                let parent = parent.to_string_lossy().replace('\\', "/");
                let children = history.directory_entries(&parent).map_err(|d| vec![d])?;
                prepared.inventory.directories.insert(parent, children);
            }
            files.push(ara_script::project::SourceText {
                path: source.storage_path.clone(),
                text,
            });
        }
        if files.is_empty() {
            continue;
        }
        for change in ara_script::project::code_fixes(root, &files, codes, module.profile)
            .map_err(|d| vec![d])?
        {
            let before = FileState::bytes(change.before.as_bytes());
            if prepared.inventory.entries.get(&change.path) != Some(&Some(before.clone())) {
                return Err(vec![
                    Diagnostic::error(
                        codes::STALE_BASE,
                        "code-fix input identity changed during analysis",
                    )
                    .file(&change.path),
                ]);
            }
            history.put(change.after.as_bytes()).map_err(|d| vec![d])?;
            prepared
                .fixes
                .push(json!({"path":change.path,"fixes":change.fixes}));
            prepared.commands.push(CallSpec {
                cmd: "source.replace".into(),
                args: Some(json!({"path":change.path,"baseHash":before.hash,"text":change.after})),
                symbol: None,
            });
        }
    }
    history
        .revalidate(&prepared.inventory)
        .map_err(|d| vec![d])?;
    Ok(prepared)
}

fn worker(
    work: impl FnOnce() -> Result<Prepared, Vec<Diagnostic>> + Send + 'static,
) -> Result<Receiver<Result<Prepared, Vec<Diagnostic>>>, Vec<Diagnostic>> {
    let (send, ready) = mpsc::channel();
    std::thread::Builder::new()
        .name("ara-source-fix".into())
        .spawn(move || {
            let _ = send.send(work());
        })
        .map_err(failure)?;
    Ok(ready)
}

impl Service for SourceFixService {
    fn name(&self) -> &'static str {
        "source-fixes"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![Method::new::<SourceFixParams>(db, "source.fix", "Apply or preview shipped C# analyzer code fixes as attributed, undoable source.replace commands.").scope(Scope::EditCode).tool("ara_fix")]
    }
    fn call(&mut self, cx: &mut Cx<'_>, _: &str, params: Value) -> Result<Value, Vec<Diagnostic>> {
        let params: SourceFixParams = parse(params)?;
        let root = self.root.clone();
        let paths = params.paths.clone();
        let codes = params.codes.clone().unwrap_or_default();
        if let Some(code) = codes
            .iter()
            .find(|code| !["ARA0001", "ARA0101", "ARA0506"].contains(&code.as_str()))
        {
            return Err(vec![Diagnostic::error(
                codes::INVALID_PARAMS,
                format!(
                    "no shipped code fix for {code}; supported codes: ARA0001, ARA0101, ARA0506"
                ),
            )]);
        }
        let ready = worker(move || prepare(&root, paths.as_deref(), &codes))?;
        let job = cx.start_job("C# analyzer fixes");
        self.pending.push(Pending {
            params,
            revision: cx.store().revision(),
            job: job.clone(),
            ready,
            revalidated: false,
        });
        Ok(json!({"ok":true,"job":job,"preparing":true}))
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        for (job, revision, value) in std::mem::take(&mut self.committed) {
            if let Some(result) = cx.materialization_result(revision) {
                let result = result.clone().map(|()| value).map_err(|d| vec![d]);
                cx.finish_job(&job, result);
            } else {
                self.committed.push((job, revision, value));
            }
        }
        let mut waiting = Vec::new();
        for mut pending in std::mem::take(&mut self.pending) {
            if cx.job_cancelled(&pending.job) {
                continue;
            }
            let result = match pending.ready.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    waiting.push(pending);
                    continue;
                }
                Err(_) => Err(failure("code-fix preparation worker exited")),
            };
            if !pending.revalidated {
                match result {
                    Ok(prepared) => {
                        let root = self.root.clone();
                        match worker(move || {
                            FileHistory::new(root)
                                .revalidate(&prepared.inventory)
                                .map_err(|d| vec![d])?;
                            Ok(prepared)
                        }) {
                            Ok(ready) => {
                                pending.ready = ready;
                                pending.revalidated = true;
                                waiting.push(pending);
                            }
                            Err(errors) => cx.finish_job(&pending.job, Err(errors)),
                        }
                    }
                    Err(errors) => cx.finish_job(&pending.job, Err(errors)),
                }
                continue;
            }
            let result = result.and_then(|prepared| {
                if prepared.commands.is_empty() {
                    return Ok(json!({"ok":true,"changedFiles":0,"commands":[],"fixes":[]}));
                }
                let preview = pending.params.apply == Some(false);
                let commands = preview
                    .then(|| serde_json::to_value(&prepared.commands))
                    .transpose()
                    .map_err(failure)?;
                let params = ApplyParams {
                    session: pending.params.session.clone(),
                    commands: prepared.commands,
                    doc: pending.params.doc,
                    why: pending.params.why,
                    dry_run: Some(preview),
                    base: None,
                    idempotency_key: None,
                    branch: None,
                };
                let mut output = cx.resume_session(&pending.params.session, |cx| {
                    cx.apply_prepared(params, prepared.inventory, pending.revision)
                })??;
                if let Some(output) = output.as_object_mut() {
                    output.insert("changedFiles".into(), prepared.fixes.len().into());
                    output.insert("fixes".into(), prepared.fixes.into());
                    if let Some(commands) = commands {
                        output.insert("commands".into(), commands);
                    }
                }
                Ok(output)
            });
            let revision = result
                .as_ref()
                .ok()
                .filter(|v| v["dryRun"] != true)
                .and_then(|v| v["revision"].as_u64())
                .filter(|rev| *rev > pending.revision);
            match (result, revision) {
                (Ok(value), Some(revision)) => self.committed.push((pending.job, revision, value)),
                (result, _) => cx.finish_job(&pending.job, result),
            }
        }
        waiting.append(&mut self.pending);
        self.pending = waiting;
    }
}
