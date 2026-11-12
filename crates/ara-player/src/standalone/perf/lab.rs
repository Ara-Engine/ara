//! Lab control state is opt-in; idle tracks submit no GPU work. A lab capture is a set of
//! shots through the installed renderer, each written as a PNG with its render time. The
//! comparison metrics the old look-development lab computed (reference diffs, regions,
//! temporal stability, live parameters) need Renderlab parameter keys and are not
//! available.
use super::*;
use ara_frame::capture::ViewMode;
use ara_runtime::extract::{CameraView, RenderFrame};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Deserialize)]
pub(super) struct Camera {
    position: [f32; 3],
    forward: [f32; 3],
    fov: f32,
}
impl Camera {
    pub fn view(self) -> Result<CameraView> {
        ensure!(
            self.position
                .iter()
                .chain(&self.forward)
                .all(|v| v.is_finite())
                && self.fov.is_finite()
                && (1. ..179.).contains(&self.fov),
            "invalid camera"
        );
        let length = self.forward.iter().map(|v| v * v).sum::<f32>().sqrt();
        ensure!(length > 0., "empty camera direction");
        Ok(CameraView {
            position: self.position,
            forward: self.forward.map(|v| v / length),
            fov: self.fov,
        })
    }
}
#[derive(Deserialize)]
struct Shot {
    name: String,
    camera: Camera,
}
#[derive(Deserialize)]
pub(super) struct Capture {
    directory: PathBuf,
    shots: Vec<Shot>,
    size: [u32; 2],
}
#[derive(Default)]
pub(super) struct State {
    pub camera: Option<CameraView>,
    pub capture: Option<Capture>,
    pub reply: Option<PathBuf>,
}
impl State {
    pub fn respond(&mut self, value: &Value) -> Result<()> {
        if let Some(path) = self.reply.take() {
            write(&path, value)?;
        }
        Ok(())
    }
}
fn write(path: &Path, value: &Value) -> Result<()> {
    let temporary = path.with_extension("reply.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}
impl Window {
    pub fn lab_mode(&self) -> bool {
        self.options.lab_idle
    }
    pub fn idle(&self) -> bool {
        self.options.lab_idle && !self.collecting && self.lab.capture.is_none()
    }
    pub fn apply_camera(&self, frame: &mut RenderFrame) {
        if let Some(camera) = self.lab.camera {
            frame.camera = Some(camera);
        }
    }
    pub fn capture(&mut self, presenter: &mut Presenter, frame: &mut RenderFrame) -> Result<bool> {
        let Some(capture) = self.lab.capture.take() else {
            return Ok(false);
        };
        let outcome = (|| -> Result<Value> {
            ensure!(
                capture.size.iter().all(|s| (16..=8192).contains(s)),
                "capture resolution outside 16..8192"
            );
            let epoch = presenter.shader_status().epoch;
            std::fs::create_dir_all(&capture.directory)?;
            let mut images = Vec::new();
            for shot in &capture.shots {
                ensure!(
                    !shot.name.is_empty()
                        && shot
                            .name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                    "invalid shot name"
                );
                frame.camera = Some(shot.camera.view()?);
                let began = Instant::now();
                let image = presenter.capture(frame, capture.size, ViewMode::Lit, None, 0)?;
                let render_ms = began.elapsed().as_secs_f64() * 1000.;
                let path = capture.directory.join(format!("{}.png", shot.name));
                image.write(&path)?;
                images.push(json!({
                    "camera":shot.name,
                    "png":path,
                    "metrics_unavailable":"the lab's comparison metrics need Renderlab parameter keys",
                    "timings_ms":{"render_readback":render_ms},
                }));
            }
            self.apply_camera(frame);
            ensure!(
                presenter.shader_status().epoch == epoch,
                "shaders changed during capture; retry after publication"
            );
            Ok(json!({"images":images,"directory":capture.directory,"shaders":presenter.shader_status()}))
        })();
        match outcome {
            Ok(result) => self.lab.respond(&result)?,
            Err(error) => self.lab.respond(&json!({"error":format!("{error:#}")}))?,
        }
        Ok(true)
    }
}
