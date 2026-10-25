//! Main-thread presentation while the same shipping runtime prepares on a worker.
use super::*;
use ara_runtime::extract::{CameraView, Draw, Geometry};
use ara_runtime_render::{QualityChoice, QualityPreset, RenderOptions, fidelity::native::Content};
use std::{collections::BTreeMap, path::Path, thread, time::Duration};

#[cfg(test)]
mod tests;

pub(super) struct Settings<'a> {
    pub name: &'a str,
    pub size: [u32; 2],
    pub cache: &'a Path,
    pub quality: QualityChoice,
    pub options: RenderOptions,
    pub hidden: bool,
}

pub(super) struct Loaded {
    pub world: World,
    pub build_ms: f64,
    pub window: device::Windowed,
    pub presenter: Presenter,
}

/// Only the prepared Runtime crosses threads; network/SDL owners stay on main.
/// Closing the loading window exits immediately, without waiting on asset I/O.
pub(super) fn load(
    scene: Arc<ara_play::PlayScene>,
    source: Arc<dyn ara_runtime::assets::AssetSource>,
    settings: Settings<'_>,
    began: Instant,
    phases: &mut BTreeMap<&'static str, u128>,
) -> Result<Option<Loaded>> {
    let job = thread::Builder::new()
        .name("ara-player-startup".into())
        .spawn(move || World::prepare_packed(&scene, source))?;
    let open = if settings.hidden {
        device::Windowed::open_offscreen
    } else {
        device::Windowed::open
    };
    let (mut window, mut presenter) = open(
        settings.name,
        settings.size,
        settings.cache,
        if settings.options.ray_tracing {
            QualityChoice::Preset(QualityPreset::Low)
        } else {
            settings.quality
        },
        settings.options,
    )?;
    phases.insert("device_ready_ms", began.elapsed().as_millis());
    presenter.set_probe_gi(false);
    window.title(&format!("{} — Loading scene", settings.name))?;
    let mut loading = loading_frame();
    let mut capture = std::env::var_os("ARA_LOADING_CAPTURE").map(PathBuf::from);
    loop {
        let frame_start = Instant::now();
        if window.events(ara_input::PlayerId(0))?.0 {
            window.shutdown(&presenter)?;
            return Ok(None);
        }
        loading.draws[1].world = glam::Mat4::from_translation(glam::Vec3::new(
            (began.elapsed().as_secs_f32() * 2.).sin() * 0.95,
            0.,
            0.02,
        ));
        window.present(&mut presenter, &loading, || Ok(Content::default()))?;
        phases
            .entry("loading_frame_ms")
            .or_insert_with(|| began.elapsed().as_millis());
        phases
            .entry("first_frame_ms")
            .or_insert_with(|| began.elapsed().as_millis());
        if let Some(path) = capture.take() {
            let request = ara_render::capture::CaptureRequest {
                camera: ara_render::scene::CameraSpec::LookAt {
                    position: [0., 0., 4.].into(),
                    target: [0.; 3].into(),
                    vertical_fov: 50_f32.to_radians(),
                },
                size: settings.size,
                mode: ara_render::capture::ViewMode::Lit,
                deterministic: true,
                exposure: 1.,
                settle_frames: 2,
                tick: 0,
                seed: 0,
                marks: None,
            };
            presenter
                .capture_with(&loading, &request, || Ok(Content::default()))?
                .write(&path)?;
        }
        if job.is_finished() {
            let (prepared, build_ms) = job
                .join()
                .map_err(|_| anyhow::anyhow!("shipping runtime preparation panicked"))??;
            let world = prepared.map_runtime(|runtime| Ok(crate::net::Play::Local(runtime)))?;
            presenter.set_probe_gi(true);
            window.title(settings.name)?;
            window.begin_game();
            return Ok(Some(Loaded {
                world,
                build_ms,
                window,
                presenter,
            }));
        }
        if let Some(wait) = Duration::from_millis(16).checked_sub(frame_start.elapsed()) {
            thread::sleep(wait.min(Duration::from_millis(8)));
        }
    }
}

fn loading_frame() -> RenderFrame {
    let mut frame = RenderFrame::default();
    frame.camera = Some(CameraView {
        position: [0., 0., 4.],
        forward: [0., 0., -1.],
        fov: 50.,
    });
    frame.sun = Some(([0., 0., 1.], [3.; 3]));
    frame.environment.sky.0 = [0.005, 0.008, 0.016, 1.];
    frame.environment.horizon.0 = frame.environment.sky.0;
    frame.environment.ground.0 = frame.environment.sky.0;
    frame.environment.intensity = 1.;
    frame.environment.exposure = 1.;
    for (i, (width, color)) in [(2.4, [0.025, 0.035, 0.05, 1.]), (0.4, [0.1, 0.45, 0.9, 1.])]
        .into_iter()
        .enumerate()
    {
        frame.draws.push(Draw {
            entity: i as u64 + 1,
            address: format!("loading-{i}"),
            label: "Loading scene".into(),
            geometry: Geometry::Shape {
                shape: ara_runtime::components::MeshShape::Box,
                size: [width, 0.06, 0.01],
            },
            world: glam::Mat4::IDENTITY,
            color,
            roughness: 1.,
            metallic: 0.,
            emissive: [0.; 3],
            emissive_factor: None,
            material: None,
            skin: None,
        });
    }
    frame
}
