//! On-demand Game frames share the Scene completion boundary and keep shown pixels.
use super::*;

#[derive(Default)]
pub(super) struct Retained {
    pub live: live::Live,
    pub retired: bool,
}

impl Views {
    pub(in crate::renderer) fn pending(&self) -> bool {
        self.pending.is_some() || self.preparing.is_some()
    }
    pub(in crate::renderer) fn take_request(&mut self) -> bool {
        // Yield one boundary, not an indefinite lock if a tab hides or its
        // settling burst ends before it can use its reserved frame.
        self.requested.take().is_some()
    }
    pub(in crate::renderer) fn cancel_request(&mut self) {
        self.requested = None;
    }

    pub(in crate::renderer) fn poll(&mut self, gpu: &mut VulkanDevice) -> Result<bool> {
        if let Some(completion) = &mut self.preparing {
            let result = gpu.poll_completion(completion);
            if completion.is_resolved() {
                self.preparing = None;
            }
            if result?.is_none() {
                return Ok(false);
            }
        }
        if let Some(id) = self.pending {
            let output = self
                .outputs
                .get_mut(&id)
                .context("pending secondary view")?;
            let result = output.live.poll(gpu);
            if !output.live.pending() {
                self.pending = None;
            }
            if !result? {
                return Ok(false);
            }
        }
        self.collect(gpu)?;
        Ok(true)
    }

    pub(in crate::renderer) fn wait(&mut self, gpu: &mut VulkanDevice) -> Result<()> {
        if let Some(completion) = &mut self.preparing {
            gpu.wait_completion(completion)?;
            self.preparing = None;
        }
        if let Some(id) = self.pending {
            self.outputs
                .get_mut(&id)
                .context("pending secondary view")?
                .live
                .wait(gpu)?;
            self.pending = None;
        }
        self.collect(gpu)
    }
}

impl Renderer {
    /// Poll an owned camera's last frame without scheduling another one. This
    /// also releases its one-shot scheduling reservation when a burst ends.
    pub fn poll_view(&mut self, view: &ViewId) -> Result<Option<ViewOutput>> {
        ensure!(self.views.owns(view), "View belongs to another renderer");
        if self.views.requested == Some(view.id) {
            self.views.requested = None;
        }
        self.poll_scene_frame()?;
        self.view_output(view)
    }

    /// Last completed secondary image, unchanged while another frame is pending.
    /// Polling never waits; None means this camera has not completed its first frame.
    pub fn view_output(&self, view: &ViewId) -> Result<Option<ViewOutput>> {
        ensure!(self.views.owns(view), "View belongs to another renderer");
        let live = &self
            .views
            .outputs
            .get(&view.id)
            .context("secondary view")?
            .live;
        let (Some(image), Some(entities), Some(size)) = (live.color(), live.ids(), live.size())
        else {
            return Ok(None);
        };
        Ok(Some(ViewOutput {
            image,
            view: self.gpu.image_view(image)?,
            entities,
            size,
        }))
    }

    /// Submit the Game camera without a GPU wait. `prepare` runs at the completed
    /// shared-content boundary, inside the same ordered upload scope as its draws.
    /// It may set the request's exposure from the newly adopted content.
    /// False keeps loading; input coalesces while a Scene/Game frame is pending.
    /// The returned image is always the last completed one. Capture callers use
    /// `render_secondary` or explicitly `wait_scene_frame` and `view_output`.
    pub fn submit_secondary(
        &mut self,
        view: &mut ViewId,
        request: &ViewRequest<'_>,
        prepare: impl FnOnce(&mut Self, &mut ViewRequest<'_>) -> Result<bool>,
    ) -> Result<Option<ViewOutput>> {
        ara_gpu::cpu_scope!("Raster.submit_secondary");
        ensure!(self.views.owns(view), "View belongs to another renderer");
        ensure!(!self.secondary_active, "Nested secondary rendering");
        request.validate()?;
        if !self.poll_scene_frame()? {
            // Reserve the next idle boundary so primary accumulation cannot
            // indefinitely starve a visible Game panel's first frame.
            self.views.requested = Some(view.id);
            return self.view_output(view);
        }
        self.views.requested = None;
        self.gpu.begin_deferred_completion()?;
        let mut request = *request;
        let mut output = None;
        let result: Result<bool> = (|| {
            if !prepare(self, &mut request)? {
                return Ok(false);
            }
            request.validate()?;
            output = Some(
                self.views
                    .outputs
                    .get_mut(&view.id)
                    .context("secondary view")?
                    .live
                    .acquire(&mut self.gpu, request.size, request.render_size)?,
            );
            self.with_secondary(view, &request, |renderer| {
                renderer.draw_secondary(&request)?;
                output
                    .as_mut()
                    .expect("secondary output acquired")
                    .copy(renderer)
            })?;
            Ok(true)
        })();
        let completion = self.gpu.end_deferred_completion()?;
        if let Some(output) = output {
            self.views
                .outputs
                .get_mut(&view.id)
                .context("secondary view")?
                .live
                .submitted(
                    completion,
                    output,
                    result.as_ref().is_ok_and(|ready| *ready),
                );
            self.views.pending = Some(view.id);
        } else {
            // Preparation can submit uploads even when content is not ready or
            // rejected. Its token must remain owned until those writes retire.
            self.views.preparing = Some(completion);
        }
        result?;
        self.view_output(view)
    }
}
