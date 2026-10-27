#![allow(clippy::unwrap_used)] // Tests may panic on unexpected failures.
//! Grids, scheduling on the sample clock, layers, transitions, stingers and loudness.

use ara_audio::{
    AudioEngine, BusId, Clip, EngineConfig, Frame, Playback, PreparedVoice, capture::capture_audio,
    effects::TrackBuilder,
};
use ara_doc::{AssetId, Document, RecordId};
use ara_music::*;
use ara_schema::{AssetRef, SchemaDb};
use std::collections::BTreeMap;

const RATE: u32 = 48000;
const BAR: u64 = 96000; // 120 BPM, 4/4.

/// Clips by reference: `@impulse/<n>` is a single click, `@tone/<hz>` a steady sine.
struct Clips(BTreeMap<String, Clip>);
impl Clips {
    fn new() -> Self {
        Self(BTreeMap::new())
    }
    fn get(&mut self, reference: &str) -> Clip {
        self.0
            .entry(reference.to_owned())
            .or_insert_with(|| {
                let tone = reference
                    .strip_prefix("@tone/")
                    .map(|hz| (hz, 0.2))
                    .or_else(|| reference.strip_prefix("@loud/").map(|hz| (hz, 0.6)));
                let frames: Vec<Frame> = if let Some((hz, amplitude)) = tone {
                    let hz: f32 = hz.parse().unwrap();
                    (0..RATE as usize * 4)
                        .map(|i| {
                            let s = (i as f32 / RATE as f32 * hz * std::f32::consts::TAU).sin()
                                * amplitude;
                            Frame::new(s, s)
                        })
                        .collect()
                } else {
                    let mut f = vec![Frame::ZERO; 4800];
                    f[0] = Frame::new(0.5, 0.5);
                    f
                };
                Clip::new(RATE, frames, 1 << 26).unwrap()
            })
            .clone()
    }
}
impl StemLoader for Clips {
    fn prepare(&mut self, clip: &str, playback: Playback) -> ara_audio::Result<PreparedVoice> {
        PreparedVoice::resident(self.get(clip), playback)
    }
}

fn stem(clip: &str, intensity: [f32; 2]) -> StemDesc {
    StemDesc {
        name: clip.into(),
        clip: Some(AssetRef(clip.into())),
        intensity,
        gain_db: 0.0,
        looping: false,
    }
}

fn segment(name: &str, bars: u32, stems: Vec<StemDesc>, next: &[(&str, f32)]) -> SegmentDesc {
    SegmentDesc {
        name: name.into(),
        bars,
        stems,
        next: next
            .iter()
            .map(|(s, w)| NextDesc {
                segment: (*s).into(),
                weight: *w,
            })
            .collect(),
        ..SegmentDesc::default()
    }
}

fn desc(seed: u32) -> MusicDesc {
    MusicDesc::new(
        vec![
            segment(
                "intro",
                1,
                vec![stem("@impulse/1", [0.0, 0.0])],
                &[("verse", 1.0)],
            ),
            segment(
                "verse",
                1,
                vec![stem("@impulse/2", [0.0, 0.0])],
                &[("verse", 1.0), ("chorus", 1.0)],
            ),
            segment(
                "chorus",
                2,
                vec![stem("@impulse/3", [0.0, 0.0])],
                &[("verse", 1.0)],
            ),
        ],
        ["chorus", "verse"]
            .map(|to| TransitionDesc {
                to: to.into(),
                fade: 0.0,
                ..TransitionDesc::default()
            })
            .to_vec(),
        vec![StingerDesc {
            name: "hit".into(),
            clip: Some(AssetRef("@impulse/9".into())),
            sync: MusicSync::Beat,
            gain_db: 0.0,
        }],
        MusicSettings {
            initial: "intro".into(),
            seed,
            ..MusicSettings::default()
        },
    )
    .unwrap()
}

