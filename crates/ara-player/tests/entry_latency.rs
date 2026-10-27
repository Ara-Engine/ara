//! Opt-in real-project Play entry probe. Run the player in release/perf with
//! ARA_BENCH_PLAYER; assets and documents are copied under target before timing.
#![allow(clippy::unwrap_used)]
mod support;

use serde_json::json;
use std::{path::PathBuf, time::Instant};
use support::{Agent, copy};

fn sidecar_count(directory: &std::path::Path) -> usize {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                0
            } else if entry.path().is_dir() {
                sidecar_count(&entry.path())
            } else {
                usize::from(name.to_string_lossy().ends_with(".import.json"))
            }
        })
        .sum()
}

#[test]
#[ignore = "real-project timing probe: set ARA_PLAY_ENTRY_PROJECT and ARA_BENCH_PLAYER; serialize with ara-dev gpu"]
fn real_project_warm_play_entry() {
    let source =
        PathBuf::from(std::env::var_os("ARA_PLAY_ENTRY_PROJECT").expect("ARA_PLAY_ENTRY_PROJECT"));
    let player = PathBuf::from(std::env::var_os("ARA_BENCH_PLAYER").expect("ARA_BENCH_PLAYER"));
    assert!(player.is_file());
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/play-entry")
        .join(std::process::id().to_string());
    let project = output.join("project");
    let cold = std::env::var_os("ARA_PLAY_ENTRY_COLD").is_some();
    let background = cold || std::env::var_os("ARA_PLAY_ENTRY_BACKGROUND").is_some();
    assert!(
        !project.exists(),
        "use a fresh process for each timing probe"
    );
    copy(&source, &project);
    for cache in ["cache", "gpu"] {
        let cache_source = source.join(".ara").join(cache);
        if !cold && cache_source.is_dir() {
            copy(&cache_source, &project.join(".ara").join(cache));
        }
    }
    let windowed = std::env::var_os("ARA_PLAY_ENTRY_WINDOW").is_some();
    let mut attachment = None;
    let mut published = None;
    let mut agent = Agent::open_with(
        &project,
        5,
        |config| {
            config.pool.headless = usize::from(!windowed);
            config.pool.windowed = usize::from(windowed);
            config.pool.size = [2560, 1440];
        },
        |service| {
            published = Some(service.published().unwrap());
            if !windowed {
                attachment = Some(service.surfaces());
            }
        },
    );
    let asset_count = sidecar_count(&project);
    let asset_warmup = Instant::now();
    let published = published.unwrap();
    if !cold {
        agent.until("completed asset publication before warm Play", |_| {
            if let Some(progress) = published.progress() {
                assert!(
                    progress.failed.is_empty(),
                    "warm Play requires complete source art: {:?}",
                    progress.failed
                );
            }
            published
                .snapshot()
                .is_some_and(|snapshot| snapshot.generations().count() >= asset_count)
        });
    }
    let asset_warmup_ms = asset_warmup.elapsed().as_secs_f64() * 1000.;
    let mut runs = Vec::new();
    for run in 0..3 {
        let warming = Instant::now();
        // The first cold click includes pool/device startup. Later clicks wait
        // for the ready pool, matching ordinary warm editor sessions.
        if !cold || run != 0 {
            agent.until("warm process pool", |agent| {
                agent.play("status", json!({}))["pool"]
                    [if windowed { "windowed" } else { "headless" }]
                .as_u64()
                .unwrap_or(0)
                    >= 1
            });
        }
        let pool_ms = warming.elapsed().as_secs_f64() * 1000.;
        let clicked = Instant::now();
        let mut params = json!({"startPaused":true});
        if background {
            params["background"] = true.into();
        }
        if windowed {
            params["window"] = true.into();
        } else {
            params["embed"] = json!({"width":2560,"height":1440,"maxFps":144});
        }
        let started = agent.play("start", params);
        let response_ms = clicked.elapsed().as_secs_f64() * 1000.;
        let world = started["world"].as_str().unwrap().to_owned();
        let mut phase = String::new();
        let mut ready = |agent: &mut Agent| {
            let status = agent.play("status", json!({}));
            if let Some(row) = status["worlds"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["world"] == world)
            {
                let next = format!("{}:{}", row["status"], row["progress"]["phase"]);
                if next != phase {
                    println!(
                        "PLAY_PROGRESS {:.1} ms {row}",
                        clicked.elapsed().as_secs_f64() * 1000.
                    );
                    phase = next;
                }
                assert_ne!(row["status"], "failed", "Play startup failed: {row}");
                assert!(row["fault"].is_null(), "Play player fault: {row}");
            }
            if let Some(surfaces) = &mut attachment {
                assert!(surfaces.poll());
                if let Some(frame) = surfaces.take(&world, 0) {
                    // The probe never samples this slot on an owner GPU.
                    surfaces.release(&world, 0, frame);
                    return true;
                }
                false
            } else {
                agent.play("status", json!({}))["worlds"][0]["players"][0]["status"]["fps"]
                    .as_f64()
                    .is_some_and(|fps| fps > 0.)
            }
        };
        let mut host_ticks_ms = Vec::new();
        while !ready(&mut agent) {
            assert!(
                clicked.elapsed() < std::time::Duration::from_secs(120),
                "timed out waiting for first completed player frame"
            );
            let tick = Instant::now();
            agent.host.tick();
            host_ticks_ms.push(tick.elapsed().as_secs_f64() * 1000.);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let first_ms = clicked.elapsed().as_secs_f64() * 1000.;
        let status = agent.play("status", json!({}));
        let ready = status["worlds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["world"] == world)
            .cloned()
            .unwrap_or_default();
        runs.push(json!({"poolWarmupMs":pool_ms,"startResponseMs":response_ms,
            "firstCompletedFrameObservedMs":first_ms,"hostTicksMs":host_ticks_ms,
            "started":started,"ready":ready}));
        if std::env::var_os("ARA_PLAY_ENTRY_CAPTURE").is_some() {
            agent.play(
                "capture",
                json!({"world":world,"path":output.join(format!("frame-{run}.png")),
                    "width":2560,"height":1440,"presented":true,"marks":0}),
            );
        }
        agent.play("stop", json!({"world":world}));
    }
    let report = json!({"format":"ara.play-entry/1","windowed":windowed,
        "cold":cold,"background":background,
        "measurement":"start request to first completed shared frame notice (embedded players publish their first frame once its presentation is ready, at most 1.5 s after their first content frame); windowed status is an upper bound with a 250 ms heartbeat; excludes editor display",
        "source":source,"player":player,"assetWarmupMs":asset_warmup_ms,
        "publishedAssets":asset_count,"runs":runs});
    std::fs::write(
        output.join("entry.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("PLAY_ENTRY {}", output.join("entry.json").display());
}
