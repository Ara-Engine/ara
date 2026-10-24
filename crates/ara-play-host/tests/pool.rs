#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
//! A player that exits before its hello (a stale build rejecting an argument) must not
//! stall the host tick: launches run off the caller's thread and refills back off.
use ara_play_host::pool::{Pool, PoolConfig};
use std::time::{Duration, Instant};

#[test]
fn players_that_exit_before_hello_back_off_without_blocking_the_tick() {
    let logs = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-play-host/pool-backoff")
        .join(std::process::id().to_string());
    // This test binary is the "player": it rejects `--owner` and exits at once.
    let config = PoolConfig {
        executable: std::env::current_exe().unwrap(),
        logs: logs.clone(),
        gpu_cache: logs.join("gpu"),
        headless: 1,
        windowed: 0,
        size: [64, 64],
    };
    let mut pool = Pool::new(config, "token".into()).unwrap();
    let (mut failures, mut worst) = (0, Duration::ZERO);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        let tick = Instant::now();
        failures += pool.poll().len();
        pool.refill();
        worst = worst.max(tick.elapsed());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(failures >= 1, "the player never failed");
    // Without the backoff (0.5 s, 1 s, 2 s…) every tick would relaunch: dozens.
    assert!(failures <= 4, "{failures} launches failed in 2 s");
    assert!(worst < Duration::from_millis(16), "a tick took {worst:?}");
    drop(pool);
    let _ = std::fs::remove_dir_all(&logs);
}
