//! Immutable publication snapshots prepared away from the editor's UI thread.
use super::*;
use std::{collections::BTreeSet, sync::Arc, thread::JoinHandle};

pub(crate) struct PreparedScene {
    pub content: FidelityContent,
    pub raster: ara_runtime_render::RasterContent,
}

impl FidelityContent {
    /// The generation actually used by the shown extraction. A newer publication
    /// must not prevent value patches to the retained last-good scene.
    pub(crate) fn snapshot_revision(&self) -> u64 {
        self.snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.revision())
    }

    /// Fork small cache maps only at a content boundary. Model/material payloads
    /// and evaluated terrain remain shared immutable generations, so UI patches
    /// can keep using this cache while the worker prepares its replacement.
    pub(crate) fn prepare_async(
        &self,
        view: &crate::world::DocumentView,
        workspace: &ara_doc::Workspace,
        schema: &ara_schema::SchemaDb,
        gesture: Option<&crate::world::gesture::Gesture>,
        hidden: &BTreeSet<String>,
        native_seed: ara_render::NativeSeed,
    ) -> anyhow::Result<JoinHandle<anyhow::Result<PreparedScene>>> {
        ara_gpu::cpu_scope!("SceneContent.snapshot");
        let mut library = ara_runtime::animation::Library::default();
        library.models = self.library.models.clone();
        library.materials = self.library.materials.clone();
        let mut content = Self {
            reader: Arc::clone(&self.reader),
            snapshot: self.snapshot.as_ref().map(Arc::clone),
            library,
            materials: self.materials,
            terrain: self
                .terrain
                .as_ref()
                .map(|(key, data)| (*key, data.clone())),
            mesh_records: self.mesh_records.clone(),
            progress: self.progress.clone(),
            placeholders: self.placeholders.clone(),
        };
        let view = view.with_records(Default::default());
        let workspace = workspace.clone();
        let schema = schema.clone();
        let poses = gesture.map(|gesture| gesture.current.clone());
        let hidden = hidden.clone();
        std::thread::Builder::new()
            .name("scene-content".into())
            .spawn(move || {
                #[cfg(windows)]
                {
                    use windows::Win32::System::Threading::{
                        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
                    };
                    // SAFETY: The pseudo-handle belongs to this worker; changing
                    // its scheduling priority neither borrows nor outlives UI data.
                    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) }
                        .context("Scene worker priority")?;
                }
                ara_gpu::cpu_scope!("SceneContent.prepare");
                let frame = content.frame_with_poses(
                    &view,
                    &workspace,
                    &schema,
                    poses.as_ref(),
                    &hidden,
                )?;
                let resolved = content.content(&frame, &workspace, &schema)?;
                let raster =
                    ara_runtime_render::RasterContent::new_reusing(frame, resolved, native_seed)?;
                Ok(PreparedScene { content, raster })
            })
            .context("Scene content worker")
    }
}
