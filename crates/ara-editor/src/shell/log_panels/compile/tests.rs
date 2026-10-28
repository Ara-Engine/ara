//! Replay real diagnostic Store dedup/retention through the same bounded logs page seam.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_diag::{
    Diagnostic, Severity, Subject,
    bus::Meta,
    store::{Filter, Retention, Store},
};
#[path = "tests/poll.rs"]
mod poll;

#[derive(Clone, Copy)]
struct Emitter {
    module: &'static str,
    producer: &'static str,
    instance: &'static str,
    binding: u64,
    world: Option<(&'static str, u32, &'static str, u64)>,
}
impl Default for Emitter {
    fn default() -> Self {
        Self {
            module: "Gameplay",
            producer: "warm",
            instance: "opaque-a",
            binding: 1,
            world: None,
        }
    }
}
impl Emitter {
    fn row(
        self,
        attempt: u64,
        phase: &str,
        code: u16,
        severity: Severity,
        message: &str,
    ) -> Diagnostic {
        let mut data = json!({"module":self.module,"compile":{"schema":1,"producer":self.producer,"instance":self.instance,"binding":self.binding,
            "attempt":attempt,"profile":"gameplay","phase":phase,"channel":"compiler"}});
        let mut diagnostic = Diagnostic::new(severity, ara_diag::Code(code), message)
            .file("scripts/Gameplay.cs")
            .subject(Subject::module(self.module))
            .subject(Subject::system(format!(
                "csharp.compile/{}/{attempt}",
                self.instance
            )));
        if let Some((world, player, source, binding)) = self.world {
            data["play"] = json!({"world":world,"player":player,"source":source,"binding":binding});
            let hex = |text: &str| {
                text.as_bytes()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            };
            diagnostic = diagnostic.subject(Subject::system(format!(
                "csharp.scope/play/{}/{player}/{}/{binding}",
                hex(world),
                hex(source)
            )));
        }
        diagnostic.data(data)
    }
    fn marker(self, attempt: u64, phase: &str) -> Diagnostic {
        self.row(attempt, phase, 906, Severity::Info, phase)
    }
    fn done(
        self,
        attempt: u64,
        result: &str,
        errors: u64,
        total: u64,
        emitted: u64,
        verified: bool,
    ) -> Diagnostic {
        let mut diagnostic = self.marker(attempt, "completed");
        let data = diagnostic.data.as_mut().unwrap();
        let compile = data["compile"].as_object_mut().unwrap();
        for (key, value) in [
            ("result", json!(result)),
            ("errorCount", json!(errors)),
            ("total", json!(total)),
            ("emitted", json!(emitted)),
            ("truncated", json!(emitted < total)),
            ("coverage", json!({"errors":"module","warnings":"partial"})),
        ] {
            compile.insert(key.into(), value);
        }
        if verified {
            compile.insert("validation".into(), json!("current-module-inputs"));
        }
        diagnostic
    }
}
fn push(store: &mut Store, diagnostic: Diagnostic) {
    store.push(diagnostic, Meta::default());
}
fn poll(view: &mut CompilationView, store: &Store) {
    let all = store.query(&Filter {
        since: view.cursor,
        ..Default::default()
    });
    let newest = &all[all.len().saturating_sub(PAGE)..];
    view.ingest(&json!({"cursor":store.cursor(),"total":all.len(),"entries":newest}));
}
fn store() -> Store {
    Store::new(Retention::default())
}
fn errors(view: &CompilationView) -> usize {
    view.entries()
        .filter(|entry| entry.diagnostic.is_error())
        .count()
}
fn bad(view: &mut CompilationView, store: &mut Store, emitter: Emitter, attempt: u64) {
    push(store, emitter.marker(0, "bound"));
    push(store, emitter.marker(attempt, "started"));
    push(
        store,
        emitter.row(attempt, "diagnostic", 900, Severity::Error, "broken source"),
    );
    push(store, emitter.done(attempt, "errors", 1, 1, 1, true));
    poll(view, store);
}

