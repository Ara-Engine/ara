//! Release benchmark: 1,000 voices with occlusion (three-band filtered paths) and per-voice
//! reverb sends into the room reverb, on one core, per 128-frame block; plus the cost of
//! one `Acoustics::evaluate` pass for 1,000 emitters. Writes `evidence/benchmark.json`.
//!
//! `cargo run -p ara-acoustics --release --example acoustics_bench`
use ara_acoustics::*;
use ara_audio::{
    AudioEngine, AudioError, AudioListener, AudioSource, BusId, Clip, EngineConfig, Frame,
    Playback, QUANTUM, VoiceCommand, VoiceParameters, effects::TrackBuilder, spatial::AcousticPath,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    time::Instant,
};

thread_local! {
    static COUNT: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}
struct Counting;
// SAFETY: forwards every call to System unchanged; thread-local counters allocate nothing.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT.with(Cell::get) {
            ALLOCS.with(|a| a.set(a.get() + 1));
        }
        // SAFETY: the caller's valid layout is forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if COUNT.with(Cell::get) {
            ALLOCS.with(|a| a.set(a.get() + 1));
        }
        // SAFETY: the pointer and layout come from a matching `alloc`.
        unsafe { System.dealloc(p, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn percentiles(mut times: Vec<f64>) -> serde_json::Value {
    times.sort_by(f64::total_cmp);
    let at = |p: f64| times[((times.len() - 1) as f64 * p) as usize];
    serde_json::json!({
        "samples": times.len(),
        "meanMs": times.iter().sum::<f64>() / times.len() as f64,
        "p50Ms": at(0.5),
        "p95Ms": at(0.95),
        "maxMs": at(1.0),
    })
}

fn noise() -> Result<Clip, AudioError> {
    let mut state = 3_u32;
    let frames = (0..48000)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let s = ((state >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0) * 0.5;
            Frame::new(s, s)
        })
        .collect();
    Clip::new(48000, frames, 1 << 24)
}

fn parameters(i: usize, t: f32) -> VoiceParameters {
    let angle = i as f32 * std::f32::consts::TAU / 1000.0 + t;
    VoiceParameters {
        gain: 0.001,
        direction: [angle.sin(), 0.0, -angle.cos()],
        path: AcousticPath {
            visibility: 0.3 + 0.4 * ((i % 7) as f32 / 7.0),
            transmission: [0.2, 0.08, 0.02],
            reverb_send: 0.6,
            air: [1.0, 0.96, 0.7],
        },
        ..VoiceParameters::default()
    }
}

fn render_bench(update_every: usize) -> Result<serde_json::Value, AudioError> {
    let mut engine = AudioEngine::new(EngineConfig {
        real_voices: 1000,
        total_voices: 1000,
        buses: 8,
        ..EngineConfig::default()
    })?;
    let sfx = engine.add_bus("sfx", BusId::MASTER, TrackBuilder::new())?;
    let settings = AcousticsSettings::default();
    let (reverb, mut handle) = room_reverb(48000, &settings);
    let room = engine.add_aux("room", Box::new(reverb))?;
    engine.route_aux(sfx, Some(room))?;
    let zone = ReverbZone {
        name: "hall".into(),
        half_extents: [12.0, 5.0, 15.0],
        material: MaterialPreset::Concrete,
        ..ReverbZone::default()
    };
    let mut acoustics = Acoustics::new(settings)?;
    acoustics.set_zones(&[zone], &[])?;
    handle.set(&acoustics.room(&AudioListener::default()))?;
    let source = noise()?;
    let mut ids = Vec::with_capacity(1000);
    for i in 0..1000 {
        ids.push(engine.play(
            sfx,
            source.clone(),
            Playback {
                looping: true,
                parameters: parameters(i, 0.0),
                ..Playback::default()
            },
        )?);
    }
    let mut output = [0.0; QUANTUM * 2];
    for _ in 0..32 {
        engine.render(&mut output)?;
    }
    let mut times = Vec::with_capacity(512);
    let mut allocations = 0;
    for block in 0..512 {
        if update_every > 0 && block % update_every == 0 {
            // Control path (untimed): a new acoustics result for every voice.
            let at = engine.next_frame();
            for (i, id) in ids.iter().enumerate() {
                engine.command(
                    *id,
                    VoiceCommand::Set {
                        at_frame: at,
                        parameters: parameters(i, block as f32 * 0.01),
                        ramp_frames: 256,
                    },
                )?;
            }
        }
        ALLOCS.with(|a| a.set(0));
        COUNT.with(|c| c.set(true));
        let start = Instant::now();
        let result = engine.render(&mut output);
        let elapsed = start.elapsed();
        COUNT.with(|c| c.set(false));
        result?;
        allocations += ALLOCS.with(Cell::get);
        times.push(elapsed.as_secs_f64() * 1000.0);
    }
    let mut value = percentiles(times);
    if let Some(o) = value.as_object_mut() {
        o.insert("voices".into(), 1000.into());
        o.insert("realVoices".into(), 1000.into());
        o.insert("quantumFrames".into(), QUANTUM.into());
        o.insert("allocations".into(), allocations.into());
        o.insert(
            "path".into(),
            "occluded three-band SVF path, air absorption, per-voice reverb send, FDN + 18 early taps"
                .into(),
        );
        let updates = if update_every == 0 {
            "static".to_owned()
        } else {
            format!("all 1000 voices retargeted every {update_every} blocks (256-frame ramps)")
        };
        o.insert("parameterUpdates".into(), updates.into());
    }
    Ok(value)
}

fn evaluate_bench() -> Result<serde_json::Value, AudioError> {
    let concrete = MaterialPreset::Concrete.material();
    let mut scene = MeshScene::new();
    // A 10x10 grid of 4 m cells with two walls each (doorways stay open).
    for gx in 0..10 {
        for gz in 0..10 {
            let (x, z) = (gx as f32 * 4.0 - 20.0, gz as f32 * 4.0 - 20.0);
            let id = (gx * 10 + gz) as u64;
            scene.add_box(
                [x, 0.0, z],
                [x + 0.2, 3.0, z + 2.8],
                id * 2,
                "wall",
                concrete,
            )?;
            scene.add_box(
                [x, 0.0, z],
                [x + 2.8, 3.0, z + 0.2],
                id * 2 + 1,
                "wall",
                concrete,
            )?;
        }
    }
    scene.commit();
    let mut acoustics = Acoustics::new(AcousticsSettings::default())?;
    let listener = AudioListener {
        position: [1.0, 1.6, 1.0],
        ..AudioListener::default()
    };
    let sources: Vec<AudioSource> = (0..1000)
        .map(|i| {
            let a = i as f32 * 0.61;
            AudioSource {
                position: [a.cos() * 18.0, 1.0, (a * 1.3).sin() * 18.0],
                max_distance: 60.0,
                ..AudioSource::default()
            }
        })
        .collect();
    let acoustic = AcousticSource::default();
    let labels: Vec<String> = (0..1000).map(|i| format!("e_{i}")).collect();
    let emitters: Vec<Emitter> = sources
        .iter()
        .zip(&labels)
        .map(|(source, label)| Emitter {
            label,
            source,
            acoustic: &acoustic,
        })
        .collect();
    let mut out = Vec::new();
    let mut times = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        acoustics.evaluate(&mut scene, &listener, &emitters, &mut out)?;
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let occluded = out.iter().filter(|e| e.record.occluded).count();
    let mut value = percentiles(times);
    if let Some(o) = value.as_object_mut() {
        o.insert("emitters".into(), 1000.into());
        o.insert("raysPerEmitter".into(), 9.into());
        o.insert("triangles".into(), scene.triangle_count().into());
        o.insert("occludedEmitters".into(), occluded.into());
        o.insert(
            "thread".into(),
            "control/worker (not the audio thread)".into(),
        );
    }
    Ok(value)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let value = serde_json::json!({
        "render": [render_bench(0)?, render_bench(8)?],
        "evaluate": evaluate_bench()?,
        "machine": "Windows 11 dev box, shared with ~8 concurrent agent builds; release profile",
        "budgetMs": 1.0,
    });
    let json = ara_json::to_canonical_string(&ara_json::sort_keys(&value));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("evidence/benchmark.json");
    std::fs::write(path, &json)?;
    println!("{json}");
    Ok(())
}
