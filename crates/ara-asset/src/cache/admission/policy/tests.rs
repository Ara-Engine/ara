use super::*;

fn window(policy: &mut Policy, ms: u32, start: u64) {
    for i in 0..16 {
        policy.observe(1024 * 1024, ms, start + i);
    }
}

#[test]
fn slow_windows_shrink_and_healthy_windows_recover_with_hysteresis() {
    let mut policy = Policy::new(0);
    window(&mut policy, 100, 1_000);
    assert_eq!(policy.limit, 2);
    window(&mut policy, 100, 2_000);
    assert_eq!(policy.limit, 1);
    for second in 3..20 {
        window(&mut policy, 1, second * 1_000);
    }
    assert_eq!(policy.limit, MAX_WRITERS);
    window(&mut policy, 10, 21_000);
    assert_eq!(policy.limit, MAX_WRITERS, "middle band holds capacity");
}

#[test]
fn tiny_records_do_not_hide_payload_pressure_and_one_spike_does_not_dominate_p90() {
    let mut policy = Policy::new(0);
    for i in 0..100 {
        policy.observe(1024, 1, 1_000 + i);
    }
    assert_eq!(policy.count, 0);
    for i in 0..15 {
        policy.observe(1024 * 1024, 2, 2_000 + i);
    }
    policy.observe(1024 * 1024, 100, 2_015);
    assert_eq!(policy.limit, 5, "p90 is healthy despite one outlier");
}

#[test]
fn corrupt_torn_stale_and_clock_reversed_feedback_reset_safely() {
    let mut policy = Policy::new(1_000);
    window(&mut policy, 100, 2_000);
    assert_eq!(Policy::decode(&policy.encode(), 2_100).limit, 2);
    let mut bytes = policy.encode();
    bytes[8] = 0;
    for input in [&bytes[..], &bytes[..12], &[]] {
        assert_eq!(Policy::decode(input, 2_100).limit, 4);
    }
    let mut stale = Policy::decode(&policy.encode(), 30_000);
    assert_eq!(stale.limit, 2, "a live lease cannot expire its cap");
    assert!(stale.reset_if_idle(30_000));
    assert_eq!(stale.limit, 4);
    assert_eq!(Policy::decode(&policy.encode(), 1_000).limit, 4);
}
