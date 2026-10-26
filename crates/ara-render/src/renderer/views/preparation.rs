//! Bounded secondary target allocation; no GPU submission or shared-content work.
use super::*;
use ara_gpu::vulkan::{BufferPreparation, ImagePreparation, PreparationId, ResourcePreparation};

pub(super) struct Pending {
    render: [u32; 2],
    display: [u32; 2],
    pub ticket: PreparationId,
}

pub(super) fn request(render: [u32; 2], display: [u32; 2], retained: bool) -> ResourcePreparation {
    let mut request = targets::request(render, display);
    for size in [std::mem::size_of::<frame::Frame>(); 3].into_iter().chain([
        std::mem::size_of::<crate::look::frame::LookFrame>(),
        crate::look::gpu::STATE_BYTES,
    ]) {
        request.buffers.push(BufferPreparation {
            size: size as u64,
            usage: vk::BufferUsageFlags::STORAGE_BUFFER,
            host: true,
            data: Default::default(),
        });
    }
    if retained {
        for _ in 0..2 {
            for (size, format) in [
                (display, vk::Format::R8G8B8A8_UNORM),
                (render, vk::Format::R32_SFLOAT),
            ] {
                request.images.push(ImagePreparation {
                    size,
                    format,
                    mip_levels: 1,
                    usage: vk::ImageUsageFlags::SAMPLED
                        | vk::ImageUsageFlags::TRANSFER_SRC
                        | vk::ImageUsageFlags::TRANSFER_DST,
                });
            }
        }
    }
    request
}

impl Renderer {
    /// Prepare one secondary camera's quality maps before handing the renderer
    /// to an interactive owner. Tiny working targets are replaced asynchronously
    /// at the first panel extent; quality resources remain resident. Keeps only
    /// one unclaimed camera and never renders or changes the primary history.
    pub fn prewarm_view_quality(&mut self, quality: crate::look::Quality) -> Result<()> {
        ara_gpu::cpu_scope!("Raster.prewarm_view_quality");
        ensure!(
            !self.scene_frame_pending() && !self.gpu.deferring_completion(),
            "Prewarm before renderer handoff"
        );
        if self
            .views
            .warm
            .as_ref()
            .is_some_and(|s| s.look.quality == quality)
        {
            return Ok(());
        }
        let mut state = ViewState::new(self, [1, 1], [1, 1])?;
        state.swap(self);
        let result = self.set_quality(quality);
        state.swap(self);
        if let Err(error) = result {
            state.destroy(&mut self.gpu)?;
            return Err(error);
        }
        if let Some(old) = self.views.warm.replace(state) {
            old.destroy(&mut self.gpu)?;
        }
        Ok(())
    }
    /// Reserve a secondary camera's render/display targets, constants and two
    /// retained output pairs on the bounded allocation worker. Poll at a completed
    /// boundary; false keeps the last image (or loading) without a host wait.
    /// Hidden callers need not poll; release cancels an unfinished allocation.
    /// This does not render, reset the primary history, or change shared content.
    pub fn preallocate_view(
        &mut self,
        view: &mut ViewId,
        request: &ViewRequest<'_>,
    ) -> Result<bool> {
        ara_gpu::cpu_scope!("Raster.preallocate_view");
        ensure!(self.views.owns(view), "View belongs to another renderer");
        request.validate()?;
        let retained = self
            .views
            .outputs
            .get_mut(&view.id)
            .context("secondary view")?;
        if retained
            .preparation
            .as_ref()
            .is_some_and(|p| p.render != request.render_size || p.display != request.size)
        {
            let old = retained.preparation.take().expect("changed target request");
            self.gpu.cancel_prepared_resources(old.ticket)?;
        }
        if view
            .state
            .as_ref()
            .is_some_and(|s| s.matches_extent(request.render_size, request.size))
        {
            return Ok(true);
        }
        if retained.preparation.is_none() {
            if let Some(ticket) = self
                .gpu
                .prepare_resources(|| self::request(request.render_size, request.size, true))?
            {
                retained.preparation = Some(Pending {
                    render: request.render_size,
                    display: request.size,
                    ticket,
                });
            }
            return Ok(false);
        }
        if self.scene_frame_pending() {
            // A continuously converging Scene must yield one completed boundary
            // for target adoption, as it already does for secondary submission.
            self.views.requested = Some(view.id);
            return Ok(false);
        }
        if self.gpu.deferring_completion() {
            return Ok(false);
        }
        if self.views.requested == Some(view.id) {
            self.views.requested = None;
        }
        let retained = self
            .views
            .outputs
            .get_mut(&view.id)
            .context("secondary view")?;
        let ticket = retained
            .preparation
            .as_ref()
            .context("pending targets")?
            .ticket;
        let resources = match self.gpu.poll_prepared_resources(ticket) {
            Ok(None) => return Ok(false),
            Ok(Some(resources)) => resources,
            Err(error) => {
                retained.preparation = None;
                return Err(error.into());
            }
        };
        retained.preparation = None;
        let (mut state, images) =
            ViewState::from_prepared(self, request.render_size, request.size, resources)?;
        if let Some(old) = view.state.as_mut() {
            state.inherit(old, &mut self.gpu, request.render_size, request.size)?;
        }
        let live = self
            .views
            .outputs
            .get_mut(&view.id)
            .context("secondary view")?;
        live.live
            .replace_preallocated(&mut self.gpu, images, request.size, request.render_size)?;
        if let Some(old) = view.state.replace(state) {
            old.destroy(&mut self.gpu)?;
        }
        Ok(true)
    }
}
