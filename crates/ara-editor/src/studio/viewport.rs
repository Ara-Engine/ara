//! The Scene view: the frame the editor extracts from the world (`scene_frame`), drawn by
//! Renderlab (`renderlab`), with the CPU scene picking, snapping and bounds read. There is no
//! other renderer: if Renderlab cannot draw, `renderlab_error` is what the panel shows.
use crate::{Editor, gpu::NativeUi, overlay};
use ara_frame::capture::{CaptureRequest, ViewMode};
use ara_runtime_render::{PresentationImage, fidelity::native::Prepared};

use super::device::SceneDevice;

mod bounds;
mod capture;
mod content;
mod ids;
mod picking;
mod renderlab;
mod scene_frame;
pub(super) use ids::IdHost;

/// Binds the Scene view's picture and its pick buffer as the UI adapter's external images:
/// Renderlab's image and the packed ids rasterised beside it.
fn attach(
    gpu: &mut NativeUi,
    device: &SceneDevice,
    (color, ids): (ara_gpu::Image, ara_gpu::Image),
) -> anyhow::Result<()> {
    // SAFETY: Both views belong to the shared live device, stay valid through UI submission,
    // and finish CopySource.
    unsafe {
        gpu.external_image(
            overlay::viewport_image(),
            device.gpu().native_image(color)?,
            device.gpu().image_view(color)?,
        )?;
        gpu.external_image(
            overlay::ENTITY_IMAGE,
            device.gpu().native_image(ids)?,
            device.gpu().image_view(ids)?,
        )?;
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct SceneView {
    /// The live gizmo drag offset for the next frames, set by the main loop.
    pub(crate) gizmo_offset: glam::Mat4,
    /// The extracted frame on screen, and the CPU scene picking, snapping and bounds read
    /// (`scene_frame`).
    frame: Option<ara_runtime::extract::RenderFrame>,
    prepared: Option<Prepared>,
    /// The document the frame and the CPU scene belong to.
    scene_doc: Option<String>,
    content: Option<crate::world::gltf::FidelityContent>,
    /// The `Editor::paint_revision` the frame shows, and whether a preview is applied.
    paint_revision: u64,
    paint_applied: bool,
    /// The still the window capture routes read: Renderlab's image.
    published: Option<PresentationImage>,
    /// The Scene's frame drawn by Renderlab (`renderlab`).
    renderlab: Option<renderlab::RenderlabScene>,
    /// Renderlab failed to start or draw: the Scene panel shows why.
    renderlab_error: Option<String>,
    /// What the Renderlab image last drawn was drawn from (`scene_frame`).
    scene_drawn: Option<scene_frame::Drawn>,
    /// Passes Renderlab still draws after the last change (`scene_frame`).
    scene_settle: u32,
    /// The Scene has drawn a frame the compositor can show.
    active: bool,
    revision: Option<(u64, u64, u64, u64)>,
    /// Transient typed drafts, projected through the same native extraction as commits.
    preview_view: Option<crate::world::DocumentView>,
    /// Owner of the transient projection, independent of content still loading.
    preview_scene: String,
    /// Pose already incorporated by the extracted frame (no repeated extraction).
    patched_pose: Option<u64>,
    /// One CPU-only extraction at a time; newer input coalesces until it finishes.
    content_job: Option<content::Job>,
    content_ready: Option<content::Publication>,
    content_epoch: u64,
    /// Input actually reflected by the frame on screen.
    shown_input: Option<content::Key>,
}

impl SceneView {
    /// Entity `address`'s drawn model primitives (Mesh Paint targets): draw identity,
    /// model generation, primitive index and model-to-world.
    pub(crate) fn paint_targets(
        &self,
        address: &str,
    ) -> Vec<(
        u64,
        std::sync::Arc<ara_runtime::model::Model>,
        usize,
        glam::Mat4,
    )> {
        self.frame
            .iter()
            .flat_map(|frame| &frame.draws)
            .filter(|d| d.address == address)
            .filter_map(|draw| match &draw.geometry {
                ara_runtime::extract::Geometry::Model { model, primitive } => Some((
                    draw.entity,
                    std::sync::Arc::clone(model),
                    *primitive,
                    glam::Mat4::from_cols_array(&draw.world.to_cols_array()),
                )),
                ara_runtime::extract::Geometry::Shape { .. } => None,
            })
            .collect()
    }
    /// Extract again at the next pass (an extraction notification, a changed input).
    pub(super) fn invalidate(&mut self) {
        self.revision = None;
        self.patched_pose = None;
    }
    /// Project typed Inspector drafts without authoring documents or baking meshes.
    pub(super) fn set_preview_view(
        &mut self,
        scene: &str,
        view: Option<crate::world::DocumentView>,
    ) {
        self.content_epoch = self.content_epoch.wrapping_add(1);
        self.preview_view = view;
        self.preview_scene.clear();
        if self.preview_view.is_some() {
            self.preview_scene.push_str(scene);
        }
        self.invalidate();
    }
    /// One pass of the Scene view (`scene_frame`). After a Renderlab failure there is nothing
    /// to draw: the panel shows `renderlab_error`.
    pub fn render(
        &mut self,
        device: &mut SceneDevice,
        editor: &Editor,
        view: &crate::world::DocumentView,
    ) -> ara_ui::Result<()> {
        if self.renderlab_error.is_some() {
            return Ok(());
        }
        self.render_scene_frame(device, editor, view)
    }
    /// Whether the next pass has work: a frame being extracted, or Renderlab still settling.
    pub fn needs_frame(&self) -> bool {
        self.scene_settle > 0 || self.content_job.is_some() || self.content_ready.is_some()
    }
    /// Why Renderlab could not draw the Scene, once it failed: the Scene panel shows it.
    pub(super) fn renderlab_error(&self) -> Option<&str> {
        self.renderlab_error.as_deref()
    }
    /// Whether the Scene has an image the compositor can show.
    pub(super) fn presentable(&self) -> bool {
        self.renderlab
            .as_ref()
            .is_some_and(|scene| scene.image().is_some())
    }
    pub(super) fn id_host(&self) -> Option<ids::IdHost<'_>> {
        self.renderlab
            .as_ref()
            .filter(|r| r.image().is_some())
            .and_then(renderlab::RenderlabScene::id_host)
    }
    /// The channels (draw `n` is `n + 1`) the Scene's ids name.
    pub(super) fn entities(&self) -> &[(u64, String)] {
        self.id_host().map_or(&[], |host| host.entities)
    }
    pub fn attach(&self, gpu: &mut NativeUi, device: &SceneDevice) -> ara_ui::Result<()> {
        match self
            .renderlab
            .as_ref()
            .and_then(|r| r.image().zip(r.id_image()))
        {
            Some(images) => attach(gpu, device, images)?,
            None => {
                gpu.forget_external(overlay::viewport_image());
                gpu.forget_external(overlay::ENTITY_IMAGE);
            }
        }
        Ok(())
    }
    pub fn destroy(self, device: &mut SceneDevice) -> ara_ui::Result<()> {
        if let Some(image) = self.published {
            image.destroy(device.gpu_mut())?;
        }
        if let Some(renderlab) = self.renderlab {
            renderlab.release(device.gpu_mut())?;
        }
        Ok(())
    }
}

/// Every Scene view mode: its `editor.view` name, its title, the group menus list it
/// under, and what the renderer draws. `editor.view` accepts exactly these names.
pub(crate) const VIEW_MODES: [(&str, &str, &str, ViewMode); 19] = [
    ("lit", "Lit", "Shading", ViewMode::Lit),
    ("baseColor", "Unlit", "Shading", ViewMode::Albedo),
    ("wireframe", "Wireframe", "Shading", ViewMode::Wireframe),
    (
        "litWireframe",
        "Lit wireframe",
        "Shading",
        ViewMode::LitWireframe,
    ),
    ("normals", "Normals", "Material", ViewMode::Normals),
    (
        "roughnessMetal",
        "Roughness and metal",
        "Material",
        ViewMode::RoughnessMetal,
    ),
    ("depth", "Depth", "Material", ViewMode::Depth),
    (
        "vertexColor",
        "Vertex colour",
        "Material",
        ViewMode::VertexColor,
    ),
    (
        "vertexColorRed",
        "Vertex colour: red",
        "Material",
        ViewMode::VertexColorRed,
    ),
    (
        "vertexColorGreen",
        "Vertex colour: green",
        "Material",
        ViewMode::VertexColorGreen,
    ),
    (
        "vertexColorBlue",
        "Vertex colour: blue",
        "Material",
        ViewMode::VertexColorBlue,
    ),
    (
        "vertexColorAlpha",
        "Vertex colour: alpha",
        "Material",
        ViewMode::VertexColorAlpha,
    ),
    (
        "gi_diffuse",
        "Indirect diffuse (GI)",
        "Lighting",
        ViewMode::GiDiffuse,
    ),
    (
        "shadowCascades",
        "Shadow cascades",
        "Lighting",
        ViewMode::ShadowCascades,
    ),
    (
        "shadowPenumbra",
        "Shadow penumbra",
        "Lighting",
        ViewMode::ShadowPenumbra,
    ),
    (
        "lightComplexity",
        "Light complexity",
        "Lighting",
        ViewMode::LightComplexity,
    ),
    ("overdraw", "Overdraw", "Performance", ViewMode::Overdraw),
    (
        "decalCoverage",
        "Decal coverage",
        "Performance",
        ViewMode::DecalCoverage,
    ),
    (
        "entityIds",
        "Entity IDs",
        "Performance",
        ViewMode::EntityIds,
    ),
];


fn pose_key(gesture: Option<&crate::world::gesture::Gesture>) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    gesture.map(|gesture| {
        let mut hash = std::hash::DefaultHasher::new();
        for (id, matrix) in &gesture.current {
            id.hash(&mut hash);
            for word in matrix.to_cols_array() {
                word.to_bits().hash(&mut hash);
            }
        }
        hash.finish()
    })
}
