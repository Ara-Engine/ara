#![allow(clippy::unwrap_used)] // Tests may panic on unexpected failures.
//! Room reverb DSP calibration, allocation-free rendering and golden offline captures.

use ara_acoustics::*;
use ara_audio::{
    AudioEngine, AudioListener, AudioSource, AuxEffect, BusId, Clip, EngineConfig, Frame, Playback,
    capture::capture_audio, effects::TrackBuilder,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Counts allocations made on threads that opt in, so the harness's threads don't interfere.
struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static COUNT: Cell<bool> = const { Cell::new(false) };
}
// SAFETY: forwards to the system allocator unchanged; counting has no side effects on memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT.with(Cell::get) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNT.with(Cell::get) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: `ptr` came from `alloc` with the same layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    COUNT.with(|c| c.set(true));
    let out = f();
    COUNT.with(|c| c.set(false));
    (out, ALLOCATIONS.load(Ordering::Relaxed) - before)
}

fn params(rt60: [f32; 3]) -> ReverbParams {
    ReverbParams {
        rt60,
        mean_free_path: 6.0,
        level: 0.5,
        wet: 1.0,
        room: 7,
        ..ReverbParams::silent()
    }
}

/// Renders an impulse through a room reverb; returns the stereo energy envelope.
fn impulse_response(p: &ReverbParams, seconds: f32) -> Vec<f32> {
    let (mut reverb, mut handle) = room_reverb(48000, &AcousticsSettings::default());
    handle.set(p).unwrap();
    let blocks = (seconds * 48000.0 / 128.0) as usize;
    let mut energy = Vec::with_capacity(blocks * 128);
    let mut out = [Frame::ZERO; 128];
    let mut input = [0.0_f32; 128];
    for block in 0..blocks {
        input[0] = if block == 0 { 1.0 } else { 0.0 };
        reverb.process(&input, &mut out, 48000);
        energy.extend(out.iter().map(|f| f.left * f.left + f.right * f.right));
    }
    energy
}

/// T30 from Schroeder backward integration, extrapolated to 60 dB.
fn rt60(energy: &[f32]) -> f32 {
    let mut tail: Vec<f64> = energy
        .iter()
        .rev()
        .scan(0.0, |s, e| {
            *s += f64::from(*e);
            Some(*s)
        })
        .collect();
    tail.reverse();
    let total = tail[0];
    let at = |db: f64| {
        tail.iter()
            .position(|e| 10.0 * (e / total).log10() <= db)
            .unwrap()
    };
    let (a, b) = (at(-5.0), at(-35.0));
    2.0 * (b - a) as f32 / 48000.0
}

#[test]
fn fdn_decay_matches_requested_rt60() {
    for target in [0.6_f32, 1.5, 3.0] {
        let measured = rt60(&impulse_response(&params([target; 3]), target * 1.6 + 0.3));
        assert!(
            (measured / target - 1.0).abs() < 0.2,
            "requested {target} s, measured {measured} s"
        );
    }
    // Bright and dark rooms: the high band decays independently.
    let dark = impulse_response(&params([2.0, 2.0, 0.3]), 1.0);
    let bright = impulse_response(&params([2.0, 2.0, 2.0]), 1.0);
    let late = |e: &[f32]| e[24000..].iter().sum::<f32>();
    assert!(late(&dark) < late(&bright));
}

#[test]
fn reverb_is_deterministic_and_allocation_free() {
    let p = params([1.2, 1.0, 0.5]);
    let (mut reverb, mut handle) = room_reverb(48000, &AcousticsSettings::default());
    handle.set(&p).unwrap();
    let mut out = [Frame::ZERO; 128];
    let mut input = [0.0_f32; 128];
    input[3] = 1.0;
    reverb.process(&input, &mut out, 48000);
    let (_, allocations) = counted(|| {
        for i in 0..200 {
            input[0] = (i as f32 * 0.37).sin();
            reverb.process(&input, &mut out, 48000);
        }
    });
    assert_eq!(allocations, 0);
    // A room change mid-stream crossfades without allocating either.
    let mut other = params([0.4; 3]);
    other.mean_free_path = 2.0;
    other.room = 9;
    handle.set(&other).unwrap();
    let (_, allocations) = counted(|| reverb.process(&input, &mut out, 48000));
    assert_eq!(allocations, 0);
    assert_eq!(impulse_response(&p, 0.5), impulse_response(&p, 0.5));
}

fn noise(seconds: f32, seed: u32) -> Clip {
    let mut state = seed;
    let frames = (0..(seconds * 48000.0) as usize)
        .map(|i| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let s = ((state >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0) * 0.25;
            // Short bursts make reverb tails audible in the capture.
            let gate = if (i / 4800) % 4 == 0 { 1.0 } else { 0.0 };
            Frame::new(s * gate, s * gate)
        })
        .collect();
    Clip::new(48000, frames, 1 << 26).unwrap()
}

