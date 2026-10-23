//! Explicit offline, benchmark and device probes. No device is opened by normal tests.
use ara_audio::{
    capture::capture_audio,
    device::{DeviceBackend, SdlBackend, pump},
    steam::SteamAudio,
    *,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

thread_local! {
    static COUNT:Cell<bool>=const {Cell::new(false)};
    static ALLOCS:Cell<usize>=const {Cell::new(0)};
    static FREES:Cell<usize>=const {Cell::new(0)};
}
struct CountingAllocator;
// SAFETY: delegates every allocation to System with unchanged pointers/layouts; thread-local counters allocate nothing.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNT.with(|c| {
            if c.get() {
                ALLOCS.with(|a| a.set(a.get() + 1));
            }
        });
        // SAFETY: forwards the caller's valid GlobalAlloc layout unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        COUNT.with(|c| {
            if c.get() {
                FREES.with(|a| a.set(a.get() + 1));
            }
        });
        // SAFETY: forwards the original live allocation pointer and matching layout unchanged.
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        COUNT.with(|c| {
            if c.get() {
                ALLOCS.with(|a| a.set(a.get() + 1));
                FREES.with(|a| a.set(a.get() + 1));
            }
        });
        // SAFETY: forwards the live pointer, original layout and requested nonzero size unchanged.
        unsafe { System.realloc(p, layout, new) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "offline".into());
    if mode == "canonical" {
        use std::io::Read;
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        print!(
            "{}",
            ara_json::to_canonical_string(&ara_json::sort_keys(&ara_json::parse(&text)?))
        );
        return Ok(());
    }
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts");
    std::fs::create_dir_all(&directory)?;
    let native = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime");
    let steam_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| native.join("steam/steamaudio/lib/windows-x64/phonon.dll"));
    let steam = if steam_path.exists() {
        Some(SteamAudio::load(&steam_path, 48000, None)?)
    } else {
        None
    };
    match mode.as_str() {
        "offline" => offline(&directory, steam.as_ref())?,
        "bench" => bench(&directory, steam.as_ref())?,
        "device" => device(&directory, steam.as_ref())?,
        #[cfg(windows)]
        "latency" => latency()?,
        _ => return Err("usage: verify [offline|bench|device] [phonon-library-path]".into()),
    }
    Ok(())
}

fn output() -> Result<(Box<dyn DeviceBackend>, serde_json::Value)> {
    let selected = std::env::var("ARA_AUDIO_BACKEND").unwrap_or_else(|_| {
        if cfg!(windows) {
            "wasapi".into()
        } else {
            "sdl".into()
        }
    });
    #[cfg(windows)]
    if selected == "wasapi" {
        let backend = ara_audio::wasapi::WasapiBackend::open(
            48000,
            ara_audio::wasapi::WasapiOptions::default(),
        )?;
        let details = serde_json::json!({"driver":"IAudioClient3 RAW GameEffects", "hardwareFrames":backend.period_frames()?,"capacityFrames":backend.capacity_frames()?,"reportedLatencyMs":backend.reported_latency_ms()?,"device":backend.endpoint_id()?});
        return Ok((Box::new(backend), details));
    }
    if selected != "sdl" {
        return Err(AudioError::Invalid(
            "ARA_AUDIO_BACKEND must be sdl or wasapi (Windows)",
        ));
    }
    let backend = SdlBackend::open(48000)?;
    let details = serde_json::json!({"driver":backend.driver()?,"hardwareFrames":backend.format().buffer_frames,"device":backend.name()?});
    Ok((Box::new(backend), details))
}

