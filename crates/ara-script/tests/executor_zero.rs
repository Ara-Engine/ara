//! Pool globals are process-wide, so the zero-worker lane has its own test binary.
#[path = "executor/fixture.rs"]
mod fixture;
mod support;

#[test]
fn zero_workers_preserve_dependent_systems_and_final_command_playback() {
    fixture::exercise(0);
}