fn engine() -> AudioEngine {
    AudioEngine::new(EngineConfig {
        real_voices: 32,
        total_voices: 64,
        buses: 8,
        limiter: None,
        ..EngineConfig::default()
    })
    .unwrap()
}

/// Runs the player for `frames`, calling `update` every quantum and `script` before it.
fn run(
    engine: &mut AudioEngine,
    player: &mut MusicPlayer,
    clips: &mut Clips,
    frames: u64,
    mut script: impl FnMut(&mut AudioEngine, &mut MusicPlayer, &mut Clips, u64),
) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames as usize * 2);
    let mut block = [0.0_f32; 256];
    while engine.next_frame() < frames {
        let now = engine.next_frame();
        script(engine, player, clips, now);
        player.update(engine, clips).unwrap();
        engine.render(&mut block).unwrap();
        out.extend_from_slice(&block);
    }
    out
}

fn onsets(samples: &[f32]) -> Vec<u64> {
    samples
        .chunks(2)
        .enumerate()
        .filter(|(_, f)| f[0].abs() > 0.1)
        .map(|(i, _)| i as u64)
        .collect()
}

#[test]
fn grid_maps_bars_beats_and_tempo_changes() {
    let mut s = segment("s", 4, vec![], &[]);
    s.tempo = vec![
        TempoChange::default(),
        TempoChange {
            bar: 2,
            bpm: 60.0,
            beats_per_bar: 3,
        },
    ];
    let grid = Grid::new(&s, RATE);
    assert_eq!(grid.bar(1), 96000.0);
    assert_eq!(grid.bar(3), 2.0 * 96000.0 + 3.0 * 48000.0);
    assert_eq!(grid.length(), 2.0 * 96000.0 + 2.0 * 144000.0);
    let p = grid.position(330000.0);
    assert_eq!(p.bar, 2);
    assert!((p.beat - 2.875).abs() < 1e-9);
    assert_eq!(grid.next(100.0, MusicSync::Bar), 96000.0);
    assert_eq!(grid.next(96000.0, MusicSync::Bar), 96000.0);
    assert_eq!(grid.next(100.0, MusicSync::Beat), 24000.0);
    assert_eq!(grid.next(100.0, MusicSync::End), grid.length());
}

#[test]
fn resequencing_is_bar_exact_and_seeded() {
    let session = |seed| {
        let mut e = engine();
        let mut clips = Clips::new();
        let mut player = MusicPlayer::new(desc(seed), BusId::MASTER, RATE);
        player.start(&mut e, &mut clips, None, 0).unwrap();
        let out = run(&mut e, &mut player, &mut clips, 12 * BAR, |_, _, _, _| {});
        (player.history().to_vec(), onsets(&out))
    };
    let (history, hits) = session(1);
    assert_eq!(
        history[0],
        Scheduled {
            segment: "intro".into(),
            frame: 0
        }
    );
    assert_eq!(
        history[1],
        Scheduled {
            segment: "verse".into(),
            frame: BAR
        }
    );
    for w in history.windows(2) {
        let bars = if w[0].segment == "chorus" { 2 } else { 1 };
        assert_eq!(w[1].frame - w[0].frame, bars * BAR, "{history:?}");
    }
    let starts: Vec<u64> = history
        .iter()
        .map(|s| s.frame)
        .filter(|f| *f < 12 * BAR)
        .collect();
    assert_eq!(
        hits, starts,
        "every segment's click lands on its start frame"
    );
    assert_eq!(session(1).0, history);
    let segments = |h: &[Scheduled]| h.iter().map(|s| s.segment.clone()).collect::<Vec<_>>();
    assert!(
        (2..8).any(|s| segments(&session(s).0) != segments(&history)),
        "seeds vary the order"
    );
}