#[test]
fn verified_success_retires_only_matching_compile_errors_and_console_history_stays() {
    let mut store = store();
    let mut view = CompilationView::default();
    let emitter = Emitter::default();
    bad(&mut view, &mut store, emitter, 1);
    bad(
        &mut view,
        &mut store,
        Emitter {
            module: "Other",
            ..emitter
        },
        1,
    );
    push(
        &mut store,
        Diagnostic::error(ara_diag::Code(910), "runtime fault").data(json!({"module":"Gameplay"})),
    );
    push(&mut store, emitter.marker(2, "started"));
    push(
        &mut store,
        emitter.row(2, "diagnostic", 901, Severity::Warning, "partial warning"),
    );
    push(&mut store, emitter.done(2, "success", 0, 1, 1, true));
    poll(&mut view, &store);
    assert_eq!(
        errors(&view),
        2,
        "unrelated module and protected runtime fault keep their errors"
    );
    assert!(
        view.entries()
            .any(|entry| entry.diagnostic.message == "partial warning")
    );
    assert!(
        view.entries()
            .any(|entry| entry.diagnostic.message == "runtime fault")
    );
    assert_eq!(
        store
            .query(&Filter::default())
            .iter()
            .filter(|record| record.diagnostic.message == "broken source")
            .count(),
        2,
        "projection never clears Console"
    );
    assert_eq!(view.report()["scopes"][0]["phase"], "success");
}

#[test]
fn pending_cancelled_unverified_invalid_and_truncated_attempts_never_clear_old_errors() {
    let mut store = store();
    let mut view = CompilationView::default();
    let emitter = Emitter::default();
    bad(&mut view, &mut store, emitter, 1);
    for (attempt, result, verified, total, emitted) in [
        (2, "cancelled", false, 0, 0),
        (3, "success", false, 0, 0),
        (4, "errors", true, 2, 1),
    ] {
        push(&mut store, emitter.marker(attempt, "started"));
        poll(&mut view, &store);
        assert!(errors(&view) >= 1);
        assert!(view.notice().is_some());
        if emitted > 0 {
            push(
                &mut store,
                emitter.row(
                    attempt,
                    "diagnostic",
                    900,
                    Severity::Error,
                    "new partial error",
                ),
            );
        }
        push(
            &mut store,
            emitter.done(
                attempt,
                result,
                if result == "errors" { 2 } else { 0 },
                total,
                emitted,
                verified,
            ),
        );
        poll(&mut view, &store);
        assert!(errors(&view) >= 1);
        assert!(view.notice().is_some());
    }
    push(&mut store, emitter.marker(5, "started"));
    let mut invalid = emitter.done(5, "success", 0, 0, 0, true);
    invalid.data.as_mut().unwrap()["compile"]["coverage"]["errors"] = json!("file");
    push(&mut store, invalid);
    poll(&mut view, &store);
    assert!(errors(&view) >= 1);
    assert!(view.notice().is_some());
    push(&mut store, emitter.marker(6, "started"));
    push(&mut store, emitter.done(6, "success", 0, 0, 0, true));
    poll(&mut view, &store);
    assert_eq!(errors(&view), 0);
    assert!(
        view.notice().is_none(),
        "later complete attempt recovers its own coverage"
    );
}

#[test]
fn lower_attempts_bindings_and_other_play_worlds_cannot_replace_current_coverage() {
    let mut store = store();
    let mut view = CompilationView::default();
    let first = Emitter {
        world: Some(("play:one", 0, "source-a", 1)),
        ..Default::default()
    };
    let other = Emitter {
        world: Some(("play:two", 0, "source-b", 1)),
        ..first
    };
    bad(&mut view, &mut store, first, 10);
    bad(&mut view, &mut store, other, 10);
    let replacement = Emitter {
        binding: 1,
        instance: "opaque-b",
        world: Some(("play:one", 0, "source-new", 2)),
        ..first
    };
    push(&mut store, replacement.marker(0, "bound"));
    push(&mut store, replacement.marker(1, "started"));
    push(&mut store, replacement.done(1, "success", 0, 0, 0, true));
    poll(&mut view, &store);
    assert_eq!(errors(&view), 1);
    push(
        &mut store,
        first.row(99, "diagnostic", 900, Severity::Error, "late old source"),
    );
    push(&mut store, first.done(99, "errors", 1, 1, 1, true));
    poll(&mut view, &store);
    assert_eq!(
        errors(&view),
        1,
        "late lower Play binding is historical only"
    );
    push(&mut store, replacement.marker(2, "started"));
    push(&mut store, replacement.done(2, "success", 0, 0, 0, true));
    poll(&mut view, &store);
    push(
        &mut store,
        replacement.row(1, "diagnostic", 900, Severity::Error, "late old attempt"),
    );
    poll(&mut view, &store);
    assert_eq!(errors(&view), 1);
}

