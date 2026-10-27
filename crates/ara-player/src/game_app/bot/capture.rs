//! Readbacks own frozen presentation data; UDP and simulation keep polling.
use super::*;
use std::{fs::File, path::Path, sync::mpsc};

struct Request {
    frame: RenderFrame,
    content: ara_runtime_render::fidelity::native::Content,
    overlay: Option<image::RgbaImage>,
    path: PathBuf,
    stage: String,
    seed: u64,
    _lock: File,
}

pub(super) struct Worker {
    send: mpsc::Sender<Request>,
    receive: mpsc::Receiver<Result<String>>,
    pending: bool,
}
impl Worker {
    pub(super) fn new(cache: PathBuf) -> Result<Self> {
        let (send, requests) = mpsc::channel::<Request>();
        let (results, receive) = mpsc::channel();
        std::thread::Builder::new()
            .name("outpost-capture".into())
            .spawn(move || {
                let mut presenter = Presenter::new([1920, 1080], vec![], cache);
                presenter.set_quality(ara_runtime_render::QualityChoice::Preset(
                    ara_runtime_render::QualityPreset::Low,
                ));
                for request in requests {
                    let result = (|| {
                        let mut capture = presenter.capture_with(
                            &request.frame,
                            &ara_render::capture::CaptureRequest {
                                camera: ara_runtime_render::camera(&request.frame)?,
                                size: [1920, 1080],
                                mode: ara_render::capture::ViewMode::Lit,
                                deterministic: true,
                                exposure: request.frame.environment.exposure,
                                settle_frames: 16,
                                tick: request.frame.tick,
                                seed: request.seed,
                                marks: None,
                            },
                            || Ok(request.content),
                        )?;
                        if let Some(overlay) = request.overlay {
                            crate::game_ui::composite(&mut capture.image, &overlay);
                        }
                        capture.write(&request.path)?;
                        Ok(request.stage)
                    })();
                    if results.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            send,
            receive,
            pending: false,
        })
    }
    pub(super) fn pending(&self) -> bool {
        self.pending
    }
    pub(super) fn completed(&mut self) -> Result<Option<String>> {
        match self.receive.try_recv() {
            Ok(result) => {
                self.pending = false;
                result.map(Some)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                anyhow::bail!("offscreen capture worker exited")
            }
        }
    }
    pub(super) fn start(
        &mut self,
        world: &mut World,
        frame: &mut RenderFrame,
        player: u64,
        stage: &str,
        directory: &Path,
        lock: File,
    ) -> Result<()> {
        world.extract_frame(frame);
        let content = world.fidelity_content(frame)?;
        let overlay = world
            .ui
            .as_ref()
            .map(|ui| crate::game_ui::snapshot(ui, &world.runtime, player, [1920, 1080]))
            .transpose()?;
        self.send
            .send(Request {
                frame: std::mem::take(frame),
                content,
                overlay,
                path: directory.join(format!("player-{player}-{stage}.png")),
                stage: stage.into(),
                seed: world.runtime.play().session().context().seed(),
                _lock: lock,
            })
            .map_err(|_| anyhow::anyhow!("offscreen capture worker exited"))?;
        self.pending = true;
        Ok(())
    }
}
