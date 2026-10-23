#![allow(clippy::unwrap_used)] // Tests may panic on unexpected failures.
//! Compiled programs as voices: timing, determinism, allocation-free rendering,
//! behaviour of the example graphs and golden offline captures.

use ara_audio::{
    AudioEngine, BusId, Clip, EngineConfig, Frame, Playback, PreparedVoice, QUANTUM, SignalSource,
    capture::capture_audio,
};
use ara_audio_graph::{GraphHandle, GraphVoice, Program, library::Op, load};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static COUNT: Cell<bool> = const { Cell::new(false) };
}
// SAFETY: forwards to the system allocator unchanged; counting has no memory effects.
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

fn counted(f: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    COUNT.with(|c| c.set(true));
    f();
    COUNT.with(|c| c.set(false));
    ALLOCATIONS.load(Ordering::Relaxed) - before
}

/// Deterministic stand-in clips: a decaying tone whose pitch depends on the reference.
fn clips(reference: &str) -> ara_audio::Result<Clip> {
    let hash = reference
        .bytes()
        .fold(7_u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let looping = reference.contains("loop");
    let seconds = if looping { 1.0 } else { 0.25 };
    let freq = 120.0 + (hash % 900) as f32;
    let frames = (0..(48000.0 * seconds) as usize)
        .map(|i| {
            let t = i as f32 / 48000.0;
            let envelope = if looping {
                0.5
            } else {
                0.8 * (-t * 18.0).exp()
            };
            let s = (t * freq * std::f32::consts::TAU).cos() * envelope;
            Frame::new(s, s * 0.9)
        })
        .collect();
    Clip::new(48000, frames, 1 << 24)
}

fn example(name: &str) -> Arc<Program> {
    let path = format!("{}/graphs/{name}.audiograph", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(path).unwrap();
    load(&text, "gr_example", 48000, &mut clips).unwrap_or_else(|d| panic!("{name}: {d:?}"))
}

/// Renders `blocks` quanta from frame 0, calling `script` before each block.
fn render(
    voice: &mut GraphVoice,
    handle: &mut GraphHandle,
    blocks: usize,
    mut script: impl FnMut(&mut GraphHandle, u64),
) -> Vec<Frame> {
    let mut out = Vec::with_capacity(blocks * QUANTUM);
    let mut block = [Frame::ZERO; QUANTUM];
    for b in 0..blocks {
        let frame = (b * QUANTUM) as u64;
        script(handle, frame);
        voice.render(&mut block, frame);
        out.extend_from_slice(&block);
    }
    out
}

fn energy(frames: &[Frame]) -> f32 {
    frames
        .iter()
        .map(|f| f.left * f.left + f.right * f.right)
        .sum()
}

#[test]
fn triggers_land_on_their_exact_frame() {
    let program = example("footsteps");
    let (mut voice, mut handle) = program.instantiate(1);
    let out = render(&mut voice, &mut handle, 8, |h, frame| {
        if frame == 0 {
            h.trigger("step", 300).unwrap();
            h.trigger("step", 700).unwrap();
        }
    });
    let first = out.iter().position(|f| f.left != 0.0).unwrap();
    assert_eq!(first, 300);
    assert!(
        handle.set("speed", 1.0, 100).is_err(),
        "controls must stay in frame order"
    );
    let activity = handle.activity("e_player");
    let player = activity.nodes.iter().find(|n| n.node == "player").unwrap();
    assert_eq!(player.op, "sample");
    assert!(player.active_blocks >= 4 && player.peak > 0.1, "{player:?}");
    let step = activity
        .nodes
        .iter()
        .find(|n| n.node == "concrete")
        .unwrap();
    assert_eq!(step.blocks, 8);
}

#[test]
fn seeds_make_variation_reproducible() {
    let program = example("footsteps");
    let take = |seed| {
        let (mut voice, mut handle) = program.instantiate(seed);
        render(&mut voice, &mut handle, 40, |h, frame| {
            if frame % 1280 == 0 {
                h.trigger("step", frame + 5).unwrap();
            }
        })
    };
    assert_eq!(take(7), take(7));
    assert_ne!(take(7), take(8));
}

#[test]
fn examples_render_without_allocating() {
    for name in ["footsteps", "wind", "engine", "ui_click", "rifle"] {
        let program = example(name);
        let (mut voice, mut handle) = program.instantiate(3);
        let mut block = [Frame::ZERO; QUANTUM];
        voice.render(&mut block, 0);
        let allocations = counted(|| {
            for b in 1..200_u64 {
                if b % 20 == 0 {
                    let _ = handle.trigger("step", b * 128);
                    let _ = handle.trigger("fire", b * 128);
                }
                voice.render(&mut block, b * 128);
                assert!(
                    block
                        .iter()
                        .all(|f| f.left.is_finite() && f.right.is_finite()),
                    "{name}"
                );
            }
        });
        assert_eq!(allocations, 0, "{name} allocated while rendering");
    }
}

#[test]
fn engine_pitch_follows_rpm_and_rifle_tails_follow_space() {
    let crossings = |frames: &[Frame]| {
        frames
            .windows(2)
            .filter(|w| (w[0].left < 0.0) != (w[1].left < 0.0))
            .count()
    };
    let engine = example("engine");
    let at = |rpm: f32| {
        let (mut voice, mut handle) = engine.instantiate(0);
        handle.set("rpm", rpm, 0).unwrap();
        let out = render(&mut voice, &mut handle, 150, |_, _| {});
        crossings(&out[QUANTUM * 100..])
    };
    assert!(
        at(6000.0) > at(1200.0) * 2,
        "{} vs {}",
        at(6000.0),
        at(1200.0)
    );
    let rifle = example("rifle");
    let tail = |indoor: f32| {
        let (mut voice, mut handle) = rifle.instantiate(0);
        handle.set("indoor", indoor, 0).unwrap();
        handle.trigger("fire", 0).unwrap();
        let out = render(&mut voice, &mut handle, 375, |_, _| {});
        energy(&out[19200..38400])
    };
    assert!(
        tail(0.0) > tail(1.0) * 4.0,
        "outdoor slap-back outlasts the indoor tail"
    );
}

#[test]
fn one_shots_end_their_voice_in_the_engine() {
    let program = example("ui_click");
    assert!(program.stereo());
    let mut engine = AudioEngine::new(EngineConfig {
        real_voices: 4,
        total_voices: 8,
        buses: 4,
        ..EngineConfig::default()
    })
    .unwrap();
    let (voice, _handle) = program.instantiate(0);
    let id = engine
        .install(
            BusId::MASTER,
            PreparedVoice::procedural(
                Box::new(voice),
                Playback {
                    spatial: false,
                    ..Playback::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
    let capture = capture_audio(&mut engine, 0.2).unwrap();
    assert!(!capture.analysis.silent);
    engine.collect_finished();
    assert!(engine.voice_state(id).is_none(), "done ended the voice");
}

#[test]
fn every_operation_compiles_and_renders() {
    let text = "graph Everything {
  param p = 0.5 range(0, 1);
  param c = @sfx/loop_a.wav;
  trigger t;
  let go = start();
  let tick = metro(40);
  let later = trigger.delay(tick, 0.01);
  let some = random.chance(later, 3, 0.7);
  let every = trigger.every(some, 2);
  let rising = threshold(p, 0.25);
  let s = sine(220);
  let w = saw(110);
  let q = square(55, 0.3);
  let tri = triangle(330);
  let ph = phasor(2);
  let wn = noise.white(1);
  let pk = noise.pink(2);
  let br = noise.brown(3);
  let sm = noise.smooth(4, 4);
  let env = adsr(ph, 0.01, 0.05, 0.6, 0.1);
  let hit = ad(tick, 0.001, 0.05);
  let lp = lowpass(wn, 800, 2);
  let hp = highpass(pk, 2000);
  let bp = bandpass(br, sm, 3);
  let nt = notch(s, 220);
  let eq = peak(w, 1000, 1, 6);
  let ls = lowshelf(q, 200, -6);
  let hs = highshelf(tri, 4000, 3);
  let op = onepole(lp, 500);
  let pick = random.pick(every, 9, c, @sfx/b.wav);
  let pl = sample(pick, tick, 1, 0, 0.1, 0.4, 1);
  let gr = granular(c, ph, 0.05, 30, 1.2, 0.1, 5);
  let m = mix(lp, hp, bp, nt, eq, ls, hs, op);
  let a = add(m, 0.1);
  let b = sub(a, 0.1);
  let mu = mul(b, env.out);
  let d = div(mu, 2);
  let lo = min(d, 0.9);
  let hi = max(lo, -0.9);
  let ab = abs(hi);
  let cl = clamp(ab, 0, 1);
  let le = lerp(cl, gr, p);
  let ma = map(le, 0, 1, -1, 1);
  let po = pow(ab, 1.5);
  let ga = gain(ma, -3);
  let cf = crossfade(ga, po, p);
  let sa = saturate(cf, 3);
  let sl = smooth(p, 0.01);
  let sw = switch(sl, sa, pl.out);
  let r = random.range(t, 4, -1, 1);
  let sq = seq(go, 0.2, 0.4, r);
  let hitmix = mul(hit.out, sq);
  let wet = delay(sw, 0.05, 0.4, 0.1);
  let rise_env = ad(rising, 0.001, 0.01);
  let every_env = ad(every, 0.001, 0.01);
  let ex = mix(wet, hitmix, rise_env.out, every_env.out);
  let stereo = pan(ex, sl);
  output left = stereo.left;
  output right = stereo.right;
}";
    let mut covered: Vec<Op> = Vec::new();
    let graph = ara_audio_graph::dsl::parse(text, "gr_all").unwrap();
    for node in graph.nodes.values() {
        covered.push(node.op);
    }
    for op in Op::ALL {
        assert!(covered.contains(op), "{} is not exercised", op.spec().name);
    }
    let program = load(text, "gr_all", 48000, &mut clips).unwrap();
    let (mut voice, mut handle) = program.instantiate(11);
    let out = render(&mut voice, &mut handle, 100, |h, frame| {
        if frame == 1280 {
            h.trigger("t", 1300).unwrap();
            h.set("p", 0.9, 1300).unwrap();
        }
    });
    assert!(
        out.iter()
            .all(|f| f.left.is_finite() && f.right.is_finite())
    );
    assert!(energy(&out) > 1.0);
    let activity = handle.activity("all");
    assert!(
        activity
            .nodes
            .iter()
            .filter(|n| n.active_blocks > 0)
            .count()
            > 40
    );
}

/// Captures one example through the engine with scripted controls; returns canonical JSON.
fn golden(name: &str) -> String {
    let program = example(name);
    let mut engine = AudioEngine::new(EngineConfig {
        real_voices: 4,
        total_voices: 8,
        buses: 4,
        ..EngineConfig::default()
    })
    .unwrap();
    let (voice, mut handle) = program.instantiate(42);
    let rate = 48000;
    match name {
        "footsteps" => {
            for (i, frame) in (0..8).map(|i| (i, 2400 + i * 11_000)) {
                handle.set("surface", (i / 3) as f32, frame).unwrap();
                handle.trigger("step", frame).unwrap();
            }
        }
        "engine" => {
            handle.set("rpm", 1000.0, 0).unwrap();
            handle.set("rpm", 5200.0, rate).unwrap();
            handle.set("load", 0.9, rate).unwrap();
        }
        "rifle" => {
            handle.trigger("fire", 0).unwrap();
            handle.set("indoor", 1.0, rate).unwrap();
            handle.trigger("fire", rate).unwrap();
        }
        "wind" => handle.set("strength", 0.9, rate / 2).unwrap(),
        _ => {}
    }
    engine
        .install(
            BusId::MASTER,
            PreparedVoice::procedural(
                Box::new(voice),
                Playback {
                    spatial: false,
                    ..Playback::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
    let mut capture = capture_audio(&mut engine, 2.0).unwrap();
    capture.analysis.graphs = vec![handle.activity(name)];
    capture.analysis_json().unwrap()
}

#[test]
fn golden_example_captures() {
    for name in ["footsteps", "wind", "engine", "ui_click", "rifle"] {
        let json = golden(name);
        assert_eq!(json, golden(name), "{name} is not deterministic");
        let path = format!("{}/tests/golden/{name}.json", env!("CARGO_MANIFEST_DIR"));
        if std::env::var_os("ARA_UPDATE_GOLDENS").is_some() {
            std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
            std::fs::write(&path, &json).unwrap();
        }
        let expected = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            json.trim_end(),
            expected.trim_end(),
            "{name}: run with ARA_UPDATE_GOLDENS=1 after intended changes"
        );
        let value = ara_json::parse(&json).unwrap();
        assert_eq!(value["clipping"], false, "{name}");
        assert_eq!(value["silent"], false, "{name}");
    }
}
