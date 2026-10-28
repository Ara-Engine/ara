#![allow(clippy::unwrap_used)]
use super::*;
fn frame(names: &[&str]) -> Frame {
    Frame::from_names(names.iter().copied())
}
fn guard() -> Guard {
    Guard::new(Expected::Hybrid, 3, Duration::from_secs(2))
}

#[test]
fn ssr_and_partial_rt_never_open_the_window() {
    let mut g = guard();
    for passes in [
        &["GBuffer", "SSR"][..],
        &HYBRID[..5],
        &["RT.Sun", "SSR"][..],
    ] {
        for _ in 0..8 {
            assert!(!g.wait(Duration::ZERO, Some(frame(passes))).unwrap());
        }
    }
    let error = g
        .wait(Duration::from_secs(2), Some(frame(HYBRID)))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with(&format!("{INVALID_PASS_SET}:"))
    );
    assert_eq!(error.evidence()["status"], "invalid");
    assert!(validate(&g.report(), 1).is_err());
}
#[test]
fn readiness_requires_consecutive_complete_frames_and_a_bounded_deadline() {
    let mut g = guard();
    assert!(!g.wait(Duration::ZERO, Some(frame(HYBRID))).unwrap());
    assert!(!g.wait(Duration::ZERO, Some(frame(HYBRID))).unwrap());
    assert!(!g.wait(Duration::ZERO, Some(frame(&["SSR"]))).unwrap());
    for _ in 0..2 {
        assert!(!g.wait(Duration::ZERO, Some(frame(HYBRID))).unwrap());
    }
    assert!(!g.wait(Duration::ZERO, None).unwrap());
    assert!(g.wait(Duration::from_secs(1), Some(frame(HYBRID))).unwrap());
    g.record(frame(HYBRID)).unwrap();
    assert!(validate(&g.report(), 1).is_ok());
    assert_eq!(g.report()["rt_present"], "1/1 frames");
}
#[test]
fn any_missing_timed_pass_fails_the_whole_window_permanently() {
    for missing in 0..HYBRID.len() {
        let mut g = guard();
        for _ in 0..3 {
            g.wait(Duration::ZERO, Some(frame(HYBRID))).unwrap();
        }
        g.record(frame(HYBRID)).unwrap();
        let partial = HYBRID
            .iter()
            .enumerate()
            .filter_map(|(i, p)| (i != missing).then_some(*p));
        assert!(g.record(Frame::from_names(partial)).is_err());
        assert_eq!(g.report()["rt_present"], "1/2 frames");
        assert!(validate(&g.report(), 2).is_err());
        assert!(g.record(frame(HYBRID)).is_err());
    }
}
#[test]
fn full_transport_and_reservoirs_are_distinct_stable_workloads() {
    let mut g = Guard::new(Expected::Fidelity, 2, Duration::from_secs(1));
    assert!(!g.wait(Duration::ZERO, Some(frame(TRACE))).unwrap());
    assert!(!g.wait(Duration::ZERO, Some(frame(RESERVOIRS))).unwrap());
    assert!(g.wait(Duration::ZERO, Some(frame(RESERVOIRS))).unwrap());
    g.record(frame(RESERVOIRS)).unwrap();
    assert!(g.record(frame(TRACE)).is_err());
}
#[test]
fn expectation_comes_from_preset_capability_and_requested_mode() {
    assert_eq!(Expected::resolve("high", true, false), Expected::Hybrid);
    assert_eq!(Expected::resolve("epic", true, false), Expected::Hybrid);
    assert_eq!(Expected::resolve("medium", true, false), Expected::Raster);
    assert_eq!(Expected::resolve("low", true, true), Expected::Raster);
    assert_eq!(Expected::resolve("high", false, false), Expected::Raster);
    assert_eq!(Expected::resolve("high", false, true), Expected::Fidelity);
    let mut raster = Guard::new(Expected::Raster, 3, Duration::from_secs(1));
    assert!(raster.wait(Duration::ZERO, Some(frame(&["SSR"]))).unwrap());
    raster.record(frame(&["SSR"])).unwrap();
    assert!(validate(&raster.report(), 1).is_ok());
    assert_eq!(raster.report()["rt_present"], "0/1 frames");
    assert!(raster.record(frame(&[])).is_err());
}
#[test]
fn old_or_truncated_evidence_cannot_be_accepted() {
    assert!(validate(&Value::Null, 240).is_err());
    let mut g = guard();
    for _ in 0..3 {
        g.wait(Duration::ZERO, Some(frame(HYBRID))).unwrap();
    }
    for _ in 0..240 {
        g.record(frame(HYBRID)).unwrap();
    }
    assert!(validate(&g.report(), 240).is_ok());
    let mut partial = g.report();
    partial["rt_present_frames"] = json!(239);
    assert!(validate(&partial, 240).is_err());
}
