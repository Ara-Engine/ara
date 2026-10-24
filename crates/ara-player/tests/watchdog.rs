//! Liveness is independent of a starved/stalled simulation, but genuine silence
//! still kills the child. No GPU, assets, .NET or machine-wide CPU load is needed.
#![allow(clippy::unwrap_used)]
use ara_play_host::pool::{Pool, PoolConfig, WATCHDOG};
use serde_json::json;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[test]
fn stalled_simulation_keeps_its_heartbeat_and_only_real_silence_is_killed() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-player/watchdog")
        .join(std::process::id().to_string());
    let config = PoolConfig {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_ara-player")),
        logs: root.join("logs"),
        gpu_cache: root.join("gpu"),
        headless: 1,
        windowed: 0,
        size: [64, 64],
    };
    let mut pool = Pool::new(config, "watchdog-test".into()).unwrap();
    let mut player = pool
        .take(1, false, Duration::from_secs(30))
        .unwrap()
        .pop()
        .unwrap();
    // Longer than the actual watchdog: extending the timeout alone cannot make
    // this pass. The main thread produces neither status nor frames throughout.
    let millis = (WATCHDOG + Duration::from_millis(750)).as_millis() as u64;
    let result = player
        .call(
            ara_play::method::STALL,
            &json!({"millis":millis}),
            Duration::from_secs(20),
        )
        .expect("live heartbeat must prevent a false kill")
        .unwrap();
    assert_eq!(result["stalledMs"], millis);
    assert!(player.child.try_wait().unwrap().is_none());
    let began = Instant::now();
    let fault = player
        .call(
            ara_play::method::CRASH,
            &json!({"kind":"hang"}),
            Duration::from_secs(20),
        )
        .expect_err("a genuinely silent child must be killed");
    assert_eq!(fault["code"], "player_unresponsive", "{fault}");
    assert!(began.elapsed() >= WATCHDOG);
    println!(
        "stalled simulation survived {millis} ms; silence killed after {:?}",
        began.elapsed()
    );
}
