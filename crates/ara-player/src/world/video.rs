//! `play.capture` `video`: a clip of live frames (jitter, temporal resolve and auto
//! exposure running as in Play) while the camera travels and turns linearly, one PNG
//! per 60 Hz frame, for side-by-side comparisons (`ffmpeg -framerate 60 -i %05d.png`).
use anyhow::{Context, Result};
use ara_frame::capture::CaptureRequest;
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::{Presenter, fidelity::native::Content};
use std::path::Path;

/// Still frames rendered at the start pose first, so the resolve and exposure have
/// settled before the clip begins.
const WARMUP: u32 = 30;

/// The camera's move over the clip.
pub(super) struct Clip {
    /// Frames to record.
    pub frames: u32,
    /// World-space travel in metres.
    pub travel: [f32; 3],
    /// Turn about the vertical axis in degrees.
    pub yaw: f32,
}

/// Renders the clip at `request.size` into `dir` (`00000.png`, …) from the frame's
/// camera, then restores the presenter's size.
pub(super) fn record(
    presenter: &mut Presenter,
    frame: &mut RenderFrame,
    request: &CaptureRequest,
    clip: Clip,
    dir: &Path,
    mut content: impl FnMut(&RenderFrame) -> Result<Content>,
) -> Result<()> {
    let start = frame.camera.context("a camera to record from")?;
    std::fs::create_dir_all(dir)?;
    let original = presenter.size();
    presenter.resize(request.size)?;
    let result = (|| {
        for i in 0..WARMUP + clip.frames {
            let t = i.saturating_sub(WARMUP) as f32 / clip.frames.saturating_sub(1).max(1) as f32;
            let (sin, cos) = (clip.yaw * t).to_radians().sin_cos();
            let [x, y, z] = start.forward;
            let camera = frame.camera.as_mut().context("camera")?;
            camera.position = std::array::from_fn(|k| start.position[k] + clip.travel[k] * t);
            camera.forward = [x * cos + z * sin, y, z * cos - x * sin];
            let frame = &*frame;
            presenter.present_with(frame, || content(frame))?;
            if i >= WARMUP {
                presenter
                    .live_image(request)
                    .context("live frame")?
                    .save(dir.join(format!("{:05}.png", i - WARMUP)))?;
            }
        }
        Ok(())
    })();
    frame.camera = Some(start);
    presenter.resize(original)?;
    result
}
