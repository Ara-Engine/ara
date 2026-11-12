//! Immutable publication snapshots prepared away from the editor's UI thread.
use super::*;
use std::{collections::BTreeSet, sync::Arc, thread::JoinHandle};

/// What the content worker returns. With a renderer's seed the old raster path gets its GPU-ready
/// `raster` content (which carries the frame). Without one the Scene view's own frame comes back
/// directly: the extracted `frame` and the CPU `scene` picking, snapping and bounds read.
pub(crate) struct PreparedScene {
    pub content: FidelityContent,
    pub raster: Option<ara_runtime_render::RasterContent>,
    pub frame: Option<ara_runtime::extract::RenderFrame>,
    pub scene: Option<ara_runtime_render::fidelity::native::Prepared>,
}

/// What the content worker snapshots (the editor's state at the moment the job starts).
pub(crate) struct SnapshotInput<'a> {
    pub view: &'a crate::world::DocumentView,
    pub workspace: &'a ara_doc::Workspace,
    pub schema: &'a ara_schema::SchemaDb,
    pub gesture: Option<&'a crate::world::gesture::Gesture>,
    pub hidden: &'a BTreeSet<String>,
    pub camera: crate::state::Camera,
}

/// What the Scene's frame is extracted from. The frame holds no selection: outlines and gizmos
/// are the editor's overlays, drawn from its own state.
pub(crate) struct FrameInputs<'a> {
    /// The document as the Scene shows it (drafts included).
    pub view: &'a crate::world::DocumentView,
    /// The project's workspace and schema (the store's).
    pub workspace: &'a ara_doc::Workspace,
    pub schema: &'a ara_schema::SchemaDb,
    /// Entity poses while a gizmo drag is live.
    pub poses: Option<&'a BTreeMap<String, glam::Mat4>>,
    /// Entities hidden in the Hierarchy.
    pub hidden: &'a BTreeSet<String>,
    /// The editor's camera: the frame's own camera view.
    pub camera: &'a crate::state::Camera,
}

/// Extracts the Scene's `RenderFrame` from the world: models, shapes, lights, terrain, the
/// environment, and the camera. It holds no renderer and no raster state; `content` is the
/// asset caches it reuses between extractions.
pub(crate) fn extract_frame(
    content: &mut FidelityContent,
    inputs: FrameInputs<'_>,
) -> anyhow::Result<ara_runtime::extract::RenderFrame> {
    let mut frame = content.frame_with_poses(
        inputs.view,
        inputs.workspace,
        inputs.schema,
        inputs.poses,
        inputs.hidden,
    )?;
    frame.camera = camera_view(&inputs.camera.spec());
    Ok(frame)
}

/// The view `camera` (the editor's look-at camera) as the extracted frame carries it; the
/// orthographic and framing requests have none yet.
pub(crate) fn camera_view(
    camera: &ara_frame::scene::CameraSpec,
) -> Option<ara_runtime::extract::CameraView> {
    match camera {
        ara_frame::scene::CameraSpec::LookAt {
            position,
            target,
            vertical_fov,
        } => Some(ara_runtime::extract::CameraView {
            position: position.to_array(),
            forward: (*target - *position).try_normalize()?.to_array(),
            fov: vertical_fov.to_degrees(),
        }),
        _ => None,
    }
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
        input: SnapshotInput<'_>,
        native_seed: Option<ara_render::NativeSeed>,
    ) -> anyhow::Result<JoinHandle<anyhow::Result<PreparedScene>>> {
        let SnapshotInput {
            view,
            workspace,
            schema,
            gesture,
            hidden,
            camera,
        } = input;
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
                let frame = extract_frame(
                    &mut content,
                    FrameInputs {
                        view: &view,
                        workspace: &workspace,
                        schema: &schema,
                        poses: poses.as_ref(),
                        hidden: &hidden,
                        camera: &camera,
                    },
                )?;
                let resolved = content.content(&frame, &workspace, &schema)?;
                match native_seed {
                    Some(seed) => Ok(PreparedScene {
                        content,
                        raster: Some(ara_runtime_render::RasterContent::new_reusing(
                            frame, resolved, seed,
                        )?),
                        frame: None,
                        scene: None,
                    }),
                    None => {
                        let scene = ara_runtime_render::fidelity::native::prepare(
                            &frame,
                            resolved,
                            ara_runtime_render::fidelity::native::NativeConfig {
                                resident_bytes: 1 << 30,
                                upload_bytes: 16 << 20,
                            },
                        )?;
                        Ok(PreparedScene {
                            content,
                            raster: None,
                            frame: Some(frame),
                            scene: Some(scene),
                        })
                    }
                }
            })
            .context("Scene content worker")
    }
}
