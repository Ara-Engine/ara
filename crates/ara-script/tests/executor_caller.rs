//! A single system and registry transitions keep deferred writes and live state.
#[path = "executor/fixture.rs"]
mod fixture;
mod support;

#[test]
fn caller_dispatch_preserves_commands_across_chain_fork_and_single_systems() {
    fixture::exercise(2);
}
