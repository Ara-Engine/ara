#![allow(clippy::unwrap_used)]
use ara_audio::{
    device::{DeviceBackend, DeviceEvent, Latency, pump},
    effects::*,
    *,
};

#[test]
fn sidechain_ducks_and_releases_a_submix() {
    let mut e = AudioEngine::new(EngineConfig::default()).unwrap();
    let speech = e
        .add_bus("speech", BusId::MASTER, TrackBuilder::new())
        .unwrap();
    let duck = Ducking {
        source: e.bus_meter(speech).unwrap().clone(),
        threshold: 0.1,
        gain: 0.25,
        attack: 0.001,
        release: 0.001,
    }
    .build()
    .unwrap();
    let music = e
        .add_bus(
            "music",
            BusId::MASTER,
            TrackBuilder::new().with_built_effect(duck),
        )
        .unwrap();
    e.apply_snapshot(
        &Snapshot {
            name: "meter without monitoring".into(),
            gains: vec![(speech, 0.0)],
        },
        0,
    )
    .unwrap();
    let c = Clip::new(48000, vec![Frame::new(0.5, 0.5); 2048], 16384).unwrap();
    e.play(
        music,
        c.clone(),
        Playback {
            spatial: false,
            ..Default::default()
        },
    )
    .unwrap();
    let trigger = e
        .play(
            speech,
            c,
            Playback {
                spatial: false,
                ..Default::default()
            },
        )
        .unwrap();
    let ducked = e.render_frames(1024).unwrap();
    assert!(
        (ducked[1023].left - 0.125).abs() < 0.001,
        "sample {}, speech peak {}",
        ducked[1023].left,
        e.bus_meter(speech).unwrap().peak()
    );
    e.command(
        trigger,
        VoiceCommand::Stop {
            at_frame: 1024,
            fade_frames: 0,
        },
    )
    .unwrap();
    let released = e.render_frames(768).unwrap();
    assert!((released[767].left - 0.5).abs() < 0.001);
}

#[test]
fn full_bus_effect_chain_has_a_finite_audible_tail() {
    let mut e = AudioEngine::new(EngineConfig::default()).unwrap();
    let builder = TrackBuilder::new()
        .with_effect(EqFilterBuilder::new(EqFilterKind::Bell, 1000.0, 3.0, 0.7))
        .with_effect(FilterBuilder::new().mode(FilterMode::HighPass).cutoff(80.0))
        .with_effect(
            FilterBuilder::new()
                .mode(FilterMode::LowPass)
                .cutoff(6000.0),
        )
        .with_effect(CompressorBuilder::new().threshold(-12.0).ratio(4.0))
        .with_effect(DelayBuilder::new(std::time::Duration::from_millis(20), 0.25, 0.4).unwrap())
        .with_effect(ReverbBuilder::new().feedback(0.6).mix(0.3));
    let bus = e.add_bus("room", BusId::MASTER, builder).unwrap();
    let mut frames = vec![Frame::ZERO; 128];
    frames[0] = Frame::new(0.5, 0.5);
    e.play(
        bus,
        Clip::new(48000, frames, 1024).unwrap(),
        Playback {
            spatial: false,
            ..Default::default()
        },
    )
    .unwrap();
    let output = e.render_frames(24000).unwrap();
    assert!(
        output
            .iter()
            .all(|f| f.left.is_finite() && f.right.is_finite())
    );
    assert!(output[512..].iter().any(|f| f.left.abs() > 0.0001));
}

struct FakeDevice {
    lost: bool,
    frames: usize,
}
impl DeviceBackend for FakeDevice {
    fn sample_rate(&self) -> u32 {
        48000
    }
    fn minimum_queue_frames(&self) -> usize {
        608
    }
    fn poll(&mut self) -> Result<DeviceEvent> {
        Ok(if self.lost {
            DeviceEvent::Lost
        } else {
            DeviceEvent::Ready
        })
    }
    fn submit(&mut self, s: &[f32]) -> Result<()> {
        self.frames += s.len() / 2;
        Ok(())
    }
    fn queued_frames(&self) -> Result<usize> {
        Ok(self.frames)
    }
    fn latency(&self) -> Result<Latency> {
        Ok(Latency {
            queued_frames: self.frames,
            estimated_output_ms: 0.0,
        })
    }
    fn recover(&mut self) -> Result<DeviceEvent> {
        self.lost = false;
        Ok(DeviceEvent::Recovered)
    }
}
#[test]
fn device_period_sets_a_safe_watermark_and_loss_preserves_graph() {
    let mut engine = AudioEngine::new(EngineConfig::default()).unwrap();
    let mut sink = FakeDevice {
        lost: false,
        frames: 0,
    };
    pump(&mut engine, &mut sink, 256).unwrap();
    assert_eq!(sink.frames, 608);
    let frame = engine.next_frame();
    sink.lost = true;
    assert_eq!(
        pump(&mut engine, &mut sink, 256).unwrap(),
        DeviceEvent::Lost
    );
    assert_eq!(engine.next_frame(), frame);
    sink.recover().unwrap();
    sink.frames = 0;
    pump(&mut engine, &mut sink, 256).unwrap();
    assert!(engine.next_frame() > frame);
}

#[test]
fn acoustic_send_feeds_a_shared_wet_return() {
    let mut e = AudioEngine::new(EngineConfig::default()).unwrap();
    let send = e
        .add_send(SendTrackBuilder::new().with_effect(
            DelayBuilder::new(std::time::Duration::from_millis(10), 0.0, 1.0).unwrap(),
        ))
        .unwrap();
    let bus = e
        .add_bus(
            "dry",
            BusId::MASTER,
            TrackBuilder::new().with_send(send, -60.0),
        )
        .unwrap();
    e.set_send(bus, send, 0.5, 0).unwrap();
    let mut samples = vec![Frame::ZERO; 128];
    samples[0] = Frame::new(0.5, 0.5);
    e.play(
        bus,
        Clip::new(48000, samples, 1024).unwrap(),
        Playback {
            spatial: false,
            ..Default::default()
        },
    )
    .unwrap();
    let output = e.render_frames(1024).unwrap();
    assert!(output[450..600].iter().any(|f| f.left.abs() > 0.1));
}
