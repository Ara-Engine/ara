//! A single system and registry transitions keep deferred writes and live state.
#[path = "executor/fixture.rs"]
mod fixture;
mod support;

#[test]
fn caller_dispatch_preserves_commands_and_one_two_one_system_transitions() {
    fixture::exercise(2);
}
