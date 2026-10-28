use super::*;

#[test]
fn recorder_is_off_and_unallocated_until_explicit_control() {
    let mut recorder = Recorder::default();
    assert!(recorder.begin().is_none());
    assert!(recorder.profiler.is_none());
}

#[test]
fn completed_frames_are_bounded_and_late_gpu_samples_keep_their_submitting_frame() {
    let mut recorder = Recorder::default();
    recorder.ensure().expect("recorder").start();
    let mut first = 0;
    for index in 0..3 {
        let frame = recorder.begin().expect("recording frame");
        if index == 0 {
            first = frame;
        }
        {
            let _scope = ara_profile::scope("player.test.work");
            std::hint::black_box(index);
        }
        recorder.end(7);
    }
    recorder.gpu(
        first,
        &[ara_gpu::commands::PassTiming {
            pass: "actual.gpu.pass".into(),
            queue: ara_gpu::Queue::Graphics,
            milliseconds: 1.25,
            start_ms: 0.,
            end_ms: 1.25,
        }],
    );
    let profiler = recorder.profiler.as_ref().expect("recording");
    assert_eq!(profiler.profile().frames[0].gpu.len(), 1);
    assert!(profiler.profile().frames[1].gpu.is_empty());
    assert_eq!(profiler.profile().frames[0].gpu[0].duration_ns, 1_250_000);
    assert!(
        profiler
            .profile()
            .frames
            .iter()
            .all(|frame| frame.counters.entities == 7 && !frame.spans.is_empty())
    );
    for _ in 0..125 {
        recorder.begin().expect("frame");
        recorder.end(7);
    }
    assert_eq!(
        recorder
            .profiler
            .as_ref()
            .expect("recorder")
            .profile()
            .frames
            .len(),
        120
    );
    assert!(!recorder.open);
}

#[test]
fn snapshots_exclude_the_unfinished_frame_and_keep_recording() {
    let mut recorder = Recorder::default();
    recorder.ensure().expect("recorder").start();
    let first = recorder.begin().expect("first");
    recorder.end(3);
    recorder.begin().expect("unfinished second");
    let captured = completed_profile(recorder.profiler.as_ref().expect("recorder"), recorder.open);
    assert_eq!(captured.frames.len(), 1);
    assert_eq!(captured.frames[0].id, first);
    assert!(recorder.recording());
    recorder.end(3);
}
