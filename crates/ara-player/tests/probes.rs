//! Hidden-window GPU probes of the player's present path, run in a spawned player
//! (`--probe NAME`, `src/standalone/probes.rs`): SDL video must start on a process's
//! main thread on macOS, which a libtest thread is not. Run one at a time:
//! `cargo test -p ara-player --test probes -- --ignored --test-threads=1`.
use std::process::Command;

/// Runs `ara-player --probe name` and fails with its output unless it exits cleanly.
fn probe(name: &str) {
    let output = Command::new(env!("CARGO_BIN_EXE_ara-player"))
        .args(["--probe", name])
        // No Dock icon or focus change for the hidden window on macOS.
        .env("SDL_MAC_BACKGROUND_APP", "1")
        .output()
        .expect("spawn ara-player");
    assert!(
        output.status.success(),
        "probe {name} failed ({}):\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "hidden SDL GPU ownership probe; run explicitly on the GPU host"]
fn native_game_ui_retires_before_the_device_when_a_frame_returns_an_error() {
    probe("ui-error-teardown");
}

#[test]
#[ignore = "hidden SDL GPU frames-in-flight probe; run explicitly (one test thread) on the GPU host"]
fn frames_in_flight_with_game_ui_and_swapchain_stay_validation_clean() {
    probe("frames-in-flight");
}

#[test]
#[ignore = "hidden SDL GPU pump probe; run explicitly (one test thread) on the GPU host"]
fn an_unpresentable_window_never_blocks_the_event_pump() {
    probe("unpresentable-pump");
}

#[test]
fn an_unknown_probe_names_the_known_ones() {
    let output = Command::new(env!("CARGO_BIN_EXE_ara-player"))
        .args(["--probe", "nope"])
        .output()
        .expect("spawn ara-player");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unpresentable-pump"),
        "unexpected output: {stderr}"
    );
}
