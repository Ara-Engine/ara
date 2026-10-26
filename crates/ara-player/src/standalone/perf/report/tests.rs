use super::*;
use clap::Parser;

#[test]
fn keepalive_and_new_requests_retain_cpu_hitches_until_stop_without_changing_gpu_samples()
-> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-player/perf-keepalive")
        .join(std::process::id().to_string());
    let control = root.join("control");
    std::fs::create_dir_all(&control)?;
    let output = root.join("player.json");
    let args = crate::Args::try_parse_from([
        "ara-player",
        "--perf-output",
        output.to_str().context("UTF-8 output")?,
        "--perf-control",
        control.to_str().context("UTF-8 control")?,
        "--perf-wait-ready",
        "--keep-alive",
    ])?;
    // The presenter stays unprepared: this control/report test uses no GPU.
    let mut presenter = Presenter::new([32, 24], Vec::new(), root.join("cache"));
    let mut window = Window::new(args.standalone.perf, true, Instant::now());
    let frame = |ms, first| CpuFrame {
        total_ms: ms,
        wait_ms: Some(2.),
        first_presented: first,
        phases: [3., 1., 4., 0.5],
    };
    // A previous GPU window is complete; keepalive still presents Game frames.
    window.collecting = false;
    window.last_report = Some(json!({"frames":600,"gpu_span_samples_ms":[1.25]}));
    window.frame(&presenter, &BTreeMap::new(), Value::Null, frame(40., true))?;
    window.frame(&presenter, &BTreeMap::new(), Value::Null, frame(25., false))?;
    std::fs::write(control.join("01-measure.json"), br#"{"measure":true}"#)?;
    assert!(!window.control(&mut presenter)?);
    assert!(window.collecting);
    assert_eq!(
        window.run_cpu.count(),
        2,
        "measure must not erase earlier frames"
    );
    window.frame(&presenter, &BTreeMap::new(), Value::Null, frame(18., false))?;
    assert_eq!(window.seen, 1, "the new GPU readiness window starts afresh");
    window.collecting = false;
    window.frame(&presenter, &BTreeMap::new(), Value::Null, frame(22., false))?;
    std::fs::write(control.join("02-stop.json"), br#"{"stop":true}"#)?;
    window.next_control = Instant::now();
    assert!(window.control(&mut presenter)?);
    window.finish()?;
    let report: Value = serde_json::from_slice(&std::fs::read(output)?)?;
    assert_eq!(report["frames"], 600);
    assert_eq!(report["gpu_span_samples_ms"], json!([1.25]));
    let cpu = &report["run"]["cpu"];
    assert_eq!(cpu["frames"], 4);
    assert_eq!(cpu["frame_samples_ms"], json!([40., 25., 18., 22.]));
    assert_eq!(cpu["hitch_samples"], 3);
    assert_eq!(cpu["hitches_over_16ms"], 3);
    assert_eq!(cpu["work_coverage"], json!({"measured":4,"unavailable":0}));
    let hitches = &report["run"]["hitches"]["frames"];
    assert_eq!(hitches[0]["frame"], 2);
    assert_eq!(hitches[0]["collecting_gpu_window"], false);
    assert_eq!(hitches[0]["host_wait_ms"], 2.);
    assert_eq!(hitches[0]["work_ms"], 23.);
    assert_eq!(hitches[0]["presentation_including_display_ms"], 4.);
    assert_eq!(hitches[1]["frame"], 3);
    assert_eq!(hitches[1]["collecting_gpu_window"], true);
    assert_eq!(hitches[2]["frame"], 4);
    assert_eq!(hitches[2]["collecting_gpu_window"], false);
    assert_eq!(report["run"]["hitches"]["omitted"], 0);
    Ok(())
}
