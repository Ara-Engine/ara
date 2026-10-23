#![cfg(feature = "profiling")]
#![allow(clippy::unwrap_used)]
#[test]
fn compile_server_report_stays_separate_from_tick_cpu_time() {
    let mut profiler = ara_profile::Profiler::new(ara_profile::Config {
        history: 1,
        ..Default::default()
    })
    .unwrap();
    let frame = profiler.begin_frame().unwrap();
    let report = ara_script::abi::ReloadReport {
        module: 7,
        server_us: 1234,
        ..Default::default()
    };
    ara_script::Boundary {
        reloaded: true,
        reports: vec![report],
    }
    .record_profile(&mut profiler, frame.id());
    profiler
        .end_frame(Default::default(), Default::default())
        .unwrap();
    let recorded = &profiler.profile().frames[0];
    assert!(recorded.spans.is_empty());
    assert_eq!(recorded.jobs[0].duration_ns, 1_234_000);
    assert_eq!(recorded.jobs[0].worker, 7);
    assert_eq!(
        profiler.profile().labels[recorded.jobs[0].label as usize],
        "compile.server"
    );
}
