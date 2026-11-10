//! Parallel read/normalization produces immutable results; only the caller inserts documents.
use super::{Project, asset_path};
use crate::project_settings::read;
use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetIndex, ReadResult, Workspace};
use ara_schema::SchemaDb;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    thread,
};

const MIN_PARALLEL_DOCUMENTS: usize = 16;
const MIN_PARALLEL_BYTES: u64 = 256 * 1024;
const MAX_WORKERS: usize = 4;

struct Input<'a> {
    file: &'a Path,
    relative: String,
    project: bool,
}

pub(super) fn documents(
    project: &Project,
    extra: &[PathBuf],
) -> (Workspace, BTreeMap<String, Vec<Diagnostic>>) {
    let mut workspace = Workspace::new(project.index.clone());
    if project.authored_root != project.root {
        workspace.set_reference_root(crate::project_layout::AUTHORING_ROOT);
    }
    let mut seen = HashSet::new();
    let inputs = project.docs.iter().chain(extra).filter_map(|file| {
        let relative = project.rel(file);
        seen.insert(relative.clone()).then(|| Input {
            file,
            relative,
            project: project.project_files.contains(file),
        })
    });
    let mut diagnostics = BTreeMap::new();
    if project.docs.len().saturating_add(extra.len()) < MIN_PARALLEL_DOCUMENTS {
        for input in inputs {
            let result = read_input(&input, &project.db, &project.index);
            insert(input, result, &mut workspace, &mut diagnostics);
        }
    } else {
        let inputs: Vec<_> = inputs.collect();
        let workers = worker_count(&inputs);
        if workers == 1 {
            for input in inputs {
                let result = read_input(&input, &project.db, &project.index);
                insert(input, result, &mut workspace, &mut diagnostics);
            }
        } else {
            let results = read_parallel(&inputs, &project.db, &project.index, workers);
            for (input, result) in inputs.into_iter().zip(results) {
                insert(input, result, &mut workspace, &mut diagnostics);
            }
        }
    }
    (workspace, diagnostics)
}

fn worker_count(inputs: &[Input<'_>]) -> usize {
    if inputs.len() < MIN_PARALLEL_DOCUMENTS {
        return 1;
    }
    let available = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(MAX_WORKERS);
    if available == 1 {
        return 1;
    }
    // Sizes only choose scheduling. Every file is still read freshly, and failed
    // metadata reads remain normal reader errors rather than skipped inputs.
    let mut bytes = 0_u64;
    for input in inputs {
        bytes = bytes.saturating_add(fs::metadata(input.file).map_or(0, |metadata| metadata.len()));
        if bytes >= MIN_PARALLEL_BYTES {
            return available;
        }
    }
    1
}

fn read_input(input: &Input<'_>, db: &SchemaDb, index: &AssetIndex) -> ReadResult {
    match fs::read_to_string(input.file) {
        Ok(text) => read(&text, &input.relative, db, Some(index)),
        Err(error) => ReadResult {
            doc: None,
            diags: vec![
                Diagnostic::error(codes::IO, format!("cannot read: {error}")).file(&input.relative),
            ],
        },
    }
}

fn read_parallel(
    inputs: &[Input<'_>],
    db: &SchemaDb,
    index: &AssetIndex,
    workers: usize,
) -> Vec<ReadResult> {
    thread::scope(|scope| {
        let batches: Vec<_> = inputs
            .chunks(inputs.len().div_ceil(workers))
            .map(|chunk| {
                thread::Builder::new()
                    .spawn_scoped(scope, move || read_chunk(chunk, db, index))
                    .map_err(|_| read_chunk(chunk, db, index))
            })
            .collect();
        batches
            .into_iter()
            .flat_map(|batch| match batch {
                Ok(worker) => worker
                    .join()
                    .unwrap_or_else(|error| std::panic::resume_unwind(error)),
                // Thread pressure must not prevent opening an otherwise valid project.
                Err(results) => results,
            })
            .collect()
    })
}

fn read_chunk(inputs: &[Input<'_>], db: &SchemaDb, index: &AssetIndex) -> Vec<ReadResult> {
    inputs
        .iter()
        .map(|input| read_input(input, db, index))
        .collect()
}

fn insert(
    input: Input<'_>,
    result: ReadResult,
    workspace: &mut Workspace,
    diagnostics: &mut BTreeMap<String, Vec<Diagnostic>>,
) {
    if let Some(doc) = result.doc {
        if input.project {
            workspace.insert_project_document(input.relative.clone(), doc);
        } else {
            let source = asset_path(&input.relative, &doc.kind);
            workspace.insert_with_asset_path(input.relative.clone(), doc, source);
        }
    }
    diagnostics.insert(input.relative, result.diags);
}

#[cfg(test)]
mod tests;
