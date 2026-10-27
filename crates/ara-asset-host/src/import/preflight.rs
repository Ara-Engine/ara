use super::*;
use super::{planner, prepare};
use std::collections::BTreeMap;

/// Executes the same request in readonly or real preparation mode. A dry run
/// validates source bytes/importer settings/companion scope but deliberately
/// excludes cooking/tool-execution failures, which belong to execution.
pub fn run(
    request: Request,
    context: Context<'_>,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(Progress),
) -> Result<Outcome> {
    if request.dry_run {
        preflight(&request, &context, cancel, progress).map(Outcome::Preflight)
    } else {
        prepare::prepare(request, context, cancel, progress).map(Outcome::Ready)
    }
}

/// Read-only conflict preflight sharing metadata/path/source validation with the
/// real preparation. Never creates staging, cooks, writes or changes an inventory.
pub fn preflight(
    request: &Request,
    context: &Context<'_>,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(Progress),
) -> Result<ImportPlan> {
    let (scan, mut entries, conflicts, total) =
        prepare::inspect(request, context, cancel, progress)?;
    let mut snapshots = BTreeMap::new();
    let mut received = 0;
    for (index, item) in scan.entries.iter().enumerate() {
        cancel.check()?;
        if item.kind == EntryKind::Directory {
            continue;
        }
        let digest = prepare::read_external(item, cancel, |bytes| {
            received += bytes;
            progress(Progress {
                phase: Phase::Inspecting,
                received,
                total,
                entries: index + 1,
                path: Some(item.relative.clone()),
            });
        })?;
        snapshots.insert(item.source.clone(), (item.source.clone(), digest));
    }
    let mut regrouped = false;
    let expanded = loop {
        match prepare::sources(
            &entries,
            &scan,
            &snapshots,
            context,
            cancel,
            prepare::SourceMode { total, cook: false },
            progress,
        )? {
            prepare::Sources::Inspected(bytes) => break bytes,
            prepare::Sources::Complete(..) => {
                return Err(Error::Invalid(
                    "readonly import inspection unexpectedly cooked outputs".into(),
                ));
            }
            prepare::Sources::Regroup(reason) => {
                if request.conflict != ImportConflict::Rename || regrouped {
                    return Err(Error::Invalid(reason));
                }
                let destination = prepare::container(&request.destination, context.inventory)?;
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
    prepare::finalize(&mut entries, &snapshots, context, false);
    let estimated_required_bytes =
        prepare::estimate(total, entries.len(), &entries, context.inventory, expanded)?;
    (context.check_space)(context.staging, estimated_required_bytes)?;
    cancel.check()?;
    progress(Progress {
        phase: Phase::Ready,
        received: total,
        total,
        entries: entries.len(),
        path: None,
    });
    cancel.check()?;
    Ok(ImportPlan {
        token: context.inventory.token.clone(),
        conflicts,
        planned: entries,
        estimated_required_bytes,
    })
}
