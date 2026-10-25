//! Owned secondary cameras over one device and one uploaded scene.
use super::*;
use anyhow::Context;
use std::sync::{Arc, Weak, mpsc};
mod state;
#[cfg(test)]
pub(super) mod tests;
use state::ViewState;

/// An independent camera's targets and temporal state on its creating renderer.
/// Keep this handle alive while the UI samples its output. Dropping it schedules
/// resource retirement at the next completed renderer boundary; `release_view`
/// performs that work immediately. It owns no scene, device or pipeline copies.
pub struct ViewId {
    owner: Weak<Owner>,
    state: Option<ViewState>,
}

/// One on-demand raster camera frame; no background or periodic rendering occurs.
pub struct ViewRequest<'a> {
    /// Camera independent of the Scene viewport's camera and previous projection.
    pub camera: &'a CameraSpec,
    /// Physical pixels of this view, independent of the primary output extent.
    pub size: [u32; 2],
    /// Render size of its scene passes, at most `size` per axis: smaller sizes
    /// are temporally upscaled (`Renderer::resize_upscaled`; a `RasterPlan`'s
    /// `render_size`); `size` renders natively.
    pub render_size: [u32; 2],
    /// Raster feature budgets for this view. Probe GI uses the shared volume.
    pub quality: crate::look::Quality,
    /// Positive authored exposure multiplier; auto exposure keeps per-view state.
    pub exposure: f32,
}

/// Completed same-device output, in Graphics/CopySource like `Renderer::output`.
#[derive(Clone, Copy)]
pub struct ViewOutput {
    /// Color image retained by the view until it resizes or is released.
    pub image: Image,
    /// Native view for a UI external-image attachment on this renderer's device.
    pub view: vk::ImageView,
    /// Independent entity-ID channel in Graphics/CopySource.
    pub entities: Image,
    /// Actual physical output extent.
    pub size: [u32; 2],
}

struct Owner {
    retired: mpsc::Sender<ViewState>,
}
pub(super) struct Views {
    owner: Arc<Owner>,
    retired: mpsc::Receiver<ViewState>,
}
impl Default for Views {
    fn default() -> Self {
        let (retired, receiver) = mpsc::channel();
        Self {
            owner: Arc::new(Owner { retired }),
            retired: receiver,
        }
    }
}
impl Views {
    pub(super) fn collect(&mut self, gpu: &mut VulkanDevice) -> Result<()> {
        let mut retired = false;
        while let Ok(state) = self.retired.try_recv() {
            state.destroy(gpu)?;
            retired = true;
        }
        if retired {
            gpu.collect()?;
        }
        Ok(())
    }
    fn owns(&self, view: &ViewId) -> bool {
        std::ptr::eq(view.owner.as_ptr(), Arc::as_ptr(&self.owner))
    }
}
impl Drop for ViewId {
    fn drop(&mut self) {
        if let (Some(owner), Some(state)) = (self.owner.upgrade(), self.state.take()) {
            // The renderer/device may already be gone; its device-owned arena
            // then freed these resources. Otherwise only the owner retires them.
            let _ = owner.retired.send(state);
        }
    }
}

impl Renderer {
    /// Creates a lazy secondary camera on this device. The first `render_secondary`
    /// allocates its extent; hidden tabs cost no rendering or target allocations.
    pub fn create_view(&mut self) -> Result<ViewId> {
        self.collect_view_resources()?;
        Ok(ViewId {
            owner: Arc::downgrade(&self.views.owner),
            state: None,
        })
    }

    /// Retire dropped view targets at a completed frame boundary.
    pub fn collect_view_resources(&mut self) -> Result<()> {
        self.views.collect(&mut self.gpu)
    }

    /// Immediately release an owned secondary view; dropping its handle also
    /// works and defers retirement until the next renderer call.
    pub fn release_view(&mut self, mut view: ViewId) -> Result<()> {
        ensure!(self.views.owns(&view), "View belongs to another renderer");
        if let Some(state) = view.state.take() {
            state.destroy(&mut self.gpu)?;
        }
        self.collect_view_resources()
    }

    /// Render only this camera, sharing uploaded content, residency and pipeline
    /// handles. Primary images, motion, exposure and temporal history are restored
    /// on success and failure. No capture resets, settle loop or readback occurs.
    pub fn render_secondary(
        &mut self,
        view: &mut ViewId,
        request: &ViewRequest<'_>,
    ) -> Result<ViewOutput> {
        ara_gpu::cpu_scope!("Raster.secondary_view");
        ensure!(self.views.owns(view), "View belongs to another renderer");
        ensure!(!self.secondary_active, "Nested secondary rendering");
        ensure!(!request.size.contains(&0), "Secondary extent is zero");
        request.camera.resolve(request.size)?;
        request.quality.validate()?;
        ensure!(
            request.exposure.is_finite() && request.exposure > 0.,
            "Invalid view exposure"
        );
        self.collect_view_resources()?;
        if view.state.is_none() {
            view.state = Some(ViewState::new(self, request.size)?);
        }
        let state = view.state.as_mut().context("secondary view initialized")?;
        state.prepare_native(self)?;
        state.look.look = self.look.look;
        let mut gi = self.look.gi.take();
        let mut staging = std::mem::take(&mut self.look.probe_staging);
        let mut writes = std::mem::take(&mut self.look.probe_writes);
        state.swap(self);
        self.look.gi = gi.take();
        self.look.probe_staging = std::mem::take(&mut staging);
        self.look.probe_writes = std::mem::take(&mut writes);
        self.secondary_active = true;
        let result = (|| {
            self.resize_upscaled(request.render_size, request.size)?;
            if self.look.quality != request.quality {
                self.set_quality(request.quality)?;
            }
            self.render_frame(
                request.camera,
                request.exposure,
                false,
                crate::capture::ViewMode::Lit,
                false,
            )?;
            ensure!(
                self.presentation_ready(),
                "Secondary presentation is incomplete"
            );
            Ok(ViewOutput {
                image: self.output(),
                view: self.gpu.image_view(self.output())?,
                entities: self.entity_channel(),
                size: self.size,
            })
        })();
        self.secondary_active = false;
        gi = self.look.gi.take();
        staging = std::mem::take(&mut self.look.probe_staging);
        writes = std::mem::take(&mut self.look.probe_writes);
        state.swap(self);
        self.look.gi = gi;
        self.look.probe_staging = staging;
        self.look.probe_writes = writes;
        if !self.gpu.deferring_completion() {
            self.gpu.publish_completed_to_ui()?;
        }
        result
    }
}