#[test]
fn transitions_and_stingers_land_on_the_grid() {
    let mut e = engine();
    let mut clips = Clips::new();
    let mut player = MusicPlayer::new(desc(0), BusId::MASTER, RATE);
    player.start(&mut e, &mut clips, Some("chorus"), 0).unwrap();
    let mut landed = (0, 0);
    let out = run(&mut e, &mut player, &mut clips, 3 * BAR, |e, p, c, now| {
        if (30_000..30_128).contains(&now) {
            landed.0 = p.stinger(e, c, "hit").unwrap();
            landed.1 = p.transition(e, c, "verse").unwrap();
        }
    });
    assert_eq!(
        landed,
        (48000, BAR),
        "stinger on the next beat, transition on the next bar"
    );
    assert_eq!(onsets(&out)[..3], [0, 48000, BAR]);
    assert_eq!(player.position(BAR + 24000).unwrap().segment, "verse");
}

fn layered() -> MusicDesc {
    MusicDesc::new(
        vec![segment(
            "battle",
            2,
            vec![stem("@tone/220", [0.0, 0.0]), stem("@tone/660", [0.5, 1.0])],
            &[],
        )],
        vec![],
        vec![],
        MusicSettings {
            initial: "battle".into(),
            intensity: 0.0,
            layer_fade: 0.1,
            seed: 0,
        },
    )
    .unwrap()
}

#[test]
fn intensity_fades_layers_in_and_out() {
    let mut e = engine();
    let mut clips = Clips::new();
    let mut player = MusicPlayer::new(layered(), BusId::MASTER, RATE);
    player.start(&mut e, &mut clips, None, 0).unwrap();
    let calm = run(
        &mut e,
        &mut player,
        &mut clips,
        RATE as u64,
        |_, _, _, _| {},
    );
    player.set_intensity(&mut e, 1.0).unwrap();
    let busy = run(
        &mut e,
        &mut player,
        &mut clips,
        2 * RATE as u64,
        |_, _, _, _| {},
    );
    let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
    let tail = &busy[busy.len() / 2..];
    assert!(
        rms(tail) > rms(&calm) * 1.3,
        "{} vs {}",
        rms(tail),
        rms(&calm)
    );
}

#[test]
fn a_short_looping_tension_stem_can_fade_in_after_its_first_clip_ended() {
    let tail_energy = |looping| {
        let mut e = engine();
        let mut clips = Clips::new();
        clips.0.insert(
            "@short".into(),
            Clip::new(RATE, vec![Frame::new(0.2, 0.2); 480], 1 << 20).unwrap(),
        );
        let mut tension = stem("@short", [0.5, 1.0]);
        tension.looping = looping;
        let mut desc = layered();
        desc.segments[0].stems = vec![tension];
        let mut player = MusicPlayer::new(desc, BusId::MASTER, RATE);
        player.start(&mut e, &mut clips, None, 0).unwrap();
        let samples = run(&mut e, &mut player, &mut clips, 24_000, |e, p, _, now| {
            if (4800..4928).contains(&now) {
                p.set_intensity(e, 1.0).unwrap();
            }
        });
        samples[samples.len() / 2..]
            .iter()
            .map(|sample| sample.abs())
            .sum::<f32>()
    };
    assert!(
        tail_energy(false) < 0.01,
        "one-shot clips finish before the chase"
    );
    assert!(
        tail_energy(true) > 100.,
        "looping clips remain available for a later tension fade"
    );
}

#[test]
fn segments_normalize_to_the_music_bus_target() {
    let mut clips = Clips::new();
    let a = clips.get("@tone/220");
    let b = clips.get("@tone/660");
    let mut desc = layered();
    desc.settings.intensity = 1.0;
    desc.segments[0].loudness = measure_mix_loudness(&[(&a, 1.0), (&b, 1.0)]).unwrap();
    let mut e = engine();
    let music = e
        .add_bus("music", BusId::MASTER, TrackBuilder::new())
        .unwrap();
    e.set_loudness_target(music, Some(-16.0)).unwrap();
    let mut player = MusicPlayer::new(desc, music, RATE);
    player.start(&mut e, &mut clips, None, 0).unwrap();
    let c = capture_audio(&mut e, 3.0).unwrap();
    let lufs = c.analysis.integrated_lufs.unwrap();
    assert!((lufs + 16.0).abs() < 0.5, "{lufs}");
}

