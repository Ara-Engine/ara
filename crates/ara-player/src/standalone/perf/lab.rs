//! Lab control state is opt-in; idle tracks submit no GPU work.
use super::*;
use ara_runtime::extract::{CameraView, RenderFrame};
use serde::Deserialize;
use std::path::{Path, PathBuf};
#[cfg(feature = "fidelity-lab")]
mod metrics;

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
    references: Option<PathBuf>,
    #[serde(default)]
    quality: Value,
}
impl Capture {
    fn analyze(
        &self,
        shot: &str,
        image: ara_render::capture::Capture,
        render_ms: f64,
    ) -> Result<Value> {
        let began = Instant::now();
        let path = self.directory.join(format!("{shot}.png"));
        image.write(&path)?;
        let encode_ms = began.elapsed().as_secs_f64() * 1000.;
        let mut metrics = json!({"camera":shot,"png":path});
        #[cfg(feature = "fidelity-lab")]
        if let Some(references) = &self.references {
            let reference = references.join(format!("{shot}.png"));
            if reference.is_file() {
                let reference = image::open(&reference)?.into_rgba8();
                let resized = reference.dimensions() != (self.size[0], self.size[1]);
                let reference = if resized {
                    image::imageops::resize(
                        &reference,
                        self.size[0],
                        self.size[1],
                        image::imageops::FilterType::Lanczos3,
                    )
                } else {
                    reference
                };
                let flip = ara_pathtrace::flip::flip(
                    &reference,
                    &image.image,
                    ara_pathtrace::flip::PIXELS_PER_DEGREE,
                )?;
                let compare = ara_render::capture::compare(&image.image, &reference, 2.)?;
                let diff = self.directory.join(format!("{shot}-diff.png"));
                compare.diff.save(&diff)?;
                metrics["ldr_secondary"] = json!({"flip":flip.mean,"ssim":1.-compare.score,"diff":diff,"reference_resized":resized});
                metrics["regions"] = Value::Array(metrics::regions(
                    &self.quality,
                    shot,
                    &image.image,
                    &reference,
                    &self.directory,
                )?);
            } else {
                metrics["reference_missing"] = json!(reference);
            }
        }
        #[cfg(not(feature = "fidelity-lab"))]
        {
            let _ = (&self.references, &self.quality);
            metrics["metrics_unavailable"] = json!("rebuild with fidelity-lab feature");
        }
        metrics["timings_ms"] = json!({"render_readback":render_ms,"encode":encode_ms,"analysis":began.elapsed().as_secs_f64()*1000.-encode_ms});
        Ok(metrics)
    }
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
            std::fs::create_dir_all(&capture.directory)?;
            let images = std::thread::scope(|scope| -> Result<Vec<Value>> {
                let mut analyses = Vec::new();
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
                    let image = presenter.capture_presented(frame, capture.size, None, 0)?;
                    let render_ms = began.elapsed().as_secs_f64() * 1000.;
                    let capture = &capture;
                    analyses
                        .push(scope.spawn(move || capture.analyze(&shot.name, image, render_ms)));
                }
                analyses
                    .into_iter()
                    .map(|analysis| {
                        analysis
                            .join()
                            .map_err(|_| anyhow::anyhow!("capture analysis worker panicked"))?
                    })
                    .collect()
            })?;
            self.apply_camera(frame);
            let region_scores = images
                .iter()
                .filter_map(|image| image["regions"].as_array())
                .flatten()
                .filter_map(|region| region["ldr_secondary"]["flip"].as_f64())
                .collect::<Vec<_>>();
            let mean = (!region_scores.is_empty())
                .then(|| region_scores.iter().sum::<f64>() / region_scores.len() as f64);
            Ok(
                json!({"images":images,"directory":capture.directory,"region_mean_ldr_secondary":mean,"matched_gate":{"status":"not_measured","reason":"matched-transform references required; LDR scores are secondary"}}),
            )
        })();
        match outcome {
            Ok(result) => self.lab.respond(&result)?,
            Err(error) => self.lab.respond(&json!({"error":format!("{error:#}")}))?,
        }
        Ok(true)
    }
}
