//! Read-only URI validation and disposable-worker cooking for one fixed plan.
use super::*;

pub(in crate::import) enum Sources {
    Complete(Vec<PreparedSource>, u64),
    Inspected(u64),
    Regroup(String),
}
pub(in crate::import) struct SourceMode {
    pub total: u64,
    pub cook: bool,
}
pub(in crate::import) fn sources(
    entries: &[Entry],
    scan: &Scan,
    staged: &BTreeMap<PathBuf, (PathBuf, Digest)>,
    context: &Context<'_>,
    cancel: &Cancellation,
    mode: SourceMode,
    progress: &mut dyn FnMut(Progress),
) -> Result<Sources> {
    let SourceMode { total, cook } = mode;
    let mut plan = entries.to_vec();
    for entry in &mut plan {
        entry.native_document = entry.destination.ends_with(".import.json")
            || context.importers.native_document(&entry.destination);
        if let Some(source) = &entry.source
            && let Some((path, digest)) = staged.get(source)
        {
            entry.digest = Some(*digest);
            if cook {
                entry.staged = Some(path.clone());
            }
        }
    }
    let prepared_resolver =
        context
            .importers
            .prepare_plan(&plan, &|entry| read_entry(entry, cancel), cancel)?;
    let resolver = prepared_resolver.as_deref().unwrap_or(context.importers);
    let mut result = Vec::new();
    let mut footprint = 0u64;
    for entry in entries {
        cancel.check()?;
        if entry.kind != EntryKind::File
            || entry.disposition == Disposition::Skipped
            || entry.destination.ends_with(".import.json")
        {
            continue;
        }
        let original = entry
            .source
            .as_ref()
            .ok_or_else(|| Error::Invalid("prepared file has no external source".into()))?;
        let mut sidecar = original.as_os_str().to_os_string();
        sidecar.push(".import.json");
        let sidecar = PathBuf::from(sidecar);
        let foreign_sidecar = staged
            .get(&sidecar)
            .map(|_| read(staged, &sidecar, cancel))
            .transpose()?;
        let existing = context.inventory.entries.get(&entry.destination);
        let binding = if entry.disposition == Disposition::Replaced {
            existing.and_then(|entry| entry.binding.clone())
        } else {
            None
        };
        let Some(mut binding) = (match binding {
            Some(binding) => {
                let rebound = resolver.rebind(&binding)?;
                if rebound.id != binding.id
                    || rebound.importer != binding.importer
                    || rebound.settings != binding.settings
                    || rebound.preset != binding.preset
                    || rebound.target != binding.target
                {
                    return Err(Error::Invalid("Replace resolver changed destination identity/author settings; retry with a binding that preserves them".into()));
                }
                Some(rebound)
            }
            None => resolver.source(&entry.destination, foreign_sidecar.as_deref())?,
        }) else {
            if foreign_sidecar.is_some() {
                return Err(Error::Invalid(format!(
                    "foreign sidecar for {} has no supported importer binding; install its importer or remove the unrelated sidecar before importing",
                    entry.destination
                )));
            }
            continue;
        };
        if binding.preset.is_none() {
            return Err(Error::Invalid(format!(
                "source binding for {} has no resolved project import preset",
                entry.destination
            )));
        }
        binding.path = entry.destination.clone();
        let source = read(staged, original, cancel)?;
        progress(Progress {
            phase: Phase::Discovering,
            received: total,
            total,
            entries: result.len(),
            path: Some(entry.destination.clone()),
        });
        cancel.check()?;
        let mut dependencies = BTreeMap::new();
        let mut snapshot_bytes = source.len() as u64;
        let discovered = context.worker.discover(&binding, source.clone(), cancel)?;
        if discovered.len() > context.limits.entries {
            return Err(Error::Invalid(
                "source companion count exceeds the import entry budget".into(),
            ));
        }
        for uri in discovered {
            cancel.check()?;
            let original_parent = original
                .parent()
                .ok_or_else(|| Error::Invalid("external source has no parent".into()))?;
            let input = external::authorized_file(original_parent, &uri, &scan.authorities)?;
            let companion = entries
                .iter()
                .find(|entry| entry.source.as_ref() == Some(&input))
                .ok_or_else(|| {
                    Error::Invalid(format!(
                        "companion `{uri}` was ignored or not selected; include its authored folder"
                    ))
                })?;
            let expected = resolve(
                entry
                    .destination
                    .rsplit_once('/')
                    .map_or("", |(parent, _)| parent),
                &uri,
            )?;
            if !same_path(
                &expected,
                &companion.destination,
                context.inventory.case_insensitive,
            ) {
                return Ok(Sources::Regroup(format!(
                    "conflict naming changes the relative URI `{uri}` for {}; drop a common containing folder or choose a conflict policy preserving that companion path",
                    entry.destination
                )));
            }
            let bytes = read(staged, &input, cancel)?;
            snapshot_bytes = snapshot_bytes
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| Error::Invalid("source snapshot byte range overflow".into()))?;
            if snapshot_bytes > context.limits.bytes {
                return Err(Error::Invalid(
                    "source companions exceed the import byte budget".into(),
                ));
            }
            if companion.disposition == Disposition::Skipped
                && context
                    .inventory
                    .entries
                    .get(&companion.destination)
                    .and_then(|entry| entry.digest)
                    != Some(Digest::of(&bytes))
            {
                return Err(Error::Invalid(format!(
                    "skipped companion `{uri}` differs from the destination bytes; choose Replace or Rename the containing folder"
                )));
            }
            dependencies.insert(uri, bytes);
        }
        let input = ImportInput {
            preset: binding.preset.expect("resolved preset checked above"),
            thread_budget: binding.thread_budget,
            source_path: entry.destination.clone(),
            source,
            importer: binding.importer.clone(),
            version: binding.version.clone(),
            settings: binding.settings.clone(),
            target: binding.target,
            dependencies,
            asset_dependencies: binding.asset_dependencies.clone(),
        };
        let input_bytes = snapshot_bytes;
        footprint = footprint
            .checked_add(input_bytes)
            .ok_or_else(|| Error::Invalid("import snapshot budget overflow".into()))?;
        if footprint > context.limits.bytes {
            return Err(Error::Invalid("expanded source/dependency snapshots exceed the import byte budget; import fewer models at once".into()));
        }
        (context.check_space)(
            context.staging,
            estimate(total, entries.len(), entries, context.inventory, footprint)?,
        )?;
        if !cook {
            continue;
        }
        progress(Progress {
            phase: Phase::Cooking,
            received: total,
            total,
            entries: result.len(),
            path: Some(entry.destination.clone()),
        });
        let output = context.worker.cook(input.clone(), cancel, &mut |_| {
            progress(Progress {
                phase: Phase::Cooking,
                received: total,
                total,
                entries: result.len(),
                path: Some(entry.destination.clone()),
            })
        })?;
        output.validate()?;
        footprint = output
            .artifacts
            .iter()
            .try_fold(footprint, |sum, artifact| {
                sum.checked_add(artifact.bytes.len() as u64)
                    .ok_or_else(|| Error::Invalid("import artifact budget overflow".into()))
            })?;
        if footprint > context.limits.bytes {
            return Err(Error::Invalid(
                "cooked outputs exceed the import byte budget; import fewer models at once".into(),
            ));
        }
        (context.check_space)(
            context.staging,
            estimate(
                total,
                scan.entries.len(),
                entries,
                context.inventory,
                footprint,
            )?,
        )?;
        result.push(PreparedSource {
            path: entry.destination.clone(),
            binding,
            input,
            output,
            foreign_sidecar,
        });
    }
    Ok(if cook {
        Sources::Complete(result, footprint)
    } else {
        Sources::Inspected(footprint)
    })
}

fn resolve(parent: &str, uri: &str) -> Result<String> {
    if uri.is_empty() || uri.contains([':', '\\', '\0']) || Path::new(uri).is_absolute() {
        return Err(Error::Invalid(format!(
            "companion URI `{uri}` must be local and relative"
        )));
    }
    let mut parts: Vec<_> = if parent.is_empty() {
        Vec::new()
    } else {
        parent.split('/').map(str::to_owned).collect()
    };
    for part in Path::new(uri).components() {
        match part {
            std::path::Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| Error::Invalid("companion names must be UTF-8".into()))?
                    .to_owned(),
            ),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if !parts.is_empty() => {
                parts.pop();
            }
            _ => {
                return Err(Error::Invalid(format!(
                    "companion URI `{uri}` escapes the destination authored root"
                )));
            }
        }
    }
    let path = parts.join("/");
    external::relative(&path, false)?;
    Ok(path)
}
