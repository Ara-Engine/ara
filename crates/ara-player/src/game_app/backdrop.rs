//! A native art-only world supplies the live menu's animated dusk vista.
use super::*;
use ara_runtime::extract::CameraView;
use ara_sim::Identity;
use glam::Vec3;

pub(super) struct Backdrop {
    pub world: World,
    eye: Vec3,
    target: Vec3,
    phase: f32,
}
pub(super) struct Prepared {
    world: crate::world::World<ara_runtime::Runtime>,
    eye: Vec3,
    target: Vec3,
}

impl Backdrop {
    fn load(root: &std::path::Path, project: bool) -> Result<Prepared> {
        let mut content = if project {
            content::project(root, |_| Ok(()))?
        } else {
            content::exported(root)?
        };
        let menu = content
            .scene
            .documents
            .iter()
            .find(|doc| doc.path == "levels/menu.scene")
            .context("authored native menu scene is missing")?;
        content.scene.scene = menu.id.clone();
        content.scene.scripts = None;
        content.scene.ui = None;
        let (mut world, _) = World::prepare_game(&content.scene, None, content.source)?;
        world.runtime.resume()?;
        let point = |identity: &str| {
            world
                .runtime
                .world()
                .iter_entities()
                .find_map(|entity| {
                    (entity.get::<Identity>()?.0 == identity)
                        .then(|| {
                            entity
                                .get::<ara_components::Transform>()
                                .map(|t| Vec3::from_array(t.position))
                        })
                        .flatten()
                })
                .with_context(|| format!("menu viewpoint {identity} is missing"))
        };
        let eye = point("e_gamemenueye")?;
        let target = point("e_gamemenutarget")?;
        Ok(Prepared { world, eye, target })
    }

    pub(super) fn frame(&mut self, frame: &mut RenderFrame, elapsed: Duration) -> Result<()> {
        self.phase += elapsed.as_secs_f32().min(0.1) * 0.07;
        self.world.runtime.advance(elapsed)?;
        self.world.extract_frame(frame);
        let eye = self.eye + Vec3::new(self.phase.sin() * 1.4, (self.phase * 0.7).sin() * 0.18, 0.);
        frame.camera = Some(CameraView {
            position: eye.to_array(),
            forward: (self.target - eye).normalize().to_array(),
            fov: 54.,
        });
        Ok(())
    }
}

impl App {
    pub(super) fn start_backdrop(&mut self) -> Result<()> {
        let root = self.root.clone();
        let project = self.project;
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("outpost-vista".into())
            .spawn(move || {
                let _ = send.send(Backdrop::load(&root, project));
            })?;
        self.backdrop_loading = Some(receive);
        Ok(())
    }

    pub(super) fn poll_backdrop(&mut self) -> Result<()> {
        let Some(receive) = &self.backdrop_loading else {
            return Ok(());
        };
        let result = match receive.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        self.backdrop_loading = None;
        let prepared = result?;
        let mut backdrop = Backdrop {
            world: prepared
                .world
                .map_runtime(|runtime| Ok(Play::Local(runtime)))?,
            eye: prepared.eye,
            target: prepared.target,
            phase: 0.,
        };
        backdrop.frame(&mut self.frame, Duration::ZERO)?;
        self.backdrop = Some(backdrop);
        if let Some(action) = self.bot_start.take() {
            if let Some(directory) = self.evidence.clone() {
                self.capture_menu(&directory.join("main-menu.png"))?;
            }
            self.click(&action)?;
        }
        Ok(())
    }

    fn capture_menu(&mut self, path: &std::path::Path) -> Result<()> {
        let size = [1920, 1080];
        let overlay = {
            let surface = self.surface().context("main menu surface")?;
            surface.resize(size, 1.)?;
            surface.screen(Screen::Main)?;
            surface.image()?
        };
        let backdrop = self.backdrop.as_ref().context("menu vista")?;
        let content = backdrop.world.fidelity_content(&self.frame)?;
        let mut capture = self.presenter.capture_with(
            &self.frame,
            &ara_render::capture::CaptureRequest {
                camera: ara_runtime_render::camera(&self.frame)?,
                size,
                mode: ara_render::capture::ViewMode::Lit,
                deterministic: true,
                exposure: self.frame.environment.exposure,
                settle_frames: 24,
                tick: 0,
                seed: 0,
                marks: None,
            },
            || Ok(content),
        )?;
        crate::game_ui::composite(&mut capture.image, &overlay);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        capture.write(path)?;
        Ok(())
    }
}
