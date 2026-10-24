//! Actual child presentation receives pending/failed policy independently of cooks.
#![allow(clippy::unwrap_used)]
mod support;
use ara_play::{ImportState, SnapshotRef, Start, method};
use ara_play_host::pool::{Pool, PoolConfig};
use serde_json::json;
use std::{path::PathBuf, time::Duration};

#[test]
fn game_loading_bounds_are_grey_failed_are_magenta_and_publication_replaces_them() {
    let project = support::demo_copy("game-pending");
    let worker = support::import_worker();
    let mut scene = ara_play_host::package_scene(&project, &worker).unwrap();
    scene.rendering.quality = ara_play::QualitySetting::Low;
    let pins = std::mem::take(&mut scene.assets);
    assert!(pins.contains_key("as_fox"));
    let root = support::repo()
        .join("target/ara-player/pending-game")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root).unwrap();
    let mut pool = Pool::new(
        PoolConfig {
            executable: PathBuf::from(env!("CARGO_BIN_EXE_ara-player")),
            logs: root.join("logs"),
            gpu_cache: root.join("gpu"),
            headless: 1,
            windowed: 0,
            size: [320, 240],
        },
        "pending-game-test".into(),
    )
    .unwrap();
    let mut player = pool
        .take(1, false, Duration::from_secs(30))
        .unwrap()
        .pop()
        .unwrap();
    player
        .link
        .notify(
            method::IMPORTS,
            ImportState {
                revision: 1,
                pending: pins.keys().cloned().collect(),
            },
        )
        .unwrap();
    player.link.flush().unwrap();
    player
        .call(
            method::START,
            &serde_json::to_value(Start {
                snapshot: SnapshotRef::Inline {
                    bytes: scene.encode().unwrap(),
                },
                role: ara_play::Role::Local,
                net: None,
                start_paused: true,
                resume: None,
            })
            .unwrap(),
            Duration::from_secs(30),
        )
        .unwrap()
        .unwrap();
    // Fox is authored in centimetres with a 0.01 entity scale. Give its
    // unresolved bounds a visible metre-sized extent through the typed patch
    // path; this field does not rescale the eventual imported model.
    player
        .call(
            method::PATCH,
            &json!({"sets":[{
                "entity":"e_fox","path":"MeshRenderer.size","value":[150.,150.,150.]
            }]}),
            Duration::from_secs(10),
        )
        .unwrap()
        .unwrap();
    let mut captures = Vec::new();
    for state in ["pending", "failed", "ready"] {
        if state == "failed" {
            player
                .link
                .notify(
                    method::IMPORTS,
                    ImportState {
                        revision: 2,
                        ..Default::default()
                    },
                )
                .unwrap();
        } else if state == "ready" {
            player
                .link
                .notify(method::ASSETS, json!({"assets":pins}))
                .unwrap();
        }
        player.link.flush().unwrap();
        let path = root.join(format!("{state}.png"));
        player
            .call(
                method::CAPTURE,
                &json!({"path":path,"width":320,"height":240,"marks":0,"presented":true}),
                Duration::from_secs(60),
            )
            .unwrap()
            .unwrap();
        let image = image::open(&path).unwrap().into_rgb8();
        let pink = image
            .pixels()
            .filter(|p| p[0] > 150 && p[1] < 90 && p[2] > 150)
            .count();
        captures.push((pink, image));
    }
    assert!(
        captures[1].0 > captures[0].0 + 100,
        "failed/pending magenta counts: {}/{}",
        captures[1].0,
        captures[0].0
    );
    assert!(
        captures[1].0 > captures[2].0 + 100,
        "publication must replace broken bounds"
    );
    assert_ne!(
        captures[0].1, captures[2].1,
        "the real textured model must replace loading bounds"
    );
    player
        .call(method::STOP, &json!({}), Duration::from_secs(10))
        .unwrap()
        .unwrap();
    println!("Game pending/failed/ready captures: {}", root.display());
}
