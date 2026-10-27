mod reading;
mod sources;
pub use reading::read_entry;
pub(super) use reading::read_external;
use reading::{copy, read};
pub(super) use sources::{SourceMode, Sources, sources};

use super::external::{self, Scan};
use super::*;
use std::{collections::BTreeMap, fs, time::SystemTime};

pub(super) struct Staging {
    root: Option<PathBuf>,
    seal: Vec<u8>,
}
impl Staging {
    fn new(path: &Path, authored: &Path, project: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error::Invalid(
                "import staging must be an absolute fresh derived-data path".into(),
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| Error::Invalid("import staging has no parent".into()))?;
        external::no_links(parent)?;
        let parent = parent.canonicalize()?;
        let path = parent
            .join(path.file_name().ok_or_else(|| {
                Error::Invalid("import staging must be a named directory".into())
            })?);
        if path.starts_with(authored)
            && !(authored == project
                && path.starts_with(project.join(".ara"))
                && path != project.join(".ara"))
        {
            return Err(Error::Invalid(
                "import staging must stay outside authored data".into(),
            ));
        }
        fs::create_dir(&path)?;
        let seal = format!(
            "ara-import-stage:{}:{:?}",
            std::process::id(),
            SystemTime::now()
        )
        .into_bytes();
        if let Err(error) = fs::write(path.join(".ara-import-owner"), &seal) {
            let _ = fs::remove_dir(&path);
            return Err(error.into());
        }
        Ok(Self {
            root: Some(path.canonicalize()?),
            seal,
        })
    }
    fn root(&self) -> &Path {
        self.root
            .as_deref()
            .expect("staging is owned until discard")
    }
    pub(super) fn discard(&mut self) -> Result<()> {
        let Some(root) = self.root.as_ref() else {
            return Ok(());
        };
        let metadata = fs::symlink_metadata(root)?;
        if external::linked(&metadata)
            || root.canonicalize()? != *root
            || fs::read(root.join(".ara-import-owner"))? != self.seal
        {
            return Err(Error::Invalid(format!(
                "import staging ownership changed; refusing cleanup of {}",
                root.display()
            )));
        }
        // This exact absolute directory was created fresh by this preparation,
        // and its canonical identity/ownership seal were checked immediately above.
        fs::remove_dir_all(root)?;
        self.root = None;
        Ok(())
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = self.discard();
    }
}

/// Prepares one file drop on a background/CLI worker, never the command/frame thread.
/// Cancellation removes only newly owned staging; destination files remain unchanged.
pub fn prepare(
    request: Request,
    context: Context<'_>,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(Progress),
) -> Result<PreparedImport> {
    if request.dry_run {
        return Err(Error::Invalid(
            "dryRun imports must use the read-only run/preflight entry point".into(),
        ));
    }
    let (scan, mut entries, conflicts, total) = inspect(&request, &context, cancel, progress)?;
    let authored = context.authored_root.canonicalize()?;
    let project = context.project_root.canonicalize()?;
    let mut staging = Staging::new(context.staging, &authored, &project)?;
    let result = (|| {
        let mut staged = BTreeMap::new();
        let mut received = 0;
        for (index, item) in scan.entries.iter().enumerate() {
            cancel.check()?;
            if item.kind == EntryKind::Directory {
                continue;
            }
            let target = staging.root().join(format!("{index:08}.blob"));
            let digest = copy(item, &target, cancel, |amount| {
                received += amount;
                progress(Progress {
                    phase: Phase::Staging,
                    received,
                    total,
                    entries: index + 1,
                    path: Some(item.relative.clone()),
                });
            })?;
            staged.insert(item.source.clone(), (target, digest));
        }
        let mut regrouped = false;
        let (sources, footprint) = loop {
            match sources(
                &entries,
                &scan,
                &staged,
                &context,
                cancel,
                SourceMode { total, cook: true },
                progress,
            )? {
                Sources::Complete(sources, footprint) => break (sources, footprint),
                Sources::Inspected(_) => {
                    return Err(Error::Invalid(
                        "source validation returned no cooked preparation".into(),
                    ));
                }
                Sources::Regroup(reason) => {
                    if request.conflict != ImportConflict::Rename || regrouped {
                        return Err(Error::Invalid(reason));
                    }
                    // Preserve unchanged binary-model URI relationships as one
                    // newly named selection, rather than renaming a companion
                    // independently and silently breaking its consumer.
                    let destination = container(&request.destination, context.inventory)?;
                    entries = planner::plan(
                        &scan.entries,
                        &destination,
                        ImportConflict::Rename,
                        context.inventory,
                    )?;
                    if entries.len() > context.limits.entries {
                        return Err(Error::Invalid(
                            "import destination parents exceed the import entry budget".into(),
                        ));
                    }
                    for entry in &mut entries {
                        if entry.disposition == Disposition::Created {
                            entry.disposition = Disposition::Renamed;
                        }
                    }
                    regrouped = true;
                }
            }
        };
        finalize(&mut entries, &staged, &context, true);
        cancel.check()?;
        progress(Progress {
            phase: Phase::Ready,
            received: total,
            total,
            entries: entries.len(),
            path: None,
        });
        cancel.check()?;
        Ok((entries, sources, footprint))
    })();
    match result {
        Ok((entries, sources, footprint)) => Ok(PreparedImport {
            token: context.inventory.token.clone(),
            conflicts,
            estimated_required_bytes: estimate(
                total,
                entries.len(),
                &entries,
                context.inventory,
                footprint,
            )?,
            entries,
            sources,
            staging,
        }),
        Err(error) => {
            staging.discard()?;
            Err(error)
        }
    }
}

