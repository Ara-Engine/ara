//! Owned secondary cameras over one device and one uploaded scene.
use super::*;
use anyhow::Context;
use std::sync::{Arc, Weak, mpsc};
mod asynchronous;
mod captures;
mod preparation;
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
    id: u64,
    state: Option<ViewState>,
}

/// One on-demand raster camera frame; no background or periodic rendering occurs.
#[derive(Clone, Copy)]
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
impl ViewRequest<'_> {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.size.contains(&0) && !self.render_size.contains(&0),
            "Secondary extent is zero"
        );
        ensure!(
            self.render_size.iter().zip(self.size).all(|(&r, d)| r <= d),
            "Secondary render exceeds display"
        );
        self.camera.resolve(self.size)?;
        self.quality.validate()?;
        ensure!(
            self.exposure.is_finite() && self.exposure > 0.,
            "Invalid view exposure"
        );
        Ok(())
    }
}

/// Completed same-device output, in Graphics/CopySource like `Renderer::output`.
#[derive(Clone, Copy)]
pub struct ViewOutput {
    /// Color image retained by the view until it resizes or is released.
    pub image: Image,
    /// Native view for a UI external-image attachment on this renderer's device.
    pub view: ara_gpu::NativeView,
    /// Independent entity-ID channel in Graphics/CopySource.
    pub entities: Image,
    /// Actual physical output extent.
    pub size: [u32; 2],
}

struct Owner {
    retired: mpsc::Sender<(u64, Option<ViewState>)>,
}
pub(super) struct Views {
    owner: Arc<Owner>,
    retired: mpsc::Receiver<(u64, Option<ViewState>)>,
    next: u64,
    warm: Option<ViewState>,
    outputs: std::collections::BTreeMap<u64, asynchronous::Retained>,
    pending: Option<u64>,
    requested: Option<u64>,
    preparing: Option<ara_gpu::GpuCompletion>,
    /// Reflection captures (`look::captures`): specs, atlas and bake plan. Every
    /// camera's frames read them (`Captures::write`); only primary frames bake.
    pub(super) captures: crate::look::captures::Captures,
    /// The square camera captures bake their faces through (`captures.rs`).
    capture_view: Option<ViewState>,
}
impl Default for Views {
    fn default() -> Self {
        let (retired, receiver) = mpsc::channel();
        Self {
            owner: Arc::new(Owner { retired }),
            retired: receiver,
            next: 0,
            warm: None,
            outputs: Default::default(),
            pending: None,
            requested: None,
            preparing: None,
            captures: Default::default(),
            capture_view: None,
        }
    }
}
impl Views {
    pub(super) fn collect(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        let mut retired = false;
        while let Ok((id, state)) = self.retired.try_recv() {
            if let Some(state) = state {
                state.destroy(gpu)?;
            }
            if self.requested == Some(id) {
                self.requested = None;
            }
            if let Some(output) = self.outputs.get_mut(&id) {
                output.retired = true;
            }
            retired = true;
        }
        let removable: Vec<_> = self
            .outputs
            .iter()
            .filter(|(_, output)| output.retired && !output.live.pending())
            .map(|(&id, _)| id)
            .collect();
        for id in removable {
            let mut output = self.outputs.remove(&id).expect("retired view exists");
            if let Some(pending) = output.preparation.take() {
                gpu.cancel_prepared_resources(pending.ticket)?;
            }
            output.live.destroy(gpu)?;
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
        if let Some(owner) = self.owner.upgrade() {
            // The renderer/device may already be gone; its device-owned arena
            // then freed these resources. Otherwise only the owner retires them.
            let _ = owner.retired.send((self.id, self.state.take()));
        }
    }
}

impl Renderer {
    /// Creates a secondary camera on this device, claiming an optional prewarmed
    /// quality state. Its extent stays lazy; hidden tabs do not render. Use
    /// `preallocate_view` to allocate targets without blocking an interactive owner.
    pub fn create_view(&mut self) -> Result<ViewId> {
        self.collect_view_resources()?;
        let id = self.views.next;
        self.views.next = id.checked_add(1).context("secondary view IDs exhausted")?;
        self.views.outputs.insert(id, Default::default());
        Ok(ViewId {
            owner: Arc::downgrade(&self.views.owner),
            id,
            state: self.views.warm.take(),
        })
    }

    /// Retire dropped view targets at a completed frame boundary.
    pub fn collect_view_resources(&mut self) -> Result<()> {
        self.views.collect(&mut self.gpu)
    }

    /// Release an owned secondary view without a host wait. In-flight resources
    /// retire after completion; dropping the handle schedules the same cleanup.
    pub fn release_view(&mut self, view: ViewId) -> Result<()> {
        ensure!(self.views.owns(&view), "View belongs to another renderer");
        drop(view);
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
        self.wait_scene_frame()?;
        self.with_secondary(view, request, |renderer| {
            renderer.draw_secondary(request)?;
            Ok(ViewOutput {
                image: renderer.output(),
                view: renderer.gpu.image_view(renderer.output())?,
                entities: renderer.entity_channel(),
                size: renderer.size,
            })
        })
    }

    fn draw_secondary(&mut self, request: &ViewRequest<'_>) -> Result<()> {
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
        Ok(())
    }

    fn with_secondary<T>(
        &mut self,
        view: &mut ViewId,
        request: &ViewRequest<'_>,
        draw: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        ensure!(self.views.owns(view), "View belongs to another renderer");
        ensure!(!self.secondary_active, "Nested secondary rendering");
        request.validate()?;
        self.collect_view_resources()?;
        if view.state.is_none() {
            view.state = Some(ViewState::new(self, request.render_size, request.size)?);
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
            draw(self)
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