#[test]
fn replacement_items_require_their_bound_marker_and_keep_last_good_errors() {
    let mut store = store();
    let mut view = CompilationView::default();
    let emitter = Emitter::default();
    bad(&mut view, &mut store, emitter, 1);
    let next = Emitter {
        binding: 2,
        instance: "opaque-next",
        ..emitter
    };
    push(&mut store, next.marker(1, "started"));
    push(&mut store, next.done(1, "success", 0, 0, 0, true));
    poll(&mut view, &store);
    assert_eq!(errors(&view), 1);
    assert!(view.notice().is_some());
    push(&mut store, next.marker(0, "bound"));
    push(&mut store, next.marker(2, "started"));
    push(&mut store, next.done(2, "success", 0, 0, 0, true));
    poll(&mut view, &store);
    assert_eq!(errors(&view), 0);
    assert!(view.notice().is_none());
}

#[test]
fn complete_error_batch_replaces_only_prior_errors_and_unknown_tagged_families_stay_visible() {
    let mut store = store();
    let mut view = CompilationView::default();
    let emitter = Emitter::default();
    bad(&mut view, &mut store, emitter, 1);
    push(&mut store, emitter.marker(2, "started"));
    push(
        &mut store,
        emitter.row(
            2,
            "diagnostic",
            900,
            Severity::Error,
            "different current error",
        ),
    );
    push(
        &mut store,
        emitter.row(
            2,
            "diagnostic",
            901,
            Severity::Warning,
            "partial warning retained",
        ),
    );
    push(&mut store, emitter.done(2, "errors", 1, 2, 2, true));
    poll(&mut view, &store);
    assert_eq!(errors(&view), 1);
    assert!(
        !view
            .entries()
            .any(|entry| entry.diagnostic.message == "broken source")
    );
    let current_seq = view
        .entries()
        .find(|entry| entry.diagnostic.message == "different current error")
        .unwrap()
        .seq;
    assert_eq!(
        view.report()["scopes"][0]["activeErrorSeqs"],
        json!([current_seq])
    );
    assert_eq!(
        view.report()["scopes"][0]["lastVerifiedCompletion"]["result"],
        "errors"
    );
    push(&mut store, emitter.marker(3, "started"));
    push(
        &mut store,
        emitter.row(
            3,
            "diagnostic",
            1300,
            Severity::Error,
            "future compiler family",
        ),
    );
    push(&mut store, emitter.done(3, "errors", 1, 1, 1, true));
    poll(&mut view, &store);
    push(&mut store, emitter.marker(4, "started"));
    push(&mut store, emitter.done(4, "success", 0, 0, 0, true));
    poll(&mut view, &store);
    assert!(
        view.entries()
            .any(|entry| entry.diagnostic.message == "future compiler family")
    );
    assert!(
        view.entries()
            .any(|entry| entry.diagnostic.message == "partial warning retained")
    );
    assert!(view.notice().is_some());
}

#[test]
fn ordinary_repeats_are_accounted_but_over_500_and_retention_gaps_remain_honest() {
    let mut store = store();
    let mut view = CompilationView::default();
    let emitter = Emitter::default();
    bad(&mut view, &mut store, emitter, 1);
    for _ in 0..40 {
        push(
            &mut store,
            Diagnostic::info(ara_diag::codes::INVALID_PARAMS, "ordinary repeated info"),
        );
    }
    push(&mut store, emitter.marker(2, "started"));
    push(&mut store, emitter.done(2, "success", 0, 0, 0, true));
    poll(&mut view, &store);
    assert_eq!(view.possible_gaps, 0);
    assert_eq!(errors(&view), 0);
    bad(&mut view, &mut store, emitter, 3);
    push(&mut store, emitter.marker(4, "started"));
    for index in 0..600 {
        push(
            &mut store,
            emitter.row(
                4,
                "diagnostic",
                900,
                Severity::Error,
                &format!("error {index}"),
            ),
        );
    }
    push(&mut store, emitter.done(4, "errors", 600, 600, 600, true));
    poll(&mut view, &store);
    assert!(view.possible_gaps > 0);
    assert!(
        view.entries()
            .any(|entry| entry.diagnostic.message == "broken source")
    );
    assert!(view.notice().is_some());
    push(&mut store, emitter.marker(5, "started"));
    push(&mut store, emitter.done(5, "success", 0, 0, 0, true));
    poll(&mut view, &store);
    assert_eq!(errors(&view), 0);
    assert!(view.notice().is_none());
    let mut retained = Store::new(Retention {
        errors: 1,
        warnings: 1,
        info: 1,
        max_age: None,
    });
    let mut limited = CompilationView::default();
    push(&mut retained, emitter.marker(0, "bound"));
    poll(&mut limited, &retained);
    push(&mut retained, emitter.marker(1, "started"));
    push(
        &mut retained,
        emitter.row(1, "diagnostic", 900, Severity::Error, "evicted error"),
    );
    push(
        &mut retained,
        emitter.row(1, "diagnostic", 900, Severity::Error, "kept error"),
    );
    push(&mut retained, emitter.done(1, "errors", 2, 2, 2, true));
    poll(&mut limited, &retained);
    assert!(limited.possible_gaps > 0);
    assert!(limited.notice().is_some());
}