pub(super) fn finalize(
    entries: &mut [Entry],
    snapshots: &BTreeMap<PathBuf, (PathBuf, Digest)>,
    context: &Context<'_>,
    include_staged: bool,
) {
    let retained: std::collections::BTreeSet<_> = entries
        .iter()
        .filter(|entry| {
            entry.disposition == Disposition::Replaced
                && context
                    .inventory
                    .entries
                    .get(&entry.destination)
                    .is_some_and(|old| old.binding.is_some())
        })
        .map(|entry| format!("{}.import.json", entry.destination))
        .collect();
    for entry in entries {
        entry.native_document = entry.destination.ends_with(".import.json")
            || context.importers.native_document(&entry.destination);
        if let Some(source) = &entry.source
            && let Some((path, digest)) = snapshots.get(source)
        {
            entry.digest = Some(*digest);
            if include_staged {
                entry.staged = Some(path.clone());
            }
        }
        if retained.contains(&entry.destination) {
            entry.disposition = Disposition::Skipped;
        }
    }
}

pub(super) fn inspect(
    request: &Request,
    context: &Context<'_>,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(Progress),
) -> Result<(Scan, Vec<Entry>, Vec<Conflict>, u64)> {
    cancel.check()?;
    external::no_links(context.authored_root)?;
    external::no_links(context.project_root)?;
    let authored = context.authored_root.canonicalize()?;
    let project = context.project_root.canonicalize()?;
    if !authored.starts_with(&project) {
        return Err(Error::Invalid(
            "authored root is outside its project".into(),
        ));
    }
    if !authored.is_dir() {
        return Err(Error::Invalid("authored root is not a directory".into()));
    }
    if !context.staging.is_absolute() {
        return Err(Error::Invalid(
            "import staging must be an absolute fresh derived-data path".into(),
        ));
    }
    let parent = context
        .staging
        .parent()
        .ok_or_else(|| Error::Invalid("import staging has no parent".into()))?;
    external::no_links(parent)?;
    let candidate = parent.canonicalize()?.join(
        context
            .staging
            .file_name()
            .ok_or_else(|| Error::Invalid("import staging must be a named directory".into()))?,
    );
    if candidate.starts_with(&authored)
        && !(authored == project
            && candidate.starts_with(project.join(".ara"))
            && candidate != project.join(".ara"))
    {
        return Err(Error::Invalid("import staging must stay outside authored data or inside the legacy project's protected .ara derived root".into()));
    }
    match fs::symlink_metadata(&candidate) {
        Ok(_) => {
            return Err(Error::Invalid(
                "import staging directory already exists; retry with a fresh job".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    external::relative(&request.destination, true)?;
    progress(Progress {
        phase: Phase::Inspecting,
        received: 0,
        total: 0,
        entries: 0,
        path: None,
    });
    cancel.check()?;
    let scan = external::scan(
        &request.sources,
        cancel,
        context.limits.entries,
        |entries| {
            progress(Progress {
                phase: Phase::Inspecting,
                received: 0,
                total: 0,
                entries,
                path: None,
            })
        },
    )?;
    let total = scan.entries.iter().try_fold(0u64, |sum, entry| {
        sum.checked_add(entry.bytes)
            .ok_or_else(|| Error::Invalid("import size exceeds the supported byte range".into()))
    })?;
    if total > context.limits.bytes {
        return Err(Error::Invalid(format!(
            "import requires {total} bytes, exceeding the {} byte budget; import a smaller selection",
            context.limits.bytes
        )));
    }
    let (entries, conflicts) = planner::plan_with_conflicts(
        &scan.entries,
        &request.destination,
        request.conflict,
        context.inventory,
    )?;
    for entry in &entries {
        let destination = &entry.destination;
        let insensitive = context.inventory.case_insensitive;
        if ["ara.project.json", "AGENTS.md", "thumbnail.png"]
            .iter()
            .any(|name| same_path(destination, name, insensitive))
            || authored == project
                && [
                    "settings",
                    "packages",
                    "builds",
                    "target",
                    "node_modules",
                    "spikes",
                    "evidence",
                    "tests",
                ]
                .iter()
                .any(|root| {
                    same_path(
                        destination.split('/').next().unwrap_or(destination),
                        root,
                        insensitive,
                    )
                })
        {
            return Err(Error::Invalid(format!(
                "import destination `{destination}` is project metadata/policy or a protected legacy root; migrate the project or choose an authored folder"
            )));
        }
    }
    if entries.len() > context.limits.entries {
        return Err(Error::Invalid(
            "import destination parents exceed the import entry budget".into(),
        ));
    }
    (context.check_space)(
        context.staging,
        estimate(total, scan.entries.len(), &entries, context.inventory, 0)?,
    )?;
    cancel.check()?;
    Ok((scan, entries, conflicts, total))
}

pub(super) fn estimate(
    total: u64,
    entries: usize,
    plan: &[Entry],
    inventory: &DestinationInventory,
    cooked: u64,
) -> Result<u64> {
    let replaced = plan
        .iter()
        .filter(|entry| entry.disposition == Disposition::Replaced)
        .try_fold(0u64, |sum, entry| {
            sum.checked_add(
                inventory
                    .entries
                    .get(&entry.destination)
                    .map_or(0, |entry| entry.bytes),
            )
            .ok_or_else(|| Error::Invalid("replacement snapshot size estimate overflow".into()))
        })?;
    total
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(replaced))
        .and_then(|bytes| bytes.checked_add(cooked))
        .and_then(|bytes| bytes.checked_add((entries as u64).saturating_mul(4096)))
        .ok_or_else(|| Error::Invalid("import peak disk-space estimate overflow".into()))
}

fn same_path(a: &str, b: &str, insensitive: bool) -> bool {
    ara_doc::file_state::path_key_with_case(a, insensitive)
        == ara_doc::file_state::path_key_with_case(b, insensitive)
}
pub(super) fn container(destination: &str, inventory: &DestinationInventory) -> Result<String> {
    let requested = planner::join(destination, "Imported");
    for number in 1..=inventory.entries.len().saturating_add(2) {
        let path = if number == 1 {
            requested.clone()
        } else {
            planner::suffixed(&requested, number, EntryKind::Directory)
        };
        if !inventory.entries.keys().any(|existing| {
            same_path(existing, &path, inventory.case_insensitive)
                || planner::inside(
                    &ara_doc::file_state::path_key_with_case(existing, inventory.case_insensitive),
                    &ara_doc::file_state::path_key_with_case(&path, inventory.case_insensitive),
                )
        }) {
            return Ok(path);
        }
    }
    Err(Error::Invalid(
        "cannot allocate a conflict-free import container".into(),
    ))
}

pub(super) fn verify(prepared: &PreparedImport, cancel: &Cancellation) -> Result<()> {
    cancel.check()?;
    for entry in &prepared.entries {
        cancel.check()?;
        if let Some(path) = &entry.staged {
            external::no_links(path)?;
            if !path.starts_with(prepared.staging.root())
                || Digest::of(&fs::read(path)?)
                    != entry
                        .digest
                        .ok_or_else(|| Error::Invalid("staged file has no digest".into()))?
            {
                return Err(Error::Invalid(
                    "staged import changed before commit; retry".into(),
                ));
            }
        }
    }
    cancel.check()
}
