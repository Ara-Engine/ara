#![cfg(feature = "gpu")]
#![allow(clippy::unwrap_used)]
use ara_profile::*;

#[test]
fn resolved_timestamp_import_preserves_submission_frame_and_queue() {
    let mut p = Profiler::new(Config {
        history: 2,
        ..Config::default()
    })
    .unwrap();
    let original = p.begin_frame().unwrap();
    p.end_frame(Default::default(), Default::default()).unwrap();
    p.begin_frame().unwrap();
    p.gpu_timings(
        original.id(),
        &[
            ara_gpu::commands::PassTiming {
                pass: "shadow".into(),
                queue: ara_gpu::Queue::Graphics,
                milliseconds: 1.25,
                start_ms: 0.,
                end_ms: 1.25,
            },
            ara_gpu::commands::PassTiming {
                pass: "cull".into(),
                queue: ara_gpu::Queue::Compute,
                milliseconds: 0.5,
                start_ms: 0.,
                end_ms: 0.5,
            },
            ara_gpu::commands::PassTiming {
                pass: "invalid".into(),
                queue: ara_gpu::Queue::Graphics,
                milliseconds: f64::NAN,
                start_ms: 0.,
                end_ms: f64::NAN,
            },
        ],
    );
    p.end_frame(Default::default(), Default::default()).unwrap();
    let frame = &p.profile().frames[0];
    assert_eq!(frame.gpu.len(), 2);
    assert_eq!(frame.dropped, 1);
    assert_eq!(frame.gpu[0].duration_ns, 1_250_000);
    assert_eq!(p.profile().labels[frame.gpu[1].queue as usize], "compute");
    assert!(p.profile().frames[1].gpu.is_empty());
}

/// Explicit hardware gate; ordinary profiling tests need no GPU or validation layers.
#[test]
#[ignore = "requires Vulkan 1.3 and validation layers; run with --ignored"]
fn real_vulkan_pass_timestamp_correlates_after_next_cpu_frame()
-> Result<(), Box<dyn std::error::Error>> {
    use ara_gpu::{GpuDevice, Queue, commands::Command};
    let cache = std::env::temp_dir().join(format!("ara-profile-gpu-{}", std::process::id()));
    let mut gpu = GpuDevice::new(&[], &cache)?;
    let mut p = Profiler::new(Config {
        history: 2,
        ..Config::default()
    })?;
    let frame = p.begin_frame()?;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.push(Command::BeginPass("profile.timestamp".into()));
    commands.push(Command::EndPass);
    let submission = gpu.submit(commands.finish(), &[])?;
    p.end_frame(Default::default(), Default::default())?;
    p.begin_frame()?;
    p.gpu_timings(frame.id(), &gpu.resolve(submission)?);
    p.end_frame(Default::default(), Default::default())?;
    assert_eq!(p.profile().frames[0].gpu.len(), 1);
    assert!(p.profile().frames[1].gpu.is_empty());
    gpu.assert_clean()?;
    Ok(())
}
