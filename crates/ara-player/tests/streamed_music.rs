//! A long imported score stays compressed through the real headless Play path.
#![allow(clippy::unwrap_used)]
mod support;

use ara_asset::{
    CachedImport, Cancellation, CookPreset, ImportContext, ImportInput, TargetPlatform,
};
use ara_audio::{AudioEngine, Playback, PreparedVoice};
use ara_runtime::assets::MemoryAssets;
use ara_runtime_audio::{AssetVoices, StreamMode, VoiceLoader};
use ara_schema::AssetRef;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};
use support::{Agent, demo_copy};

const SECONDS: usize = 181;
const RATE: usize = 48000;

fn long_wav() -> Vec<u8> {
    wav(SECONDS)
}

fn wav(seconds: usize) -> Vec<u8> {
    let count = seconds * RATE;
    let size = (count * 2) as u32;
    let mut wav = Vec::with_capacity(size as usize + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(size + 36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    wav.extend_from_slice(&(RATE as u32).to_le_bytes());
    wav.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    wav.extend_from_slice(b"\x02\0\x10\0data");
    wav.extend_from_slice(&size.to_le_bytes());
    let second: Vec<i16> = (0..RATE)
        .map(|i| ((i as f32 * 220. * std::f32::consts::TAU / RATE as f32).sin() * 8192.) as i16)
        .collect();
    for _ in 0..seconds {
        for sample in &second {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
    }
    wav
}

#[test]
fn play_import_configuration_uses_current_commands_before_sidecar_save() {
    let dir = demo_copy("current-import");
    std::fs::create_dir_all(dir.join("sounds")).unwrap();
    std::fs::write(dir.join("sounds/tone.wav"), wav(1)).unwrap();
    let mut agent = Agent::open(&dir, 44, |_| {});
    agent.tool(
        "ara_apply",
        json!({"why":"Import a short effect", "commands":[
            {"cmd":"import.initialize", "args":{"source":"sounds/tone.wav", "id":"as_tone",
                "settings":{"usage":"shortEffect","codec":"pcm"}}}
        ]}),
    );
    let worker = support::import_worker();
    let mut assets = ara_play_host::assets::Assets::new(&dir, &worker).unwrap();
    assets
        .synchronize(
            agent.host.store().workspace(),
            agent.host.store().revision(),
        )
        .unwrap();
    let wanted = std::collections::BTreeSet::from(["as_tone".to_owned()]);
    let original = assets
        .pins(&wanted, std::time::Duration::from_secs(30))
        .unwrap()["as_tone"]
        .clone();
    agent.host.flush();
    let before = std::fs::read(dir.join("sounds/tone.wav.import.json")).unwrap();
    agent.tool("ara_apply", json!({"why":"Use compressed effects", "commands":[
        {"cmd":"import.configure", "args":{"source":"sounds/tone.wav", "settings":{"usage":"shortEffect","codec":"adpcm"}}}
    ]}));
    assert_eq!(
        before,
        std::fs::read(dir.join("sounds/tone.wav.import.json")).unwrap(),
        "test must cover the interval before the canonical save"
    );
    assets
        .synchronize(
            agent.host.store().workspace(),
            agent.host.store().revision(),
        )
        .unwrap();
    assert_eq!(
        assets.digest("as_tone").as_deref(),
        Some(original.as_str()),
        "last good remains live while cooking"
    );
    let updated = assets
        .pins(&wanted, std::time::Duration::from_secs(30))
        .unwrap()["as_tone"]
        .clone();
    assert_ne!(
        original, updated,
        "new Play must wait for the command's configured generation"
    );
}

fn prepared(voices: &mut AssetVoices, looping: bool) -> ara_audio::Result<PreparedVoice> {
    voices.prepare(
        &AssetRef("@sounds/long.wav#as_longscore".into()),
        Playback {
            looping,
            spatial: false,
            ..Default::default()
        },
    )
}

#[test]
fn long_score_renders_past_three_minutes_and_loops_with_a_64k_queue_budget() {
    let registry = ara_importers::registry().unwrap();
    let importer = registry.get("ara.audio").unwrap();
    let input = ImportInput {
        source_path: "long.wav".into(),
        source: long_wav(),
        importer: importer.id().into(),
        version: importer.version().into(),
        preset: CookPreset::Fast,
        thread_budget: 4,
        target: TargetPlatform::Windows,
        settings: json!({"usage":"music", "decodedBudget":65536}),
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    };
    let output = registry
        .import(
            &input,
            &mut ImportContext {
                cancellation: &Cancellation::default(),
                metrics: None,
                progress: &mut |_| {},
            },
        )
        .unwrap();
    let assets = Arc::new(MemoryAssets(BTreeMap::from([(
        "as_longscore".into(),
        CachedImport {
            key: input.key().unwrap(),
            output,
        },
    )])));
    // The former Play consumer fails on this same valid import (>64 MiB stereo PCM).
    assert!(
        ara_runtime_audio::clip(
            assets.as_ref(),
            &AssetRef("@sounds/long.wav#as_longscore".into())
        )
        .is_err()
    );
    let mut voices = AssetVoices::new(assets.clone(), StreamMode::Offline, 65536);
    let mut engine = AudioEngine::new(Default::default()).unwrap();
    engine
        .install(
            ara_audio::BusId::MASTER,
            prepared(&mut voices, true).unwrap(),
        )
        .unwrap();
    assert!(voices.queued_bytes() > 32000 && voices.queued_bytes() <= 65536);
    assert!(
        prepared(&mut voices, false).is_err(),
        "aggregate budget must reject a second queue"
    );
    let mut block = [0.; ara_audio::QUANTUM * 2];
    let mut late_energy = 0.;
    let mut loop_energy = 0.;
    let mut frame = 0;
    while frame < RATE * (SECONDS + 2) {
        voices.refill().unwrap();
        engine.render(&mut block).unwrap();
        let energy: f64 = block.iter().map(|v| f64::from(*v).powi(2)).sum();
        if (RATE * 179..RATE * SECONDS).contains(&frame) {
            late_energy += energy;
        }
        if frame >= RATE * SECONDS {
            loop_energy += energy;
        }
        frame += ara_audio::QUANTUM;
    }
    assert!(
        late_energy > 100. && loop_energy > 100.,
        "late={late_energy}, loop={loop_energy}"
    );
    drop(engine);
    voices.refill().unwrap();
    assert_eq!(
        voices.queued_bytes(),
        0,
        "abandoned queues release the budget"
    );
    let mut live = AssetVoices::new(assets, StreamMode::Live, 65536);
    let voice = prepared(&mut live, true).unwrap();
    assert!(live.queued_bytes() <= 65536);
    drop(voice);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while live.queued_bytes() != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "abandoned live producer did not exit"
        );
        live.refill().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn agent_imports_and_plays_a_three_minute_music_document_headlessly() {
    let dir = demo_copy("streamed-music");
    std::fs::create_dir_all(dir.join("sounds")).unwrap();
    std::fs::write(dir.join("sounds/long.wav"), long_wav()).unwrap();
    let mut agent = Agent::open(&dir, 42, |_| {});
    agent.tool("ara_apply", json!({"why":"Import the long score", "commands":[
        {"cmd":"import.initialize", "args":{"source":"sounds/long.wav", "id":"as_longscore", "settings":{"usage":"music", "decodedBudget":65536}}},
        {"cmd":"doc.create", "args":{"path":"sounds/score.music", "id":"mu_longscore"}}
    ]}));
    agent.tool("ara_apply", json!({"doc":"sounds/score.music", "why":"Compose the long score", "commands":[
        {"cmd":"music.settings.set", "args":{"settings":{"initial":"Long"}}},
        {"cmd":"music.segment.set", "args":{"id":"sg_long", "segment":{"name":"Long", "bars":128,
            "stems":[{"name":"Long","clip":"@sounds/long.wav#as_longscore"}]}}}
    ]}));
    agent.tool("ara_apply", json!({"doc":"levels/demo.scene", "why":"Play the long score", "commands":[
        {"cmd":"entity.create", "args":{"id":"e_longscore", "components":{"MusicSource":{"music":"@sounds/score.music#mu_longscore"}}}}
    ]}));
    agent.pool_ready();
    let id = agent.play("start", json!({"startPaused":true,"seed":42}))["world"].clone();
    let path = dir.join("long-score.wav");
    let capture = agent.play(
        "captureAudio",
        json!({"world":id,"seconds":1.,"path":path.to_string_lossy()}),
    );
    assert_eq!(capture["analysis"]["silent"], false, "{capture}");
    assert!(
        capture["analysis"]["integratedLufs"].as_f64().is_some(),
        "{capture}"
    );
    let bytes = std::fs::read(&path).unwrap();
    let mut decoder =
        ara_audio::codec::MediaDecoder::open(Box::new(std::io::Cursor::new(bytes.clone())), "wav")
            .unwrap();
    let audible = ara_audio::codec::decode_clip(&mut decoder, 1024 * 1024).unwrap();
    assert!(
        audible.frames()[RATE - 1024..]
            .iter()
            .any(|f| f.left.abs() > 0.01),
        "offline streams must remain audible after their initial prebuffer"
    );
    let repeat = dir.join("long-score-repeat.wav");
    agent.play(
        "captureAudio",
        json!({"world":id,"seconds":1.,"path":repeat.to_string_lossy()}),
    );
    assert_eq!(
        bytes,
        std::fs::read(repeat).unwrap(),
        "offline refills must be deterministic"
    );
    agent.play("stop", json!({"world":id}));
}
