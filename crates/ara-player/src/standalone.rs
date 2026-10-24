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

#[derive(clap::Args)]
pub(super) struct Options {
    #[command(flatten)]
    perf: perf::Options,
    /// Export folder containing ara-launch.json (defaults to the executable folder).
    #[arg(long)]
    root: Option<PathBuf>,
    /// Run the authored world and capture it without opening a window.
    #[arg(long)]
    headless: bool,
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
    frames: Option<u32>,
}

#[derive(Deserialize)]
struct Launch {
    contract: String,
    pack: String,
    encryption_key_env: Option<String>,
}

pub(super) fn run(args: Args, began: Instant) -> Result<()> {
    let mut phases = std::collections::BTreeMap::new();
    // Include crash-monitor setup and executable hashing in launch latency.
    phases.insert("bootstrap_ms", began.elapsed().as_millis());
    let root = args.standalone.root.as_ref().map_or_else(
        || {
            std::env::current_exe()?
                .parent()
                .map(PathBuf::from)
                .context("player executable folder")
        },
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
    let (mut world, build_ms) = World::build_packed(&scene, source, !args.standalone.headless)?;
    let started = world.start(args.standalone.headless, build_ms)?;
    phases.insert("world_ready_ms", began.elapsed().as_millis());
    let mut frame = RenderFrame::default();
    let mut presenter2d = Presenter2d::new(&args.gpu_cache);
    let (mut window, mut presenter) = if args.standalone.headless {
        (None, Presenter::new(size, Vec::new(), &args.gpu_cache))
    } else {
        let (window, presenter) =
            device::Windowed::open(&args.name, size, &args.gpu_cache, quality)?;
        (Some(window), presenter)
    };
    presenter.set_quality(quality);
    phases.insert("device_ready_ms", began.elapsed().as_millis());
    let mut frame_count = 0_u32;
    let mut frame_sum_ms = 0_f64;
    let mut frame_max_ms = 0_f64;
    let mut frame_phase_ms = [0_f64; 4];
    let mut frame_gpu_ms = 0_f64;
    let mut perf = perf::Window::new(args.standalone.perf);
    if args.standalone.headless {
        world.runtime.step(args.standalone.ticks, &[])?;
        phases.insert("simulation_ready_ms", began.elapsed().as_millis());
    } else if let Some(window) = &mut window {
        let source = world.asset_source()?;
        let simulation = world.runtime.play_mut().session_mut().world_mut();
        let is_2d = ara_runtime_render::two_d::camera(simulation).is_some();
        ensure!(
            !perf.enabled() || !is_2d,
            "perf frame windows require the 3D presenter; a 2D pack cannot supply these GPU pass timings"
        );
        if is_2d {
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
        while args.standalone.frames.is_none_or(|limit| frames < limit) {
            ara_gpu::cpu_scope!("Player.standalone_frame");
            if perf.control(&mut presenter)? {
                break;
            }
            let frame_start = Instant::now();
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
            {
                ara_gpu::cpu_scope!("Player.extract");
                world.runtime.extract(&mut frame);
            }
            let simulation_done = Instant::now();
            if is_2d {
                let tick = world.runtime.tick();
                window.prepare_2d(
                    &mut presenter2d,
                    world.runtime.play_mut().session_mut().world_mut(),
                    tick,
                )?;
                window.present_2d(&mut presenter2d)?;
            } else {
                window.present(&mut presenter, &frame, || world.fidelity_content(&frame))?;
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
                phases.insert("first_frame_ms", began.elapsed().as_millis());
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
            frames += 1;
            #[cfg(feature = "tracy")]
            if let Some(client) = ara_gpu::__tracy::Client::running() {
                client.frame_mark();
            }
            if perf.enabled() {
                let camera = frame.camera.map(|camera| json!({"position":camera.position,"forward":camera.forward,"fov":camera.fov}));
                if perf.frame(
                    &presenter,
                    &phases,
                    json!(camera),
                    [
                        simulation_done.duration_since(frame_start).as_secs_f64() * 1000.,
                        presenter.stats().prepare.as_secs_f64() * 1000.,
                        presentation_done
                            .duration_since(simulation_done)
                            .as_secs_f64()
                            * 1000.,
                        presentation_done.elapsed().as_secs_f64() * 1000.,
                    ],
                    frame_start.elapsed().as_secs_f64() * 1000.,
                )? {
                    break;
                }
            }
        }
        window.shutdown(&presenter)?;
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
            Some(&mut presenter),
            Some(&mut presenter2d),
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
