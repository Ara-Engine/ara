//! Explicit readbacks of the installed renderer's presentation.
use super::*;

impl Presenter {
    /// Captures `frame` with the installed renderer (history cut, settled, read back).
    /// Resolves content only when required.
    pub fn capture_with(
        &mut self,
        frame: &RenderFrame,
        request: &CaptureRequest,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<Capture> {
        let original_size = self.size;
        let result = self.capture_at(frame, request, resolve);
        self.size = original_size;
        result
    }

    fn capture_at(
        &mut self,
        frame: &RenderFrame,
        request: &CaptureRequest,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<Capture> {
        // A buffer visualization (RenderDebug::view) replaces the lit image.
        let viewed;
        let request = if matches!(request.mode, ViewMode::Lit) && self.debug.view != ViewMode::Lit {
            viewed = CaptureRequest {
                mode: self.debug.view,
                ..request.clone()
            };
            &viewed
        } else {
            request
        };
        self.resize(request.size)?;
        self.capture_external(frame, request, resolve)
    }

    /// A deterministic capture of `frame` in `mode`, settled for 16 frames.
    pub fn capture(
        &mut self,
        frame: &RenderFrame,
        size: [u32; 2],
        mode: ViewMode,
        marks: Option<usize>,
        seed: u64,
    ) -> Result<Capture> {
        self.capture_with(
            frame,
            &CaptureRequest {
                camera: camera(frame)?,
                size,
                mode,
                deterministic: true,
                exposure: frame.environment.exposure,
                settle_frames: 16,
                tick: frame.tick,
                seed,
                marks,
            },
            || Ok(Content::default()),
        )
    }
}
