//! Exact abort records never erase dispatched or legacy incomplete actions.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new(records: &[Value]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = crate::test_target()
            .join("monkey-journal-reader")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("events.jsonl");
        let text = records
            .iter()
            .map(|record| serde_json::to_string(record).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, text).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}
fn begin(step: u64, ticket: u64) -> Value {
    serde_json::json!({"phase":"begin","seed":7,"hostSeed":7,"step":step,"ticket":ticket,
        "action":{"kind":"tab","reverse":step==1}})
}
fn abort(step: u64, ticket: u64) -> Value {
    serde_json::json!({"phase":"abort","step":step,"ticket":ticket,"wasDispatched":false})
}

#[test]
fn exact_uncompleted_abort_is_excluded_but_same_step_can_later_really_run() {
    let fixture = Fixture::new(&[
        begin(0, 1),
        serde_json::json!({"phase":"end","step":0,"outcome":"rejected: real failure"}),
        begin(1, 2),
        abort(1, 2),
        begin(1, 3),
        serde_json::json!({"phase":"end","step":1,"outcome":"accepted"}),
    ]);
    let replay = read(&fixture.0).unwrap();
    assert_eq!(replay.actions.len(), 2);
    assert_eq!(replay.original_host_seed, Some(7));
    assert!(matches!(
        &replay.actions[0].action,
        super::super::Action::Tab { reverse: false }
    ));
    assert!(matches!(
        &replay.actions[1].action,
        super::super::Action::Tab { reverse: true }
    ));
}

#[test]
fn a_wrong_ticket_or_step_or_an_abort_after_end_is_rejected_without_erasing_a_real_action() {
    for records in [
        vec![begin(0, 1), abort(0, 2)],
        vec![begin(0, 1), abort(1, 1)],
        vec![
            begin(0, 1),
            serde_json::json!({"phase":"end","step":0,"outcome":"rejected"}),
            abort(0, 1),
        ],
    ] {
        let fixture = Fixture::new(&records);
        let error = read(&fixture.0).err().unwrap().to_string();
        assert!(error.contains("abort must match"));
    }
}

#[test]
fn legacy_begins_with_no_ticket_or_end_keep_their_original_replay_semantics() {
    let mut first = begin(0, 1);
    let mut second = begin(1, 2);
    for begin in [&mut first, &mut second] {
        begin.as_object_mut().unwrap().remove("ticket");
        begin.as_object_mut().unwrap().remove("hostSeed");
    }
    let fixture = Fixture::new(&[first, second]);
    let replay = read(&fixture.0).unwrap();
    assert_eq!(replay.actions.len(), 2);
    assert_eq!(replay.original_host_seed, None);
}
