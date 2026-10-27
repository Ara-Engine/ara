//! The Hub template and editor's real shared-surface path, including broken assets.
use super::*;
use ara_audio::codec::{MediaDecoder, decode_clip};
use ara_hub_service::{DirectoryTemplates, TemplateSource};

#[test]
#[ignore = "requires fetched Outpost assets and a Vulkan external-memory GPU; offscreen only"]
fn hub_outpost_embedded_hud_survives_thirty_seconds_and_missing_images() {
    let repo = support::repo();
    let evidence = repo
        .join("target/ara-player/outpost-embedded")
        .join(format!("run-{}", std::process::id()));
    let root = evidence.join("Hub Outpost project");
    std::fs::create_dir_all(&root).unwrap();
    DirectoryTemplates(repo.join("samples"))
        .instantiate("outpost", &root)
        .unwrap();
    let initial = ara_play_host::read_game_ui(&root).unwrap().unwrap();
    assert!(
        initial.resource_diagnostics.is_empty(),
        "{:?}",
        initial.resource_diagnostics
    );
    let reticle = "assets/third_party/outpost_ui/PNG/Extra/Default/crosshair_a.png";
    assert_eq!(initial.resource_paths["image:reticle"], reticle);
    assert_eq!(
        initial.images["reticle"],
        std::fs::read(root.join(reticle)).unwrap()
    );
    assert_eq!(
        initial.fonts[0],
        std::fs::read(root.join(&initial.resource_paths["font:0"])).unwrap()
    );
    assert_eq!(
        initial.click,
        std::fs::read(root.join(&initial.resource_paths["click"])).unwrap()
    );
    let mut decoder =
        MediaDecoder::open(Box::new(std::io::Cursor::new(initial.click.clone())), "ogg").unwrap();
    decode_clip(&mut decoder, 1024 * 1024).unwrap();
    assert_eq!(
        (
            initial.music[0].entity.as_str(),
            initial.music[0].component.as_str(),
            initial.music[0].field.as_str(),
            initial.music[0].target.as_str()
        ),
        ("e_run", "SalvageRun", "alert", "e_music")
    );
    assert_eq!(initial.sounds.len(), 4);

    let mut surfaces = None;
    let mut agent = Agent::open_with(
        &root,
        5,
        |config| {
            config.pool.headless = 1;
        },
        |service| surfaces = Some(service.surfaces()),
    );
    let mut surfaces = surfaces.unwrap();
    let gpu = GpuDevice::new(&[], evidence.join("owner-gpu")).unwrap();
    for (stage, seconds) in [("valid", 30), ("missing", 5), ("corrupt", 5)] {
        match stage {
            "missing" => std::fs::remove_file(root.join(reticle)).unwrap(),
            "corrupt" => std::fs::write(root.join(reticle), b"not a PNG").unwrap(),
            _ => {}
        }
        agent.until("a warm embedded player", |agent| {
            agent.play("status", json!({}))["pool"]["headless"]
                .as_u64()
                .unwrap_or(0)
                > 0
        });
        let started = agent.play(
            "start",
            json!({"embed":{"width":1280,"height":720,"maxFps":30},"seed":5}),
        );
        let world = started["world"].as_str().unwrap().to_owned();
        let mut view = View {
            imported: None,
            shown: None,
        };
        let began = Instant::now();
        let mut visible = None;
        let mut frames = 0;
        let mut first_tick = None;
        let last_tick = loop {
            assert!(
                began.elapsed() < Duration::from_secs(seconds + 120),
                "{stage}: no live HUD"
            );
            if let Some(frame) = view.pump(&mut agent, &mut surfaces, &world, &gpu) {
                first_tick.get_or_insert(frame.tick);
                frames += 1;
                let since = *visible.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_secs(seconds) {
                    break frame.tick;
                }
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        assert!(frames >= seconds * 3, "{stage}: only {frames} frames");
        assert!(
            last_tick > first_tick.unwrap(),
            "{stage}: simulation stopped"
        );
        let image = view.pixels(&gpu);
        let (mean, spread) = luma_stats(&image);
        assert!(mean > 8. && spread > 4., "{stage}: black or flat image");
        image
            .save(evidence.join(format!("{stage}-hud.png")))
            .unwrap();
        let events = agent.play("events", json!({"world":world}));
        let resource_events: Vec<_> = events["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event.to_string().contains("ARA4051"))
            .collect();
        if stage == "valid" {
            assert!(resource_events.is_empty(), "{events}");
            let audio = agent.play("captureAudio", json!({"world":world,"path":evidence.join("music.wav").to_string_lossy(),"seconds":1.0}));
            assert_eq!(audio["analysis"]["silent"], false, "{audio}");
        } else {
            assert!(
                resource_events
                    .iter()
                    .any(|event| event.to_string().contains(reticle)),
                "{events}"
            );
            assert!(
                resource_events.len() <= 3,
                "resource diagnostics must not flood every frame: {events}"
            );
        }
        println!(
            "EMBEDDED_OUTPOST {stage}: {frames} frames, {} seconds visible, ticks {}..{last_tick}, {}",
            visible.unwrap().elapsed().as_secs_f64(),
            first_tick.unwrap(),
            evidence.display()
        );
        agent.play("stop", json!({"world":world}));
        view.imported = None;
        surfaces.poll();
    }
    drop(agent);
    gpu.assert_clean().unwrap();
}