#[test]
fn documents_register_validate_and_round_trip() {
    let mut db = SchemaDb::new();
    register(&mut db).unwrap();
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    for (name, value) in [
        (
            "MusicSegment",
            ara_json::to_value(&SegmentDesc::default()).unwrap(),
        ),
        (
            "MusicTransition",
            ara_json::to_value(&TransitionDesc::default()).unwrap(),
        ),
        (
            "MusicStinger",
            ara_json::to_value(&StingerDesc::default()).unwrap(),
        ),
        (
            "MusicSettings",
            ara_json::to_value(&MusicSettings::default()).unwrap(),
        ),
        (
            "MusicTempo",
            ara_json::to_value(&TempoChange::default()).unwrap(),
        ),
    ] {
        let d = db.get(name).unwrap();
        for field in d.fields() {
            assert!(!field.doc.is_empty(), "{name}.{}", field.name);
            assert!(
                ara_json::json_eq(
                    &db.canonical_value(&field.ty, &value[&field.name]),
                    &db.canonical_value(&field.ty, &db.field_default(field))
                ),
                "{name}.{}",
                field.name
            );
        }
    }
    let original = desc(4);
    let mut doc = Document::new("music", AssetId::parse("mu_score").unwrap());
    let mut put = |id: &str, v: serde_json::Value| {
        let serde_json::Value::Object(r) = v else {
            panic!()
        };
        doc.insert_unchecked(RecordId::parse(id).unwrap(), r);
    };
    for (i, s) in original.segments.iter().enumerate() {
        put(&format!("sg_{i}"), ara_json::to_value(s).unwrap());
    }
    put(
        "tn_0",
        ara_json::to_value(&original.transitions[0]).unwrap(),
    );
    put(
        "tn_1",
        ara_json::to_value(&original.transitions[1]).unwrap(),
    );
    put(
        "st_0",
        ara_json::to_value(&original.stingers["hit"]).unwrap(),
    );
    put("ms_0", ara_json::to_value(&original.settings).unwrap());
    let text = ara_doc::write(&doc, &db);
    let read = ara_doc::read(&text, "music/score.music", &db, None);
    assert!(!ara_diag::has_errors(&read.diags), "{:?}", read.diags);
    assert_eq!(
        MusicDesc::from_document(&read.doc.unwrap()).unwrap(),
        original
    );
    let mut bad = segment("bad", 1, vec![], &[("ghost", 1.0)]);
    bad.tempo = vec![TempoChange {
        bar: 1,
        ..TempoChange::default()
    }];
    let errors = MusicDesc::new(vec![bad], vec![], vec![], MusicSettings::default()).unwrap_err();
    assert_eq!(errors.len(), 2, "{errors:?}");
}

