//! Embedded Play (D19) end to end: the demo starts with `embed` through the play
//! service, the test plays the editor (the in-process `Surfaces` handle, a second
//! Vulkan device importing every surface generation), reads frames back and checks
//! they show the scene, drives the Fox with raw device input, resizes, and stops.
//! Prints the frame latency and the player's per-frame cost.
#![allow(clippy::unwrap_used)]
mod support;

use ara_gpu::{
    shared::Frame,
    vulkan::{VulkanDevice, external::ImportedSurface},
};
use ara_play_host::embed::{DeviceEvent, Surfaces};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use support::{Agent, demo_copy};

/// What the test's "editor" has imported and shows for player 0.
struct View {
    imported: Option<ImportedSurface>,
    shown: Option<Frame>,
}

impl View {
    /// Polls the service and shows the newest frame of player 0 (importing a new
    /// generation first); returns the frame newly shown.
    fn pump(
        &mut self,
        agent: &mut Agent,
        surfaces: &mut Surfaces,
        world: &str,
        gpu: &VulkanDevice,
    ) -> Option<Frame> {
        agent.host.tick();
        assert!(surfaces.poll(), "the play service is gone");
        let state = surfaces
            .surfaces(world)
            .into_iter()
            .find(|s| s.player == 0)?;
        if self.imported.as_ref().map(ImportedSurface::generation) != Some(state.generation) {
            // A new generation: the old import (and the frame it showed) is dropped;
            // the player's old ring no longer exists, so nothing is released.
            self.shown = None;
            self.imported = None;
            // SAFETY: The device outlives the import; this thread owns its queue.
            self.imported =
                Some(unsafe { ImportedSurface::new(gpu.context(), state.surface).unwrap() });
        }
        let frame = surfaces.take(world, 0)?;
        let imported = self.imported.as_mut()?;
        assert_eq!(frame.generation, imported.generation());
        // SAFETY: Reads of the previous frame completed (every run waits).
        unsafe {
            imported
                .exchange(Some(frame.slot), self.shown.map(|f| f.slot))
                .unwrap();
        }
        if let Some(previous) = self.shown.replace(frame) {
            surfaces.release(world, 0, previous);
        }
        Some(frame)
    }

    fn pixels(&mut self) -> image::RgbaImage {
        let frame = self.shown.unwrap();
        let imported = self.imported.as_mut().unwrap();
        let [w, h] = imported.size();
        // SAFETY: The shown slot is acquired; this thread owns the queue.
        let bytes = unsafe { imported.read(frame.slot).unwrap() };
        image::RgbaImage::from_raw(w, h, bytes).unwrap()
    }
}