#[cfg(windows)]
fn latency() -> Result<()> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryPerformanceCounter(value: *mut i64) -> i32;
        fn QueryPerformanceFrequency(value: *mut i64) -> i32;
    }
    let mut frequency = 0;
    // SAFETY: output points to writable i64; Windows exposes a process-independent QPC clock.
    if unsafe { QueryPerformanceFrequency(&mut frequency) } == 0 {
        return Err(AudioError::Backend("QPC frequency".into()));
    }
    let mut random = 123_u32;
    let frames = (0..768)
        .map(|_| {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let s = if random & 0x80000000 == 0 { 0.2 } else { -0.2 };
            Frame::new(s, s)
        })
        .collect();
    let clip = Clip::new(48000, frames, 768 * 8)?;
    let mut e = AudioEngine::new(EngineConfig::default())?;
    let (mut backend, details) = output()?;
    let count = std::env::var("ARA_AUDIO_LATENCY_TRIALS")
        .map_or(Ok(8), |v| v.parse::<usize>())
        .map_err(|_| AudioError::Invalid("latency trial count"))?;
    if !(8..=128).contains(&count) {
        return Err(AudioError::Invalid("latency trial count must be 8..128"));
    }
    let start = Instant::now();
    let mut trials = Vec::with_capacity(count);
    let mut scheduled = Vec::with_capacity(count);
    let mut first_submit_ms = Vec::with_capacity(count);
    let mut cached_frames = Vec::with_capacity(count);
    let mut queued_frames = Vec::with_capacity(count);
    while start.elapsed().as_secs_f64() < count as f64 * 0.25 + 1.0 {
        if trials.len() < count && start.elapsed().as_secs_f64() >= 0.5 + trials.len() as f64 * 0.25
        {
            let mut counter = 0;
            // SAFETY: output points to writable i64 for the shared WASAPI timestamp clock.
            if unsafe { QueryPerformanceCounter(&mut counter) } == 0 {
                return Err(AudioError::Backend("QPC counter".into()));
            }
            e.play(
                BusId::MASTER,
                clip.clone(),
                Playback {
                    at_frame: e.next_frame(),
                    spatial: false,
                    ..Default::default()
                },
            )?;
            trials.push((counter as f64 / frequency as f64 * 10000000.0) as u64);
            scheduled.push(e.next_frame());
            first_submit_ms.push(None);
            cached_frames.push(e.next_frame() - e.delivered_frames());
            queued_frames.push(backend.queued_frames()?);
        }
        pump(&mut e, backend.as_mut(), QUANTUM * 2)?;
        if scheduled
            .iter()
            .zip(&first_submit_ms)
            .any(|(&frame, submitted)| submitted.is_none() && e.delivered_frames() > frame)
        {
            let mut counter = 0;
            // SAFETY: writable QPC output on the same clock as command and loopback timestamps.
            if unsafe { QueryPerformanceCounter(&mut counter) } == 0 {
                return Err(AudioError::Backend("QPC counter".into()));
            }
            let now = counter as f64 / frequency as f64 * 10000000.0;
            for ((&frame, submitted), &command) in
                scheduled.iter().zip(&mut first_submit_ms).zip(&trials)
            {
                if submitted.is_none() && e.delivered_frames() > frame {
                    *submitted = Some((now - command as f64) / 10000.0);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut value = details;
    value["commandQpc100ns"] = serde_json::json!(trials);
    value["submittedFrames"] = serde_json::json!(e.delivered_frames());
    value["commandToFirstSubmitMs"] = serde_json::json!(first_submit_ms);
    value["cachedFramesAtCommand"] = serde_json::json!(cached_frames);
    value["queuedFramesAtCommand"] = serde_json::json!(queued_frames);
    value["latencyEstimate"] =
        serde_json::to_value(backend.latency()?).map_err(|e| AudioError::Backend(e.to_string()))?;
    println!(
        "{}",
        ara_json::to_canonical_string(&ara_json::sort_keys(&value))
    );
    Ok(())
}
fn door() -> Result<Clip> {
    let mut random = 42_u32;
    let frames = (0..24000)
        .map(|i| {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = ((random >> 16) as i32 - 32768) as f32 / 32768.0;
            let time = i as f32 / 48000.0;
            let value = (noise * 0.15 + (time * std::f32::consts::TAU * 330.0).sin() * 0.4)
                * (-time * 12.0).exp();
            Frame::new(value, value)
        })
        .collect();
    Clip::new(48000, frames, 24000 * 8)
}
fn offline(directory: &Path, steam: Option<&SteamAudio>) -> Result<()> {
    let mut e = AudioEngine::new(EngineConfig::default())?;
    let clip = door()?;
    for (at, direction) in [(0, [-0.8, 0.0, -0.6]), (48000, [0.8, 0.0, -0.6])] {
        let spatializer = steam
            .map(|s| {
                s.spatializer()
                    .map(|s| Box::new(s) as Box<dyn ara_audio::spatial::Spatializer>)
            })
            .transpose()?;
        e.play(
            BusId::MASTER,
            clip.clone(),
            Playback {
                at_frame: at,
                parameters: VoiceParameters {
                    direction,
                    ..Default::default()
                },
                spatializer,
                ..Default::default()
            },
        )?;
    }
    let capture = capture_audio(&mut e, 2.0)?;
    capture.write(
        directory.join("spatial.wav"),
        directory.join("spatial.json"),
    )?;
    println!("{}", capture.analysis_json()?);
    Ok(())
}
fn bench(directory: &Path, steam: Option<&SteamAudio>) -> Result<()> {
    let mut results = Vec::new();
    for (hrtf, real) in [(false, 1000), (true, 1000), (true, 128)] {
        if hrtf && steam.is_none() {
            continue;
        }
        let config = EngineConfig {
            real_voices: real,
            total_voices: 1000,
            buses: 4,
            ..Default::default()
        };
        let mut engine = AudioEngine::new(config)?;
        let clip = door()?;
        for i in 0..1000 {
            let angle = i as f32 * std::f32::consts::TAU / 1000.0;
            let spatializer = if hrtf {
                steam
                    .map(|s| {
                        s.spatializer()
                            .map(|s| Box::new(s) as Box<dyn ara_audio::spatial::Spatializer>)
                    })
                    .transpose()?
            } else {
                None
            };
            engine.play(
                BusId::MASTER,
                clip.clone(),
                Playback {
                    looping: true,
                    parameters: VoiceParameters {
                        gain: 0.001,
                        direction: [angle.sin(), 0.0, -angle.cos()],
                        ..Default::default()
                    },
                    spatializer,
                    ..Default::default()
                },
            )?;
        }
        let mut output = [0.0; QUANTUM * 2];
        for _ in 0..32 {
            engine.render(&mut output)?;
        }
        let mut times = Vec::with_capacity(256);
        let mut allocations = 0;
        let mut frees = 0;
        for _ in 0..256 {
            ALLOCS.with(|c| c.set(0));
            FREES.with(|c| c.set(0));
            COUNT.with(|c| c.set(true));
            let start = Instant::now();
            let result = engine.render(&mut output);
            let elapsed = start.elapsed();
            COUNT.with(|c| c.set(false));
            result?;
            allocations += ALLOCS.with(Cell::get);
            frees += FREES.with(Cell::get);
            times.push(elapsed.as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        results.push(serde_json::json!({"voices":1000,"spatializer":if hrtf {"Steam Audio bilinear HRTF + direct effect"} else {"equal-power stereo"},
            "realVoices":real,"quantumFrames":QUANTUM,"sampleRate":48000,"samples":times.len(),"meanMs":times.iter().sum::<f64>()/times.len() as f64,
            "p50Ms":times[128],"p95Ms":times[243],"maxMs":times[255],"allocations":allocations,"deallocations":frees}));
    }
    let mut initial = AudioEngine::new(EngineConfig::default())?;
    let spatializer = steam
        .map(|s| {
            s.spatializer()
                .map(|s| Box::new(s) as Box<dyn ara_audio::spatial::Spatializer>)
        })
        .transpose()?;
    let prepared = PreparedVoice::resident(
        door()?,
        Playback {
            spatializer,
            ..Default::default()
        },
    )?;
    let mut output = [0.0; QUANTUM * 2];
    ALLOCS.with(|c| c.set(0));
    FREES.with(|c| c.set(0));
    COUNT.with(|c| c.set(true));
    let lifecycle = (|| {
        let id = initial.install(BusId::MASTER, prepared)?;
        initial.command(
            id,
            VoiceCommand::Stop {
                at_frame: 256,
                fade_frames: 64,
            },
        )?;
        for _ in 0..32 {
            initial.render(&mut output)?;
        }
        Ok::<(), AudioError>(())
    })();
    COUNT.with(|c| c.set(false));
    lifecycle?;
    let value = serde_json::json!({"measurements":results,"preparedFirstRenderAndStop":{
        "allocations":ALLOCS.with(Cell::get),"deallocations":FREES.with(Cell::get),"quanta":32,"steamHrtf":steam.is_some()}});
    let json = ara_json::to_canonical_string(&ara_json::sort_keys(&value));
    std::fs::write(directory.join("benchmark.json"), &json)?;
    println!("{json}");
    Ok(())
}
fn device(directory: &Path, steam: Option<&SteamAudio>) -> Result<()> {
    let mut engine = AudioEngine::new(EngineConfig::default())?;
    let spatializer = steam
        .map(|s| {
            s.spatializer()
                .map(|s| Box::new(s) as Box<dyn ara_audio::spatial::Spatializer>)
        })
        .transpose()?;
    engine.play(
        BusId::MASTER,
        door()?,
        Playback {
            looping: true,
            spatializer,
            parameters: VoiceParameters {
                gain: 0.2,
                ..Default::default()
            },
            ..Default::default()
        },
    )?;
    let (mut backend, mut value) = output()?;
    let start = Instant::now();
    let mut max_queue = 0;
    let mut recovered = false;
    let mut max_pump_ms = 0.0_f64;
    while start.elapsed() < Duration::from_secs(3) {
        let before = Instant::now();
        pump(&mut engine, backend.as_mut(), QUANTUM * 2)?;
        max_pump_ms = max_pump_ms.max(before.elapsed().as_secs_f64() * 1000.0);
        max_queue = max_queue.max(backend.queued_frames()?);
        if !recovered && start.elapsed() > Duration::from_millis(1500) {
            backend.recover()?;
            recovered = true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let latency = backend.latency()?;
    let measurement = serde_json::json!({"deviceRate":backend.sample_rate(),
        "submittedFrames":engine.delivered_frames(),"maxQueuedFrames":max_queue,"latencyEstimate":latency,"maxPumpMs":max_pump_ms,"reopenSucceeded":recovered,
        "note":"Known queue + period estimate; independent loopback.json measures command to output mix."});
    if let (Some(details), Some(measurement)) = (value.as_object_mut(), measurement.as_object()) {
        details.extend(measurement.clone());
    }
    let json = ara_json::to_canonical_string(&ara_json::sort_keys(&value));
    std::fs::write(directory.join("device.json"), &json)?;
    println!("{json}");
    Ok(())
}
