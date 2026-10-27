//! Lab control state is opt-in; idle tracks submit no GPU work.
use super::*;
use ara_runtime::extract::{CameraView, RenderFrame};
use serde::Deserialize;
use std::path::{Path, PathBuf};
#[cfg(feature = "fidelity-lab")]
mod display;
#[cfg(feature = "fidelity-lab")]
mod metrics;
#[cfg(feature = "fidelity-lab")]
mod params;
#[cfg(feature = "fidelity-lab")]
mod temporal;

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
    linear_references: Option<PathBuf>,
    #[serde(default)]
    linear_policy: Value,
    #[serde(default = "three")]
    runs: u32,
    #[serde(default = "enabled")]
    temporal: bool,
    #[serde(default)]
    quality: Value,
}
fn three() -> u32 {
    3
}
fn enabled() -> bool {
    true
}
#[cfg(feature = "fidelity-lab")]
#[derive(Deserialize)]
pub(super) struct TemporalRequest {
    directory: PathBuf,
    #[serde(default)]
    config: Value,
}
struct Captured {
    image: ara_render::capture::Capture,
    render_ms: f64,
    #[cfg(feature = "fidelity-lab")]
    transform: std::result::Result<display::Transform, String>,
    #[cfg(feature = "fidelity-lab")]
    repeats: Vec<image::RgbaImage>,
    #[cfg(feature = "fidelity-lab")]
    temporal: Value,
}
impl Capture {
    fn analyze(&self, shot: &str, captured: Captured) -> Result<Value> {
        let image = captured.image;
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
            } else {
                metrics["reference_missing"] = json!(reference);
            }
        }
        #[cfg(feature = "fidelity-lab")]
        {
            let linear = self
                .linear_references
                .as_ref()
                .map(|folder| folder.join(format!("{shot}.exr")));
            let primary = if self.linear_policy["colour_space"] != "scene_linear_rec709"
                || self.linear_policy["exposure_baked"] != false
            {
                metrics["matched_transform"] = json!({"status":"not_measured","reason":"linear-reference.json must declare unexposed scene-linear Rec.709 and its radiance scale"});
                None
            } else if let Some(path) = linear.as_ref().filter(|p| p.is_file()) {
                match &captured.transform {
                    Ok(transform) => {
                        let cache = self
                            .directory
                            .parent()
                            .context("capture cache parent")?
                            .join("display-reference-cache");
                        let truth = transform.reference(path, self.size, &cache)?;
                        let png = self.directory.join(format!("{shot}-matched-reference.png"));
                        truth.save(&png)?;
                        metrics["matched_transform"] = json!({"reference_exr":path,"reference_display_png":png,"transform":transform.report()});
                        Some(truth)
                    }
                    Err(reason) => {
                        metrics["matched_transform"] =
                            json!({"status":"not_measured","reason":reason});
                        None
                    }
                }
            } else {
                metrics["matched_transform"] = json!({"status":"not_measured","reason":"scene-linear Cycles EXR missing; run lab reference","expected":linear});
                None
            };
            let reference = self
                .references
                .as_ref()
                .map(|folder| folder.join(format!("{shot}.png")))
                .filter(|p| p.is_file())
                .map(|path| image::open(path).map(|i| i.into_rgba8()))
                .transpose()?;
            let reference = reference.map(|i| {
                if i.dimensions() != (self.size[0], self.size[1]) {
                    image::imageops::resize(
                        &i,
                        self.size[0],
                        self.size[1],
                        image::imageops::FilterType::Lanczos3,
                    )
                } else {
                    i
                }
            });
            metrics["regions"] = Value::Array(metrics::regions(
                &self.quality,
                shot,
                &image.image,
                reference.as_ref(),
                primary.as_ref(),
                &captured.repeats,
                &self.directory,
            )?);
            metrics["temporal"] = captured.temporal;
        }
        #[cfg(not(feature = "fidelity-lab"))]
        {
            let _ = (
                &self.references,
                &self.quality,
                &self.linear_references,
                &self.linear_policy,
                self.runs,
                self.temporal,
            );
            metrics["metrics_unavailable"] = json!("rebuild with fidelity-lab feature");
        }
        metrics["timings_ms"] = json!({"render_readback":captured.render_ms,"encode":encode_ms,"analysis":began.elapsed().as_secs_f64()*1000.-encode_ms});
        Ok(metrics)
    }
}
#[cfg(feature = "fidelity-lab")]
#[derive(serde::Serialize)]
pub(super) struct ShaderReload {
    pub requested_epoch: u64,
    pub request_unix_ms: u64,
    pub first_new_frame_unix_ms: Option<u64>,
}
#[cfg(feature = "fidelity-lab")]
pub(super) fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
#[derive(Default)]
pub(super) struct State {
    #[cfg(feature = "fidelity-lab")]
    pub shader_reload: Option<ShaderReload>,
    pub camera: Option<CameraView>,
    pub capture: Option<Capture>,
    pub reply: Option<PathBuf>,
    #[cfg(feature = "fidelity-lab")]
    pub param_report: Option<Value>,
    #[cfg(feature = "fidelity-lab")]
    keys: Option<ara_render::lab::Keys>,
    #[cfg(feature = "fidelity-lab")]
    pub temporal: Option<TemporalRequest>,
    #[cfg(feature = "fidelity-lab")]
    pub repaint: bool,
    #[cfg(feature = "fidelity-lab")]
    pub reloading: bool,
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
        let idle = self.options.lab_idle && !self.collecting && self.lab.capture.is_none();
        #[cfg(feature = "fidelity-lab")]
        {
            idle && self.lab.temporal.is_none() && !self.lab.repaint && !self.lab.reloading
        }
        #[cfg(not(feature = "fidelity-lab"))]
        {
            idle
        }
    }
    pub fn apply_camera(&self, frame: &mut RenderFrame) {
        if let Some(camera) = self.lab.camera {
            frame.camera = Some(camera);
        }
    }
    pub fn capture(&mut self, presenter: &mut Presenter, frame: &mut RenderFrame) -> Result<bool> {
        #[cfg(feature = "fidelity-lab")]
        if let Some(request) = self.lab.temporal.take() {
            let result = temporal::run(
                presenter,
                frame,
                &request.directory,
                "view",
                &request.config,
            );
            match result {
                Ok(value) => self.lab.respond(&json!({"temporal":value}))?,
                Err(error) => self.lab.respond(&json!({"error":format!("{error:#}")}))?,
            }
            return Ok(true);
        }
        let Some(capture) = self.lab.capture.take() else {
            return Ok(false);
        };
        let outcome = (|| -> Result<Value> {
            ensure!(
                capture.size.iter().all(|s| (16..=8192).contains(s)),
                "capture resolution outside 16..8192"
            );
            ensure!(
                (1..=3).contains(&capture.runs),
                "capture repeat count must be 1..3"
            );
            let epoch = presenter.shader_status().epoch;
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
                    #[cfg(feature = "fidelity-lab")]
                    self.apply_params(presenter, frame);
                    let began = Instant::now();
                    let image = presenter.capture_presented(frame, capture.size, None, 0)?;
                    let render_ms = began.elapsed().as_secs_f64() * 1000.;
                    #[cfg(feature = "fidelity-lab")]
                    let transform = display::Transform::new(
                        self.lab.look(presenter, frame),
                        frame,
                        &capture.quality,
                    )
                    .map_err(|e| format!("{e:#}"));
                    #[cfg(feature = "fidelity-lab")]
                    let mut repeats = Vec::new();
                    #[cfg(feature = "fidelity-lab")]
                    for _ in 1..capture.runs {
                        repeats.push(
                            presenter
                                .capture_presented(frame, capture.size, None, 0)?
                                .image,
                        );
                    }
                    #[cfg(feature = "fidelity-lab")]
                    let temporal = if capture.temporal {
                        temporal::run(
                            presenter,
                            frame,
                            &capture.directory,
                            &shot.name,
                            &capture.quality["temporal"],
                        )?
                    } else {
                        json!({"status":"not_measured","reason":"quick capture"})
                    };
                    let captured = Captured {
                        image,
                        render_ms,
                        #[cfg(feature = "fidelity-lab")]
                        transform,
                        #[cfg(feature = "fidelity-lab")]
                        repeats,
                        #[cfg(feature = "fidelity-lab")]
                        temporal,
                    };
                    let capture = &capture;
                    analyses.push(scope.spawn(move || capture.analyze(&shot.name, captured)));
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
            ensure!(
                presenter.shader_status().epoch == epoch,
                "shaders changed during capture; retry after publication"
            );
            let region_scores = images
                .iter()
                .filter_map(|image| image["regions"].as_array())
                .flatten()
                .filter_map(|region| region["ldr_secondary"]["flip"].as_f64())
                .collect::<Vec<_>>();
            let mean = (!region_scores.is_empty())
                .then(|| region_scores.iter().sum::<f64>() / region_scores.len() as f64);
            let regions = images
                .iter()
                .filter_map(|i| i["regions"].as_array())
                .flatten()
                .collect::<Vec<_>>();
            let matched = regions
                .iter()
                .filter_map(|r| r["matched_transform"]["flip"].as_f64())
                .collect::<Vec<_>>();
            let matched_mean =
                (!matched.is_empty()).then(|| matched.iter().sum::<f64>() / matched.len() as f64);
            let status = if regions.is_empty() || matched.len() != regions.len() {
                "not_measured"
            } else if regions.iter().any(|r| r["gate"]["status"] == "fail") {
                "fail"
            } else if regions.iter().all(|r| r["gate"]["status"] == "pass") {
                "pass"
            } else {
                "uncalibrated_or_incomplete"
            };
            let temporal_failed = images
                .iter()
                .any(|i| i["temporal"]["static"]["gate"]["status"] == "fail");
            let temporal_complete = images
                .iter()
                .all(|i| i["temporal"]["static"]["gate"]["status"] == "pass");
            let quality_status = if status == "fail" || temporal_failed {
                "fail"
            } else if status == "pass" && temporal_complete {
                "pass"
            } else {
                "not_measured_or_uncalibrated"
            };
            Ok(
                json!({"images":images,"directory":capture.directory,"region_mean_ldr_secondary":mean,"mean_matched_region_flip":matched_mean,"matched_gate":{"status":status,"region_count":regions.len(),"repeat_runs":capture.runs},"quality_gate":{"status":quality_status,"temporal_static_failed":temporal_failed},"shaders":presenter.shader_status()}),
            )
        })();
        match outcome {
            Ok(result) => self.lab.respond(&result)?,
            Err(error) => self.lab.respond(&json!({"error":format!("{error:#}")}))?,
        }
        Ok(true)
    }
}
