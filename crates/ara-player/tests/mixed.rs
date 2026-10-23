#![allow(clippy::unwrap_used)]
//! A scene mixing 3D, VFX and audio in Play, driven over MCP: the demo (skinned Fox,
//! physics, crowds, lights) with its campfire effect gets a crackle sound an agent
//! attaches through `ara_apply`; the sound imports through the pipeline (the
//! `ara.audio` worker importer), plays through the play audio path (mixer, room
//! reverb, acoustics against the physics scene) and renders offline to an analysed
//! WAV; the image capture composites the fire's GPU particles.
mod support;

use ara_asset::{ImportSettings, Sidecar, SidecarCommand};
use serde_json::json;
use std::{path::Path, time::Instant};
use support::{Agent, demo_copy};

/// One second of 48 kHz stereo PCM16: a decaying 220 Hz tone with seeded crackle.
fn crackle() -> Vec<u8> {
    let rate = 48_000u32;
    let mut samples = Vec::new();
    let mut noise = 0x2545_f491u32;
    for i in 0..rate {
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        let t = i as f32 / rate as f32;
        let tone = (t * 220.0 * std::f32::consts::TAU).sin() * 0.25;
        let pop = if noise.is_multiple_of(2400) { 0.6 } else { 0.0 };
        let v = ((tone + pop) * (1.0 - t * 0.5) * i16::MAX as f32) as i16;
        samples.extend_from_slice(&v.to_le_bytes());
        samples.extend_from_slice(&v.to_le_bytes());
    }
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(&samples);
    wav
}

/// Writes the sound and its sidecar (typed `SidecarCommand`, canonical writer).
fn add_sound(dir: &Path) {
    let worker = support::import_worker(Path::new(env!("CARGO_BIN_EXE_ara-player")));
    let versions = ara_asset::ProcessWorker::new(worker).versions().unwrap();
    std::fs::create_dir_all(dir.join("sounds")).unwrap();
    std::fs::write(dir.join("sounds/fire.wav"), crackle()).unwrap();
    let mut schema = ara_schema::SchemaDb::new();
    ara_doc::register(&mut schema);
    ara_asset::register_schema(&mut schema);
    let id = ara_doc::AssetId::parse("as_fire0001").unwrap();
    let mut workspace = ara_doc::Workspace::default();
    workspace.insert("sidecar", Sidecar::empty(id.clone()).into_document());
    let mut store = ara_ops::Store::new(schema, workspace);
    let edit = SidecarCommand::Initialize(ImportSettings {
        importer: "ara.audio".into(),
        version: versions["ara.audio"].clone(),
        settings: json!({"usage": "shortEffect", "codec": "pcm"}),
        ..Default::default()
    })
    .edit(id.clone())
    .unwrap();
    store
        .transact(ara_ops::TxnMeta::by(ara_ops::Author::Human), [edit])
        .unwrap();
    let text = ara_doc::write(store.workspace().doc(id.as_str()).unwrap(), store.schema());
    std::fs::write(dir.join("sounds/fire.wav.import.json"), text).unwrap();
}

#[test]
fn agent_plays_a_scene_mixing_3d_vfx_and_audio() {
    let dir = demo_copy("mixed");
    add_sound(&dir);
    let captures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/runtime-polish/captures");
    let mut agent = Agent::open(&dir, 13, |_| {});
    agent.tool(
        "ara_apply",
        json!({"doc": "levels/demo.scene", "why": "The campfire crackles", "commands": [
            {"cmd": "component.add", "args": {"entity": "e_fire", "component": "AudioSource",
                "value": {"clip": "@sounds/fire.wav#as_fire0001", "looping": true,
                          "referenceDistance": 2, "maxDistance": 40}}}
        ]}),
    );
    agent.pool_ready();
    let world = agent.play("start", json!({"startPaused": true, "seed": 13}));
    let id = world["world"].clone();
    agent.play(
        "step",
        json!({"world": id, "ticks": 60, "inputs": [{"action": "Move", "value": [0.3, 1], "holdTicks": 60}]}),
    );
    // Audio: the fire's crackle, heard from the follow camera.
    let wav = captures.join("mixed.wav");
    let t = Instant::now();
    let audio = agent.play(
        "captureAudio",
        json!({"world": id, "path": wav.to_string_lossy(), "seconds": 1.0}),
    );
    let audio_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(audio["voices"], json!(["e_fire"]), "{audio}");
    // Acoustics against the physics scene: distance, attenuation and the direct path.
    let fire = &audio["sources"][0];
    assert!(fire["distance"].as_f64().unwrap() > 1.0, "{fire}");
    // The fire sits at ground height: the ground occludes part of its direct path and
    // the audibility report (an `ara why` cause) says so.
    assert!(fire["visibility"].as_f64().unwrap() < 1.0, "{fire}");
    assert_eq!(fire["occluders"], json!(["generic e_ground"]), "{fire}");
    assert_eq!(fire["explain"][0]["code"], json!("ARA3402"), "{fire}");
    let analysis = &audio["analysis"];
    assert_eq!(analysis["silent"], json!(false), "{analysis}");
    assert!(analysis["integratedLufs"].as_f64().is_some(), "{analysis}");
    assert!(wav.exists());
    // Image: 3D scene with the fire's GPU particles composited in.
    let png = captures.join("mixed.png");
    let t = Instant::now();
    agent.play(
        "capture",
        json!({"world": id, "path": png.to_string_lossy(), "marks": 12}),
    );
    let image_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert!(png.exists());
    agent.play("stop", json!({"world": id}));
    println!(
        "MIXED {}",
        serde_json::to_string_pretty(&json!({
            "audio": {
                "captureMs": audio_ms,
                "fire": fire,
                "integratedLufs": analysis["integratedLufs"],
                "samplePeak": analysis["samplePeak"],
            },
            "imageWithVfxMs": image_ms,
        }))
        .unwrap()
    );
}
