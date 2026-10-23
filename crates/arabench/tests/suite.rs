//! Every v0 verifier rejects its untouched fixture and accepts its reference
//! solution (the same check as `arabench selftest`, without agents).

use arabench::fixture::Init;
use arabench::runner::self_test;
use arabench::suite;

#[test]
fn every_verifier_rejects_the_fixture_and_accepts_the_reference_solution() {
    let tasks = suite::v0();
    assert!(tasks.len() >= 20, "v0 has {} tasks", tasks.len());
    let failures: Vec<String> = tasks
        .iter()
        .map(|t| self_test(t, Init::Minimal))
        .filter(|st| !st.ok())
        .map(|st| format!("{}: {:?} / after {:?}", st.task, st.error, st.after))
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn task_ids_are_unique() {
    let mut ids: Vec<&str> = suite::v0().iter().map(|t| t.id).collect();
    ids.sort_unstable();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n);
}
