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
    assert!(
        !project.exists(),
        "use a fresh process for each timing probe"
    );
    copy(&source, &project);
    for cache in ["cache", "gpu"] {
        let cache_source = source.join(".ara").join(cache);
        if cache_source.is_dir() {
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
    agent.until("completed asset publication before warm Play", |_| {
        published
            .snapshot()
            .is_some_and(|snapshot| snapshot.generations().count() >= asset_count)
    });
    let asset_warmup_ms = asset_warmup.elapsed().as_secs_f64() * 1000.;
    let mut runs = Vec::new();
    for _ in 0..3 {
        let warming = Instant::now();
        agent.until("warm process pool", |agent| {
            agent.play("status", json!({}))["pool"][if windowed { "windowed" } else { "headless" }]
                .as_u64()
                .unwrap_or(0)
                >= 1
        });
        let pool_ms = warming.elapsed().as_secs_f64() * 1000.;
        let clicked = Instant::now();
        let mut params = json!({"startPaused":true});
        if windowed {
            params["window"] = true.into();
        } else {
            params["embed"] = json!({"width":2560,"height":1440,"maxFps":144});
        }
        let started = agent.play("start", params);
        let response_ms = clicked.elapsed().as_secs_f64() * 1000.;
        let world = started["world"].as_str().unwrap().to_owned();
        agent.until("first completed player frame", |agent| {
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
        });
        let first_ms = clicked.elapsed().as_secs_f64() * 1000.;
        runs.push(json!({"poolWarmupMs":pool_ms,"startResponseMs":response_ms,
            "firstCompletedFrameObservedMs":first_ms,"started":started}));
        agent.play("stop", json!({"world":world}));
    }
    let report = json!({"format":"ara.play-entry/1","windowed":windowed,
        "measurement":"start request to first completed shared frame notice; windowed status is an upper bound with a 250 ms heartbeat; excludes editor display",
        "source":source,"player":player,"assetWarmupMs":asset_warmup_ms,
        "publishedAssets":asset_count,"runs":runs});
    std::fs::write(
        output.join("entry.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("PLAY_ENTRY {}", output.join("entry.json").display());
}