#[test]
fn golden_music_session() {
    let session = || {
        let mut e = engine();
        let mut clips = Clips::new();
        let mut layered = layered();
        layered.segments.push(segment(
            "calm",
            1,
            vec![stem("@tone/330", [0.0, 0.0])],
            &[("battle", 1.0)],
        ));
        let desc = MusicDesc::new(
            layered.segments,
            vec![TransitionDesc {
                to: "battle".into(),
                sync: MusicSync::Bar,
                fade: 0.25,
                ..TransitionDesc::default()
            }],
            vec![StingerDesc {
                name: "hit".into(),
                clip: Some(AssetRef("@impulse/9".into())),
                sync: MusicSync::Beat,
                gain_db: -6.0,
            }],
            MusicSettings {
                initial: "calm".into(),
                intensity: 0.2,
                layer_fade: 0.5,
                seed: 7,
            },
        )
        .unwrap();
        let mut player = MusicPlayer::new(desc, BusId::MASTER, RATE);
        player.start(&mut e, &mut clips, None, 0).unwrap();
        let samples = run(
            &mut e,
            &mut player,
            &mut clips,
            4 * RATE as u64,
            |e, p, c, now| match now {
                25_600 => {
                    p.stinger(e, c, "hit").unwrap();
                }
                51_200 => {
                    p.transition(e, c, "battle").unwrap();
                }
                128_000 => p.set_intensity(e, 1.0).unwrap(),
                _ => {}
            },
        );
        let capture = ara_audio::capture::analyze(RATE, &samples, 0).unwrap();
        let value = serde_json::json!({
            "capture": ara_json::to_value(&capture.analysis).unwrap(),
            "schedule": ara_json::to_value(player.history()).unwrap(),
        });
        ara_json::to_canonical_string(&ara_json::sort_keys(&value))
    };
    let json = session();
    assert_eq!(json, session(), "music sessions are deterministic");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/session.json");
    if std::env::var_os("ARA_UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
        std::fs::write(path, &json).unwrap();
    }
    let expected = std::fs::read_to_string(path).unwrap();
    assert!(
        json.trim_end() == expected.trim_end(),
        "run with ARA_UPDATE_GOLDENS=1 after intended changes"
    );
    assert!(json.contains("\"segment\": \"battle\""));
}

#[test]
fn late_updates_and_unpromoted_segments_keep_the_music_going() {
    let mut e = engine();
    let mut clips = Clips::new();
    let mut player = MusicPlayer::new(desc(3), BusId::MASTER, RATE);
    player.start(&mut e, &mut clips, None, 0).unwrap();
    // Render well past the intro without any update: the follow-up starts late, not never.
    capture_audio(&mut e, 2.5).unwrap();
    player.update(&mut e, &mut clips).unwrap();
    let late = player.history()[1].frame;
    assert!(late >= (2.5 * RATE as f64) as u64, "{:?}", player.history());
    // The verse is queued; render past its start, then transition before any update.
    run(
        &mut e,
        &mut player,
        &mut clips,
        late + BAR - 1000,
        |_, _, _, _| {},
    );
    let queued = player.history().last().unwrap().frame;
    let mut block = [0.0_f32; 4000];
    while e.next_frame() < queued + 2000 {
        e.render(&mut block).unwrap();
    }
    let landed = player.transition(&mut e, &mut clips, "chorus").unwrap();
    assert_eq!(
        landed,
        queued + BAR,
        "the boundary comes from the segment now playing"
    );
    // Intensity spam while voices wait for their start never fills command queues.
    for _ in 0..500 {
        player.set_intensity(&mut e, 0.3).unwrap();
        player.set_intensity(&mut e, 0.7).unwrap();
    }
}

#[test]
fn segment_loudness_trims_respect_the_stem_peak() {
    let mut clips = Clips::new();
    let tone = clips.get("@loud/440");
    let mut desc = layered();
    desc.segments[0].stems.truncate(1);
    desc.segments[0].stems[0].clip = Some(AssetRef("@loud/440".into()));
    // Claimed quieter than reality: the bus would boost +12 dB without the peak cap.
    desc.segments[0].loudness = Some(-40.0);
    let mut e = engine();
    let music = e
        .add_bus("music", BusId::MASTER, TrackBuilder::new())
        .unwrap();
    e.set_loudness_target(music, Some(-16.0)).unwrap();
    let mut player = MusicPlayer::new(desc, music, RATE);
    player.start(&mut e, &mut clips, None, 0).unwrap();
    let c = capture_audio(&mut e, 1.0).unwrap();
    assert!(
        c.analysis.sample_peak <= 1.0 + 1e-6,
        "{}",
        c.analysis.sample_peak
    );
    assert!(
        c.analysis.sample_peak > f64::from(tone.peak()) * 1.5,
        "boosted until the peak cap binds"
    );
}