#[test]
fn console_collapse_keeps_context_but_groups_repeat_attempts_and_invalid_metadata_falls_back() {
    let a = Emitter {
        world: Some(("play:one", 0, "source-a", 1)),
        ..Default::default()
    };
    let as_entry = |diagnostic| {
        entry(&json!({"seq":1,"first":1,"count":1,"origin":"engine","diagnostic":diagnostic}))
            .unwrap()
    };
    let first = as_entry(a.row(1, "diagnostic", 900, Severity::Error, "same"));
    let repeat = as_entry(a.row(2, "diagnostic", 900, Severity::Error, "same"));
    assert_eq!(
        first.identity(),
        repeat.identity(),
        "attempt is deliberately absent from collapse identity"
    );
    for emitter in [
        Emitter {
            producer: "project",
            ..a
        },
        Emitter {
            world: Some(("play:two", 0, "source-a", 1)),
            ..a
        },
        Emitter {
            world: Some(("play:one", 1, "source-a", 1)),
            ..a
        },
    ] {
        assert_ne!(
            first.identity(),
            as_entry(emitter.row(1, "diagnostic", 900, Severity::Error, "same")).identity()
        );
    }
    let view = CompilationView::default();
    let invalid = as_entry(
        Diagnostic::error(ara_diag::Code(900), "legacy compiler error")
            .data(json!({"module":"Gameplay","compile":{"schema":99}})),
    );
    assert!(view.fallback(&invalid));
}

#[test]
fn scoped_runtime_fault_survives_500_record_tail_and_matching_compile_success() {
    let mut store = store();
    let mut view = CompilationView::default();
    let emitter = Emitter::default();
    bad(&mut view, &mut store, emitter, 1);
    let old_compile = view
        .entries()
        .find(|entry| entry.diagnostic.message == "broken source")
        .unwrap()
        .seq;
    let runtime=store.push(Diagnostic::error(ara_diag::Code(910),"world-owned runtime fault").data(json!({"module":"Gameplay","play":{"world":"play:outside","player":3,"source":"opaque-player","binding":1}})),Meta::default()).seq;
    poll(&mut view, &store);
    let other = Emitter {
        module: "Other",
        ..emitter
    };
    push(&mut store, other.marker(0, "bound"));
    push(&mut store, other.marker(1, "started"));
    poll(&mut view, &store);
    for index in 0..600 {
        push(
            &mut store,
            other.row(
                1,
                "diagnostic",
                900,
                Severity::Error,
                &format!("other error {index}"),
            ),
        );
    }
    push(&mut store, other.done(1, "errors", 600, 600, 600, true));
    poll(&mut view, &store);
    assert!(
        !store
            .query(&Filter {
                severity: Some(Severity::Warning),
                limit: 500,
                ..Default::default()
            })
            .iter()
            .any(|record| record.seq == runtime)
    );
    push(&mut store, emitter.marker(2, "started"));
    push(&mut store, emitter.done(2, "success", 0, 0, 0, true));
    poll(&mut view, &store);
    assert!(
        view.entries().any(|entry| entry.seq == runtime),
        "protected runtime fault remains reachable beyond the fresh tail"
    );
    assert!(!view.entries().any(|entry| entry.seq == old_compile));
    assert!(
        view.entries()
            .any(|entry| entry.diagnostic.message.starts_with("other error"))
    );
    assert_eq!(view.report()["protectedRuntimeFaultSeqs"], json!([runtime]));
    assert!(
        store.get(old_compile).is_some() && store.get(runtime).is_some(),
        "Console history was never cleared"
    );
}