/// A two-room level: the listener in `west`, a radio behind the wall in `east`, a car
/// passing in `west`. Returns the capture's WAV hash and canonical analysis JSON.
fn golden_scene() -> (String, String) {
    let mut scene = MeshScene::new();
    let concrete = MaterialPreset::Concrete.material();
    scene
        .add_box(
            [-0.1, 0.0, -5.0],
            [0.1, 3.0, -0.5],
            1,
            "e_wallnorth",
            concrete,
        )
        .unwrap();
    scene
        .add_box(
            [-0.1, 0.0, 0.5],
            [0.1, 3.0, 5.0],
            2,
            "e_wallsouth",
            concrete,
        )
        .unwrap();
    let zone = |name: &str, x: f32| ReverbZone {
        name: name.into(),
        position: [x, 1.5, 0.0],
        half_extents: [5.0, 1.5, 5.0],
        material: MaterialPreset::Concrete,
        ..ReverbZone::default()
    };
    let mut acoustics = Acoustics::new(AcousticsSettings::default()).unwrap();
    acoustics
        .set_zones(
            &[zone("west", -5.0), zone("east", 5.0)],
            &[AcousticPortal {
                a: "west".into(),
                b: "east".into(),
                position: [0.0, 1.2, 0.0],
                ..AcousticPortal::default()
            }],
        )
        .unwrap();
    let listener = AudioListener {
        position: [-4.0, 1.5, -3.0],
        ..AudioListener::default()
    };
    let mut engine = AudioEngine::new(EngineConfig {
        real_voices: 8,
        total_voices: 16,
        buses: 8,
        ..EngineConfig::default()
    })
    .unwrap();
    let sfx = engine
        .add_bus("sfx", BusId::MASTER, TrackBuilder::new())
        .unwrap();
    let (reverb, mut handle) = room_reverb(48000, acoustics.settings());
    let room = engine.add_aux("room", Box::new(reverb)).unwrap();
    engine.route_aux(sfx, Some(room)).unwrap();
    handle.set(&acoustics.room(&listener)).unwrap();
    let sources = [
        (
            "e_radio",
            AudioSource {
                position: [4.0, 1.5, -3.0],
                max_distance: 60.0,
                ..AudioSource::default()
            },
        ),
        (
            "e_car",
            AudioSource {
                position: [-4.0, 1.5, 3.0],
                velocity: [0.0, 0.0, -15.0],
                max_distance: 60.0,
                ..AudioSource::default()
            },
        ),
        (
            "e_drip",
            AudioSource {
                position: [-6.0, 0.5, -3.5],
                max_distance: 60.0,
                ..AudioSource::default()
            },
        ),
    ];
    let acoustic = AcousticSource::default();
    let emitters: Vec<Emitter<'_>> = sources
        .iter()
        .map(|(label, source)| Emitter {
            label,
            source,
            acoustic: &acoustic,
        })
        .collect();
    let mut evaluations = Vec::new();
    acoustics
        .evaluate(&mut scene, &listener, &emitters, &mut evaluations)
        .unwrap();
    for (i, evaluation) in evaluations.iter().enumerate() {
        engine
            .play(
                sfx,
                noise(1.0, 17 + i as u32),
                Playback {
                    looping: true,
                    parameters: evaluation.parameters,
                    ..Playback::default()
                },
            )
            .unwrap();
    }
    let mut capture = capture_audio(&mut engine, 2.0).unwrap();
    capture.analysis.occlusion = evaluations.into_iter().map(|e| e.record).collect();
    (
        capture.analysis.wav_blake3.clone(),
        capture.analysis_json().unwrap(),
    )
}

#[test]
fn golden_two_room_capture_is_deterministic() {
    let (hash, json) = golden_scene();
    let (again, json_again) = golden_scene();
    assert_eq!(hash, again);
    assert_eq!(json, json_again);
    let golden = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/two_rooms.json");
    if std::env::var_os("ARA_UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(std::path::Path::new(golden).parent().unwrap()).unwrap();
        std::fs::write(golden, &json).unwrap();
    }
    let expected = std::fs::read_to_string(golden).unwrap();
    assert!(
        json.trim_end() == expected.trim_end(),
        "run with ARA_UPDATE_GOLDENS=1 after intended changes"
    );
    let value = ara_json::parse(&json).unwrap();
    let occlusion = value["occlusion"].as_array().unwrap();
    assert_eq!(occlusion[0]["emitter"], "e_radio");
    assert_eq!(occlusion[0]["occluded"], true);
    let room = value["buses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["name"] == "room")
        .unwrap();
    assert!(
        room["rmsDbfs"].as_f64().unwrap() > -40.0,
        "reverb return is audible"
    );
}

#[test]
fn band_overshoot_never_makes_the_network_grow() {
    // A dead low band with long mid/high times used to push the loop gain above one.
    for rt60 in [[0.0, 3.0, 3.0], [0.1, 8.0, 8.0], [8.0, 0.0, 8.0]] {
        let energy = impulse_response(&params(rt60), 4.0);
        assert!(energy.iter().all(|e| e.is_finite()), "{rt60:?}");
        let early: f32 = energy[..48000].iter().sum();
        let late: f32 = energy[144000..].iter().sum();
        assert!(late < early, "{rt60:?}: late {late} early {early}");
    }
}

#[test]
fn leaving_a_room_keeps_its_tail() {
    let (mut reverb, mut handle) = room_reverb(48000, &AcousticsSettings::default());
    handle.set(&params([2.0; 3])).unwrap();
    let mut out = [Frame::ZERO; 128];
    let noise = [0.3_f32; 128];
    for _ in 0..40 {
        reverb.process(&noise, &mut out, 48000);
    }
    // Outdoors: a silent room of another size; nothing new is sent.
    handle.set(&ReverbParams::silent()).unwrap();
    let silence = [0.0_f32; 128];
    let mut tail = 0.0;
    for block in 0..80 {
        reverb.process(&silence, &mut out, 48000);
        if (40..80).contains(&block) {
            tail += out.iter().map(|f| f.left * f.left).sum::<f32>();
        }
    }
    assert!(
        tail > 1e-3,
        "the hall's tail rings on after leaving: {tail}"
    );
}
