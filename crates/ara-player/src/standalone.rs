//! Exported games use the Play world, input mapper and window/headless presenters.
use super::{Args, World, device, parse_size, world};
use anyhow::{Context, Result, ensure};
use ara_pack::{EncryptionKey, OwnedPack};
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::{Presenter, two_d::Presenter2d};
use serde::Deserialize;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Instant};
mod perf;
mod startup;
#[cfg(test)]
mod tests;

#[derive(clap::Args)]
pub(super) struct Options {
    #[command(flatten)]
    perf: perf::PerfOptions,
    /// Export folder containing ara-launch.json (defaults to the executable folder).
    #[arg(long)]
    pub(crate) root: Option<PathBuf>,
    /// Run the authored world and capture it without opening a window.
    #[arg(long)]
    pub(crate) headless: bool,
    /// Run continuous presentation on a hidden Vulkan window; perf mode implies this.
    #[arg(long, conflicts_with = "headless")]
    pub(super) offscreen: bool,
    /// Use a resizable game window instead of borderless desktop fullscreen.
    #[arg(long, conflicts_with = "headless")]
    pub(crate) windowed: bool,
    /// Diagnose rendering without swapchain acquisition or presentation; always hidden.
    #[arg(long, requires = "perf_output", conflicts_with = "headless")]
    no_present: bool,
    /// Write a capture after --ticks (headless defaults to verify.png).
    #[arg(long)]
    capture: Option<PathBuf>,
    /// Fixed ticks to simulate before a standalone headless capture.
    #[arg(long, default_value_t = 120)]
    ticks: u32,
    /// Read an entity.Component.field after simulation (repeatable).
    #[arg(long = "read")]
    reads: Vec<String>,
    /// Exit a standalone window after this many frames (unlimited by default).
    #[arg(long)]
    pub(crate) frames: Option<u32>,
}

#[derive(Deserialize)]
struct Launch {
    contract: String,
    pack: String,
    encryption_key_env: Option<String>,
}

impl Options {
    pub(crate) fn opens_menu(&self) -> bool {
        !self.headless && !self.offscreen && !self.perf.requested() && self.frames.is_none()
    }
}

/// UI/swapchain borrowers retire before either creating renderer, including errors.
struct Presentation {
    window: Option<crate::device::Windowed>,
    presenter2d: Presenter2d,
    presenter: Presenter,
}

impl Drop for Presentation {
    fn drop(&mut self) {
        if let Some(window) = &mut self.window {
            if let Err(error) = window.shutdown(&self.presenter) {
                eprintln!("ara-player: retire 3D presentation: {error}");
            }
            if let Err(error) = window.shutdown_2d(&self.presenter2d) {
                eprintln!("ara-player: retire 2D presentation: {error}");
            }
        }
    }
}

