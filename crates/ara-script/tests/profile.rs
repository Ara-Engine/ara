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
        kind: ara_script::abi::RELOAD_L3,
        escalation: 0,
        generation: 1,
        delta_count: 0,
        changed_members: 0,
        error_count: 0,
        hot: 1,
        server_us: 1234,
        t_received: 100,
        t_staged: 200,
        t_applied: 300,
        bytes: 32,
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
