//! Preserve native BEGIN-to-COMPLETE batches; legacy events group within one frame.
use super::*;
use crate::shell::asset_import::Request;
use ara_host::project::{AuthoredFile, AuthoredRole, AuthoredScope};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
mod native;

/// Authoritative storage-to-authored mapping and already-known path conflicts.
pub(super) struct Destination {
    /// Destination understood by the typed asset.import command.
    pub authored: String,
    /// Existing paths from the backend's validation/planning seam, never guessed here.
    pub conflicts: Vec<String>,
}

/// Resolve from the worker's semantic index without project IO on the event thread.
pub(super) fn destination(
    storage: &str,
    files: &[AuthoredFile],
    reference_root: &str,
) -> ara_ui::Result<Destination> {
    if storage.is_empty() {
        return Ok(Destination {
            authored: String::new(),
            conflicts: Vec::new(),
        });
    }
    if let Some(folder) = files.iter().find(|file| file.storage_path == storage) {
        if !matches!(folder.scope, AuthoredScope::Authored)
            || !matches!(folder.role, AuthoredRole::Folder)
        {
            return Err("Choose a folder in authored content to import these files".into());
        }
        return Ok(Destination {
            authored: folder.path.clone(),
            conflicts: Vec::new(),
        });
    }
    if !reference_root.is_empty() && storage == reference_root {
        return Ok(Destination {
            authored: String::new(),
            conflicts: Vec::new(),
        });
    }
    Err("The dropped-on folder is not present in the current project index".into())
}
#[derive(Default)]
struct Batch {
    sources: Vec<PathBuf>,
    seen: BTreeSet<PathBuf>,
}
impl Batch {
    fn push(&mut self, source: PathBuf) {
        if self.seen.insert(source.clone()) {
            self.sources.push(source);
        }
    }
}
struct Stream {
    id: u64,
    window: u32,
    files: Batch,
    positions: BTreeMap<u32, [f32; 2]>,
    current: Option<(u32, [f32; 2])>,
    cancelled: bool,
}
/// One persistent native drag and independent compatibility/file-chooser batches.
#[derive(Default)]
pub(super) struct ImportDrops {
    batches: BTreeMap<String, Batch>,
    stream: Option<Stream>,
    next: u64,
}
impl ImportDrops {
    /// Resolve against the same worker snapshot as the visible browser, then queue
    /// each target once without holding an immutable Shell borrow across mutation.
    pub fn flush_authored(&mut self, shell: &mut Shell) -> ara_ui::Result<usize> {
        let mut resolved: BTreeMap<_, _> = self
            .batches
            .keys()
            .map(|storage| {
                (
                    storage.clone(),
                    destination(
                        storage,
                        shell.asset_authored_files(),
                        shell.asset_reference_root(),
                    ),
                )
            })
            .collect();
        self.flush(shell, |storage, _| match resolved.remove(storage) {
            Some(destination) => destination,
            None => Err("Import destination lost its indexed resolution".into()),
        })
    }
    /// Queue a resolved browser-storage target, preserving source order within it.
    pub fn push(&mut self, source: PathBuf, storage_folder: String) {
        let batch = self.batches.entry(storage_folder).or_default();
        batch.push(source);
    }
    /// Convert the tagged event using its own surface scale before hit-testing folders.
    #[allow(
        clippy::too_many_arguments,
        reason = "Drop event and native surface owners are independent borrows"
    )]
    pub fn native(
        &mut self,
        source: PathBuf,
        position: [f32; 2],
        sdl: u32,
        editor: &Editor,
        shell: &mut Shell,
        main: &Window,
        dpi: f32,
        floats: &windows::Floats,
    ) -> ara_ui::Result<()> {
        if self.stream.is_some() {
            return self.native_file(source, position, sdl, editor, shell, main, dpi, floats);
        }
        if !position.iter().all(|value| value.is_finite()) {
            return Err("File drop position is non-finite".into());
        }
        let scale = if sdl == 0 || sdl == main.id() {
            shell.leave();
            crate::native::points_to_logical(main.raw(), dpi)
        } else {
            let float = floats
                .windows
                .iter()
                .find(|float| float.sdl == sdl)
                .ok_or("The file drop window was closed")?;
            if !shell.enter(&float.id) {
                return Err("The file drop surface was closed".into());
            }
            crate::native::points_to_logical(
                float.window.raw(),
                float.window.display_scale() * windows::ui_scale(editor),
            )
        };
        if !scale.is_finite() || scale <= 0. {
            shell.leave();
            return Err("File drop surface scale is invalid".into());
        }
        let folder =
            shell.asset_import_storage_folder_at(editor, position.map(|value| value * scale));
        shell.leave();
        self.push(source, folder);
        Ok(())
    }
    /// Resolve each group through the backend contract, then queue exactly once.
    /// Invalid groups report their original error while independent groups proceed.
    pub fn flush(
        &mut self,
        shell: &mut Shell,
        mut resolve: impl FnMut(&str, &[PathBuf]) -> ara_ui::Result<Destination>,
    ) -> ara_ui::Result<usize> {
        let mut queued = 0;
        let mut error = None;
        for (storage, batch) in std::mem::take(&mut self.batches) {
            let result = resolve(&storage, &batch.sources).and_then(|destination| {
                shell.queue_asset_import(Request {
                    sources: batch.sources,
                    destination: destination.authored,
                    conflicts: destination.conflicts,
                })
            });
            match result {
                Ok(()) => queued += 1,
                Err(failed) if error.is_none() => error = Some(failed),
                Err(_) => {}
            }
        }
        match error {
            Some(error) => Err(error),
            None => Ok(queued),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_frame_groups_by_actual_target_and_keeps_each_os_source_once() {
        let first = std::env::temp_dir().join("first.glb");
        let second = std::env::temp_dir().join("second.png");
        let mut drops = ImportDrops::default();
        drops.push(first.clone(), "Some Storage/Props".into());
        drops.push(first.clone(), "Some Storage/Props".into());
        drops.push(second.clone(), "Some Storage/Props".into());
        drops.push(first.clone(), "Other Storage/Textures".into());
        assert_eq!(drops.batches.len(), 2);
        assert_eq!(
            drops.batches["Some Storage/Props"].sources,
            [first.clone(), second]
        );
        assert_eq!(drops.batches["Other Storage/Textures"].sources, [first]);
    }
    #[test]
    fn only_the_default_authored_root_resolves_without_a_cached_folder() {
        assert_eq!(
            destination("", &[], "")
                .map(|target| target.authored)
                .ok()
                .as_deref(),
            Some("")
        );
        assert_eq!(
            destination(ara_doc::AUTHORING_ROOT, &[], ara_doc::AUTHORING_ROOT)
                .map(|target| target.authored)
                .ok()
                .as_deref(),
            Some("")
        );
        assert!(
            destination(ara_doc::AUTHORING_ROOT, &[], "").is_err(),
            "A legacy folder named like the modern root must wait for its actual indexed mapping"
        );
        assert!(destination("Assets/Unindexed", &[], "").is_err());
        assert!(destination("../Elsewhere", &[], ara_doc::AUTHORING_ROOT).is_err());
    }
    #[test]
    fn a_legacy_folder_named_like_the_modern_root_keeps_its_logical_destination() {
        let files = [AuthoredFile {
            path: ara_doc::AUTHORING_ROOT.into(),
            storage_path: ara_doc::AUTHORING_ROOT.into(),
            scope: AuthoredScope::Authored,
            role: AuthoredRole::Folder,
        }];
        assert_eq!(
            destination(ara_doc::AUTHORING_ROOT, &files, "")
                .map(|target| target.authored)
                .ok()
                .as_deref(),
            Some(ara_doc::AUTHORING_ROOT)
        );
    }
}