pub(super) fn run(args: Args, began: Instant) -> Result<()> {
    let mut perf = perf::Window::new(args.standalone.perf, args.standalone.offscreen, began);
    perf.configure_presentation(args.standalone.no_present, args.standalone.windowed);
    ensure!(
        !args.standalone.headless || !perf.enabled(),
        "perf frame windows need continuous presentation; use --offscreen instead of --headless"
    );
    let mut phases = std::collections::BTreeMap::new();
    // Include crash-monitor setup and executable hashing in launch latency.
    phases.insert("bootstrap_ms", began.elapsed().as_millis());
    let root = args.standalone.root.as_ref().map_or_else(
        || ara_gpu::cooked::install_root().context("player install folder"),
        |root| Ok(root.to_owned()),
    )?;
    let launch: Launch = serde_json::from_slice(&std::fs::read(root.join("ara-launch.json"))?)?;
    ensure!(
        launch.contract == "ara.player.runtime/1",
        "standalone launch needs ara.player.runtime/1"
    );
    ara_pack::validate_path(&launch.pack)?;
    let key = launch
        .encryption_key_env
        .as_deref()
        .map(|name| -> Result<_> {
            Ok(EncryptionKey::from_hex(
                &std::env::var(name).context("pack encryption key is unset")?,
            )?)
        })
        .transpose()?;
    let pack = Arc::new(OwnedPack::open(std::fs::read(root.join(&launch.pack))?)?);
    let mut scene = ara_play::PlayScene::decode(&pack.view().read(
        ".ara/scene.cbor",
        64 * 1024 * 1024,
        key.as_ref(),
    )?)
    .map_err(anyhow::Error::msg)?;
    phases.insert("pack_ms", began.elapsed().as_millis());
    // Runtime paths come from this installation, never the exporting workspace.
    scene.nav_cache = None;
    if let Some(scripts) = &mut scene.scripts {
        ensure!(
            matches!(scripts.host, ara_play::ScriptHost::NativeAot { .. }),
            "ARA0920: exported C# modules require a precompiled shipping host; export the project again"
        );
        scripts.resolve_shipping(&root);
    }
    let source = Arc::new(crate::packed::PackedAssets::new(
        Arc::clone(&pack),
        key,
        &scene,
    )?);
    let quality = args
        .quality
        .unwrap_or_else(|| scene.rendering.quality.into());
    let size = parse_size(&args.size)?;
    let render_options = ara_runtime_render::RenderOptions {
        ray_tracing: args.ray_tracing.unwrap_or(scene.rendering.ray_tracing),
    };
    let scene = Arc::new(scene);
    let (mut world, build_ms, window, presenter) = if args.standalone.headless {
        let (world, build_ms) = World::build_packed(&scene, source, false)?;
        (
            world,
            build_ms,
            None,
            Presenter::new(size, Vec::new(), &args.gpu_cache),
        )
    } else {
        let Some(loaded) = startup::load(
            Arc::clone(&scene),
            source,
            startup::Settings {
                name: &args.name,
                size,
                cache: &args.gpu_cache,
                quality,
                options: render_options,
                hidden: perf.offscreen(),
                window_mode: if args.standalone.windowed {
                    ara_platform::GameWindowMode::Windowed
                } else {
                    ara_platform::GameWindowMode::Borderless
                },
                no_present: args.standalone.no_present,
            },
            began,
            &mut phases,
        )?
        else {
            println!("ARA_RESULT {}", json!({"cancelled":true,"startup":phases}));
            return Ok(());
        };
        (
            loaded.world,
            loaded.build_ms,
            Some(loaded.window),
            loaded.presenter,
        )
    };
    // Keep device borrowers inside the same owner on every return/unwind path.
    let mut presentation = Presentation {
        window,
        presenter2d: Presenter2d::new(&args.gpu_cache),
        presenter,
    };
    let (window, presenter, presenter2d) = (
        &mut presentation.window,
        &mut presentation.presenter,
        &mut presentation.presenter2d,
    );
    let started = world.start(args.standalone.headless, build_ms)?;
    phases.insert("world_ready_ms", began.elapsed().as_millis());
    let mut frame = RenderFrame::default();
    presenter.set_quality(quality);
    presenter.set_render_options(render_options);
    phases
        .entry("device_ready_ms")
        .or_insert_with(|| began.elapsed().as_millis());
    let mut frame_count = 0_u32;
    let mut frame_sum_ms = 0_f64;
    let mut frame_max_ms = 0_f64;
    let mut frame_phase_ms = [0_f64; 4];
    let mut frame_gpu_ms = 0_f64;
    if args.standalone.headless {
        world.runtime.step(args.standalone.ticks, &[])?;
        phases.insert("simulation_ready_ms", began.elapsed().as_millis());
    } else if let Some(window) = window.as_mut() {
        let source = world.asset_source()?;
        let simulation = world.runtime.play_mut().session_mut().world_mut();
        let is_2d = ara_runtime_render::two_d::camera(simulation).is_some();
        ensure!(
            !perf.enabled() || !is_2d,
            "perf frame windows require the 3D presenter; a 2D pack cannot supply these GPU pass timings"
        );
        if is_2d {
            window.shutdown(&*presenter)?;
            presenter2d.set_atlases(ara_runtime_render::two_d::atlases(
                simulation,
                source.as_ref(),
            )?);
        }
        let mut audio = match ara_audio::device::SdlBackend::open(48_000) {
            Ok(device) => Some((world.audio_presenter()?, device)),
            Err(error) => {
                eprintln!("ara-player: no audio: {error}");
                None
            }
        };
        let mut previous = Instant::now();
        let mut frames = 0;
        #[cfg(feature = "hotpatch")]
        let mut rust_patches = ara_hotpatch::Hotpatch::connect();
        while args.standalone.frames.is_none_or(|limit| frames < limit) {
            #[cfg(feature = "hotpatch")]
            rust_patches.frame_boundary();
            ara_gpu::cpu_scope!("Player.standalone_frame");
            if perf.control(&mut *presenter)? {
                break;
            }
            let frame_start = Instant::now();
            let waits_before = if perf.enabled() {
                presenter.host_wait_stats()
            } else {
                None
            };
            let (quit, inputs) = window.events(world.runtime.player())?;
            if quit {
                break;
            }
            if !inputs.is_empty() {
                world.runtime.input(0, inputs)?;
            }
            let now = Instant::now();
            {
                ara_gpu::cpu_scope!("Player.advance");
                world.runtime.advance(now.duration_since(previous))?;
            }
            previous = now;
            world.mark();
            window.update_game_ui(world.ui.as_ref(), &world.runtime, world.runtime.player().0)?;
            {
                ara_gpu::cpu_scope!("Player.extract");
                world.runtime.extract(&mut frame);
            }
            let simulation_done = Instant::now();
            if is_2d {
                let tick = world.runtime.tick();
                window.prepare_2d(
                    &mut *presenter2d,
                    world.runtime.play_mut().session_mut().world_mut(),
                    tick,
                )?;
                window.present_2d(&mut *presenter2d)?;
            } else {
                window.present(&mut *presenter, &frame, || world.fidelity_content(&frame))?;
            }
            let presentation_done = Instant::now();
            if let Some((audio, device)) = &mut audio {
                ara_gpu::cpu_scope!("Player.audio");
                let inputs = world.audio_inputs()?;
                world::update_audio(
                    audio,
                    world.runtime.world(),
                    &world::listener(&frame),
                    &inputs,
                )?;
                audio.pump(device, super::AUDIO_WATERMARK)?;
            }
            if frames == 0 {
                phases
                    .entry(if args.standalone.no_present {
                        "first_rendered_frame_ms"
                    } else {
                        "first_frame_ms"
                    })
                    .or_insert_with(|| began.elapsed().as_millis());
            } else {
                let elapsed = frame_start.elapsed().as_secs_f64() * 1000.;
                frame_count += 1;
                frame_sum_ms += elapsed;
                frame_max_ms = frame_max_ms.max(elapsed);
                frame_phase_ms[0] +=
                    simulation_done.duration_since(frame_start).as_secs_f64() * 1000.;
                frame_phase_ms[1] += presenter.stats().prepare.as_secs_f64() * 1000.;
                frame_phase_ms[2] += presentation_done
                    .duration_since(simulation_done)
                    .as_secs_f64()
                    * 1000.;
                frame_phase_ms[3] += frame_start.elapsed().as_secs_f64() * 1000.
                    - presentation_done.duration_since(frame_start).as_secs_f64() * 1000.;
                frame_gpu_ms += presenter.stats().gpu_ms;
            }
            if is_2d || presenter.presentation_error().is_none() {
                phases
                    .entry(if args.standalone.no_present {
                        "full_scene_rendered_ms"
                    } else {
                        "full_scene_frame_ms"
                    })
                    .or_insert_with(|| began.elapsed().as_millis());
            }
            frames += 1;
            #[cfg(feature = "tracy")]
            if let Some(client) = ara_gpu::__tracy::Client::running() {
                client.frame_mark();
            }
            if perf.enabled() {
                let camera = frame.camera.map(|camera| json!({"position":camera.position,"forward":camera.forward,"fov":camera.fov}));
                let stop = perf.frame(
                    &*presenter,
                    &phases,
                    json!(camera),
                    perf::CpuFrame {
                        phases: [
                            simulation_done.duration_since(frame_start).as_secs_f64() * 1000.,
                            presenter.stats().prepare.as_secs_f64() * 1000.,
                            presentation_done
                                .duration_since(simulation_done)
                                .as_secs_f64()
                                * 1000.,
                            presentation_done.elapsed().as_secs_f64() * 1000.,
                        ],
                        wait_ms: if is_2d {
                            None
                        } else {
                            presenter
                                .host_wait_stats()
                                .and_then(|after| after.elapsed_since(waits_before))
                                .map(|elapsed| elapsed.as_secs_f64() * 1000.)
                        },
                        total_ms: frame_start.elapsed().as_secs_f64() * 1000.,
                        first_presented: frames == 1 && !args.standalone.no_present,
                    },
                )?;
                if stop {
                    break;
                }
            }
        }
        window.shutdown(&*presenter)?;
        perf.finish()?;
    }
    let capture = args
        .standalone
        .capture
        .or_else(|| args.standalone.headless.then(|| root.join("verify.png")));
    let result = if let Some(path) = capture {
        Some(world.capture(
            &ara_play::CaptureParams {
                path: Some(path.to_string_lossy().into_owned()),
                width: size[0],
                height: size[1],
                marks: 0,
                presented: true,
            },
            Some(&mut *presenter),
            Some(&mut *presenter2d),
            &mut frame,
        )?)
    } else {
        None
    };
    ensure!(
        presenter.presentation_error().is_none(),
        "standalone presentation failed: {:?}",
        presenter.presentation_error()
    );
    let reads = args
        .standalone
        .reads
        .iter()
        .map(|path| {
            Ok((
                path,
                world::read_path(world.runtime.play().session(), path)?,
            ))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
    println!(
        "ARA_RESULT {}",
        json!({"verified":true,"template_contract":launch.contract,
        "scene":scene.scene,"entities":started.entities,"tick":world.runtime.tick(),
        "capture":result,"reads":reads,"quality":presenter.quality().map(|value| value.name()),
        "gpu_ms":presenter.stats().gpu_ms,"elapsed_ms":began.elapsed().as_millis(),
        "startup":phases,"frames_after_first":frame_count,
        "fps_after_first":if frame_sum_ms > 0. { Some(f64::from(frame_count)*1000./frame_sum_ms) } else { None },
        "mean_frame_ms_after_first":if frame_count>0 {Some(json!({
            "simulation_and_extraction":frame_phase_ms[0]/f64::from(frame_count),
            "presentation_prepare":frame_phase_ms[1]/f64::from(frame_count),
            "presentation_including_display":frame_phase_ms[2]/f64::from(frame_count),
            "audio":frame_phase_ms[3]/f64::from(frame_count),
            "gpu":frame_gpu_ms/f64::from(frame_count)
        }))} else {None},
        "max_frame_ms_after_first":if frame_count>0 {Some(frame_max_ms)} else {None}})
    );
    Ok(())
}
