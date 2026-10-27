//! Benchmark-only camera and capture adapter; no rendering algorithm changes.
use anyhow::{Context, Result, ensure};
use ara_runtime::extract::{CameraView, RenderFrame};
use ara_runtime_render::Presenter;
use serde::Deserialize;
use std::{
    io::Write,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    time::Instant,
};

#[derive(Deserialize)]
struct View {
    position: [f32; 3],
    target: [f32; 3],
    fov: f32,
    #[serde(default)]
    name: String,
}
#[derive(Deserialize)]
struct Manifest {
    shots: Vec<View>,
    path: Vec<View>,
}
pub(super) struct Benchmark {
    manifest: Option<Manifest>,
    mode: String,
    output: PathBuf,
    size: [u32; 2],
    started: Option<Instant>,
    frame: usize,
    shot: usize,
    encoder: Option<(Child, ChildStdin)>,
}
impl Benchmark {
    pub fn load(size: [u32; 2]) -> Result<Self> {
        let manifest = std::env::var_os("ARA_BENCH_MANIFEST")
            .map(|p| -> Result<Manifest> { Ok(serde_json::from_slice(&std::fs::read(p)?)?) })
            .transpose()?;
        Ok(Self {
            manifest,
            mode: std::env::var("ARA_BENCH_MODE").unwrap_or_else(|_| "timing".into()),
            output: std::env::var_os("ARA_BENCH_OUTPUT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".")),
            size,
            started: None,
            frame: 0,
            shot: 0,
            encoder: None,
        })
    }
    pub fn fixed_step(&self) -> bool {
        self.manifest.is_some() && self.mode == "video"
    }
    pub fn camera(&mut self, frame: &mut RenderFrame, sampling: bool) {
        let Some(manifest) = &self.manifest else {
            return;
        };
        if sampling {
            self.started.get_or_insert_with(Instant::now);
        }
        let cursor = if self.mode == "video" {
            self.frame.saturating_sub(180) as f64
        } else {
            self.started.map_or(0., |s| s.elapsed().as_secs_f64() * 60.)
        };
        let index = (cursor as usize).min(manifest.path.len() - 1);
        let view = if self.mode == "shots" {
            &manifest.shots[self.shot.min(manifest.shots.len() - 1)]
        } else {
            &manifest.path[index]
        };
        let next = if self.mode == "timing" {
            &manifest.path[(index + 1).min(manifest.path.len() - 1)]
        } else {
            view
        };
        let fraction = if self.mode == "timing" {
            cursor.fract() as f32
        } else {
            0.
        };
        let p = glam::Vec3::from_array(view.position)
            .lerp(glam::Vec3::from_array(next.position), fraction);
        let target =
            glam::Vec3::from_array(view.target).lerp(glam::Vec3::from_array(next.target), fraction);
        let f = (target - p).normalize();
        frame.camera = Some(CameraView {
            position: p.to_array(),
            forward: f.to_array(),
            fov: view.fov + (next.fov - view.fov) * fraction,
        });
    }
    pub fn capture(&mut self, presenter: &mut Presenter, frame: &RenderFrame) -> Result<bool> {
        if self.manifest.is_none() || self.mode == "timing" {
            return Ok(false);
        }
        self.frame += 1;
        if self.mode == "shots" && self.frame % 120 == 0 {
            std::fs::create_dir_all(&self.output)?;
            let manifest = self.manifest.as_ref().context("benchmark manifest")?;
            let capture = presenter.capture_presented(frame, self.size, None, 0)?;
            capture.image.save(
                self.output
                    .join(format!("{}.png", manifest.shots[self.shot].name)),
            )?;
            self.shot += 1;
            return Ok(self.shot == manifest.shots.len());
        }
        if self.mode == "video" && self.frame > 180 {
            if self.encoder.is_none() {
                let parent = self.output.parent().context("video parent")?;
                std::fs::create_dir_all(parent)?;
                let dimensions = format!("{}x{}", self.size[0], self.size[1]);
                let mut process = Command::new("ffmpeg")
                    .args([
                        "-y",
                        "-v",
                        "warning",
                        "-f",
                        "rawvideo",
                        "-pixel_format",
                        "rgba",
                        "-video_size",
                        &dimensions,
                        "-framerate",
                        "60",
                        "-i",
                        "pipe:0",
                        "-an",
                        "-c:v",
                        "hevc_nvenc",
                        "-preset",
                        "p6",
                        "-cq",
                        "18",
                        "-pix_fmt",
                        "yuv420p",
                        "-tag:v",
                        "hvc1",
                    ])
                    .arg(&self.output)
                    .stdin(Stdio::piped())
                    .spawn()?;
                let stdin = process.stdin.take().context("ffmpeg stdin")?;
                self.encoder = Some((process, stdin));
            }
            let capture = presenter.capture_presented(frame, self.size, None, 0)?;
            self.encoder
                .as_mut()
                .context("video encoder")?
                .1
                .write_all(capture.image.as_raw())?;
            if self.frame >= 1380 {
                let (mut process, stdin) = self.encoder.take().context("video encoder")?;
                drop(stdin);
                ensure!(process.wait()?.success(), "ffmpeg failed");
                return Ok(true);
            }
        }
        Ok(false)
    }
}
