//! Collect one native event batch into one typed import request per drop folder.
use super::*;
use crate::shell::asset_import::Request;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

/// Authoritative storage-to-authored mapping and already-known path conflicts.
pub(super) struct Destination {
    /// Destination understood by the typed asset.import command.
    pub authored: String,
    /// Existing paths from the backend's validation/planning seam, never guessed here.
    pub conflicts: Vec<String>,
}
#[derive(Default)]
struct Batch {
    sources: Vec<PathBuf>,
    seen: BTreeSet<PathBuf>,
}
/// Frame-local OS drops grouped by the browser storage folder under each cursor.
#[derive(Default)]
pub(super) struct ImportDrops {
    batches: BTreeMap<String, Batch>,
}
impl ImportDrops {
    /// Queue a resolved browser-storage target, preserving source order within it.
    pub fn push(&mut self, source: PathBuf, storage_folder: String) {
        let batch = self.batches.entry(storage_folder).or_default();
        if batch.seen.insert(source.clone()) {
            batch.sources.push(source);
        }
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
}