fn percentile(samples: &mut [u64], p: f64) -> u64 {
    samples.sort_unstable();
    samples
        .get(((samples.len() as f64 * p) as usize).min(samples.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0)
}

/// Mean absolute RGB difference of two images reduced to 32x18 blocks, in 0..1.
fn coarse_difference(a: &image::RgbaImage, b: &image::RgbaImage) -> f64 {
    let small = |img: &image::RgbaImage| {
        image::imageops::resize(img, 32, 18, image::imageops::FilterType::Triangle)
    };
    let (a, b) = (small(a), small(b));
    let total: u64 = a
        .pixels()
        .zip(b.pixels())
        .map(|(p, q)| {
            (0..3)
                .map(|c| u64::from(p.0[c].abs_diff(q.0[c])))
                .sum::<u64>()
        })
        .sum();
    total as f64 / (32.0 * 18.0 * 3.0 * 255.0)
}

fn luma_stats(img: &image::RgbaImage) -> (f64, f64) {
    let luma: Vec<f64> = img
        .pixels()
        .map(|p| {
            0.2126 * f64::from(p.0[0]) + 0.7152 * f64::from(p.0[1]) + 0.0722 * f64::from(p.0[2])
        })
        .collect();
    let mean = luma.iter().sum::<f64>() / luma.len() as f64;
    let variance = luma.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / luma.len() as f64;
    (mean, variance.sqrt())
}

#[test]
#[ignore = "needs a Vulkan GPU with VK_KHR_external_memory_win32; run explicitly with --ignored"]
fn embedded_play_shares_frames_with_the_owner_device() {
    let dir = demo_copy("embed");
    let mut surfaces = None;
    let mut agent = Agent::open_with(
        &dir,
        5,
        |config| config.pool.headless = 1,
        |service| surfaces = Some(service.surfaces()),
    );
    let mut surfaces = surfaces.unwrap();
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sn/test-gpu");
    let gpu = VulkanDevice::new(&[], cache).unwrap();
    agent.until("a warm pool", |a| {
        a.play("status", json!({}))["pool"]["headless"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    });

    let t = Instant::now();
    let started = agent.play(
        "start",
        json!({"embed": {"width": 640, "height": 360}, "seed": 5}),
    );
    let boot_ms = t.elapsed().as_secs_f64() * 1000.0;
    let world = started["world"].as_str().unwrap().to_owned();
    let surface = &started["players"][0]["surface"];
    assert_eq!(surface["generation"], 1, "{started}");
    assert_eq!(
        (surface["width"].clone(), surface["height"].clone()),
        (json!(640), json!(360))
    );
    assert_eq!(
        surface["process"],
        json!(std::process::id()),
        "handles live in the owner"
    );
    assert_eq!(surface["slots"].as_array().unwrap().len(), 3);

    // At least 60 frames, imported on this process's own device.
    let mut view = View {
        imported: None,
        shown: None,
    };
    let mut frames = 0;
    let mut ticks = Vec::new();
    let first = Instant::now();
    while frames < 90 {
        assert!(
            first.elapsed() < Duration::from_secs(60),
            "only {frames} frames"
        );
        match view.pump(&mut agent, &mut surfaces, &world, &gpu) {
            Some(frame) => {
                frames += 1;
                ticks.push(frame.tick);
                if frames % 30 == 0 {
                    let (mean, spread) = luma_stats(&view.pixels());
                    assert!(
                        mean > 8.0 && spread > 4.0,
                        "frame {frames}: black or flat ({mean}, {spread})"
                    );
                }
            }
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    let frames_s = frames as f64 / first.elapsed().as_secs_f64();
    assert!(
        ticks.windows(2).all(|w| w[0] <= w[1]),
        "frames arrive in tick order"
    );

    // Raw input drives the Fox like a windowed player's keyboard: W held, then released.
    let read_z = |agent: &mut Agent| {
        agent.play(
            "read",
            json!({"world": world, "paths": ["e_fox.Transform.position.z"]}),
        )["values"]["e_fox.Transform.position.z"]
            .as_f64()
            .unwrap()
    };
    let z0 = read_z(&mut agent);
    surfaces.device(
        &world,
        0,
        vec![DeviceEvent::Key {
            scancode: 26,
            down: true,
        }],
    );
    let walk = Instant::now();
    while walk.elapsed() < Duration::from_millis(1200) {
        view.pump(&mut agent, &mut surfaces, &world, &gpu);
        std::thread::sleep(Duration::from_millis(2));
    }
    // The JSON method reaches exactly one player too (agents, MCP).
    let accepted = agent.play(
        "device",
        json!({"world": world, "player": 0, "events": [{"kind": "key", "scancode": 26, "down": false}, {"kind": "focusLost"}]}),
    );
    assert_eq!(accepted["accepted"], 2);
    let z1 = read_z(&mut agent);
    assert!(z1 > z0 + 0.5, "W walked the Fox forward: {z0} -> {z1}");

    // Paused, the live frame and a presented capture of the same tick show the same scene.
    agent.play("pause", json!({"world": world}));
    let mut settled = 0;
    while settled < 8 {
        if view.pump(&mut agent, &mut surfaces, &world, &gpu).is_some() {
            settled += 1;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let live = view.pixels();
    let evidence = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/presenter-demo");
    std::fs::create_dir_all(&evidence).unwrap();
    live.save(evidence.join("game-shared-surface.png")).unwrap();
    let capture_path = evidence.join("game-capture.png");
    agent.play(
        "capture",
        json!({"world": world, "path": capture_path.to_string_lossy(), "width": 640, "height": 360, "presented": true, "marks": 0}),
    );
    let captured = image::open(&capture_path).unwrap().into_rgba8();
    let difference = coarse_difference(&live, &captured);
    let ssim = ara_render::capture::compare(&live, &captured, 1.0)
        .unwrap()
        .score;
    assert!(
        difference < 0.08,
        "live frame differs from the capture: {difference}"
    );
    agent.play("resume", json!({"world": world}));

    // Resize through the typed API: a new generation at the new size.
    surfaces.resize(&world, 0, 320, 180);
    let resized = Instant::now();
    loop {
        assert!(
            resized.elapsed() < Duration::from_secs(20),
            "no frame after the resize"
        );
        if let Some(frame) = view.pump(&mut agent, &mut surfaces, &world, &gpu)
            && frame.generation == 2
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let resize_ms = resized.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(view.pixels().dimensions(), (320, 180));
    // And through JSON.
    let json_resize = agent.play(
        "resize",
        json!({"world": world, "player": 0, "width": 400, "height": 240}),
    );
    assert_eq!(json_resize["surface"]["generation"], 3, "{json_resize}");
    loop {
        if let Some(frame) = view.pump(&mut agent, &mut surfaces, &world, &gpu)
            && frame.generation == 3
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(view.pixels().dimensions(), (400, 240));

    let status = agent.play("status", json!({}));
    let player_status: Value = status["worlds"][0]["players"][0].clone();
    assert_eq!(player_status["surface"]["generation"], 3);

    // Stop releases everything: the handle forgets the world.
    agent.play("stop", json!({"world": world}));
    view.imported = None;
    surfaces.poll();
    assert!(surfaces.surfaces(&world).is_empty());
    gpu.assert_clean().unwrap();

    let timings = surfaces.take_timings();
    let mut latency: Vec<u64> = timings.iter().map(|t| t.latency_us).collect();
    let mut cost: Vec<u64> = timings.iter().map(|t| t.cost_us).collect();
    let mut publish: Vec<u64> = timings.iter().map(|t| t.publish_us).collect();
    println!(
        "EMBED {}",
        serde_json::to_string_pretty(&json!({
            "bootFromPoolMs": boot_ms,
            "framesPerSecond": frames_s,
            "frames": timings.len(),
            "renderToOwnerLatencyUs": {"p50": percentile(&mut latency, 0.5), "p95": percentile(&mut latency, 0.95)},
            "playerFrameCostUs": {"p50": percentile(&mut cost, 0.5), "p95": percentile(&mut cost, 0.95)},
            "copyAndFenceUs": {"p50": percentile(&mut publish, 0.5), "p95": percentile(&mut publish, 0.95)},
            "resizeToFirstFrameMs": resize_ms,
            "liveVsCapture": {"coarseDifference": difference, "oneMinusSsim": ssim},
            "fox": [z0, z1],
            "status": player_status["status"],
        }))
        .unwrap()
    );
    drop(agent);
    // The player's side of the protocol ran under validation too (debug builds).
    for log in std::fs::read_dir(dir.join(".ara/play")).unwrap().flatten() {
        let text = std::fs::read_to_string(log.path()).unwrap_or_default();
        assert!(
            !text.contains("VALIDATION ERROR") && !text.contains("VALIDATION WARNING"),
            "{}: {text}",
            log.path().display()
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
