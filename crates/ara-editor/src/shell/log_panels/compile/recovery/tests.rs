//! Real Store sequence/dedup replay; Status values never enter this projection.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_diag::{
    Diagnostic, Severity, Subject,
    bus::Meta,
    store::{Filter, Retention, Store},
};

#[derive(Clone, Copy)]
struct Scope {
    world: &'static str,
    player: u32,
    source: &'static str,
    binding: u64,
}
const LIVE: Scope = Scope {
    world: "play:one",
    player: 0,
    source: "opaque-player",
    binding: 1,
};
impl Scope {
    fn value(self) -> Value {
        json!({"world":self.world,"player":self.player,"source":self.source,"binding":self.binding})
    }
    fn fault(self, module: &str, code: u16, message: &str) -> Diagnostic {
        Diagnostic::error(ara_diag::Code(code), message)
            .subject(Subject::module(module))
            .subject(Subject::system(format!(
                "play/{}/{}/{}/{}",
                self.world, self.player, self.binding, self.source
            )))
            .data(json!({"module":module,"play":self.value()}))
    }
    fn marker(self, ticket: u64, outcome: &str, modules: &[&str]) -> Diagnostic {
        Diagnostic::info(ara_diag::Code(935), "C# recovery finished")
            .subject(Subject::system(format!("script-recovery/{ticket}")))
            .data(json!({"play":self.value(),"recovery":{"schema":1,"ticket":ticket,"outcome":outcome,"modules":modules}}))
    }
}
fn store() -> Store {
    Store::new(Retention::default())
}
fn emit(store: &mut Store, diagnostic: Diagnostic) {
    store.push(diagnostic, Meta::default());
}
fn poll(view: &mut CompilationView, store: &Store) {
    let entries = store.query(&Filter {
        since: view.cursor,
        ..Default::default()
    });
    view.ingest(&json!({"cursor":store.cursor(),"total":entries.len(),"entries":&entries[entries.len().saturating_sub(PAGE)..]}));
}
fn active(view: &CompilationView, store: &Store) -> Vec<String> {
    let fresh = store.query(&Filter {
        severity: Some(Severity::Warning),
        limit: PAGE,
        ..Default::default()
    });
    fresh
        .iter()
        .filter_map(|row| entry(&json!(row)))
        .filter(|row| view.fallback(row))
        .map(|row| row.diagnostic.message)
        .chain(view.entries().map(|row| row.diagnostic.message.clone()))
        .collect()
}

#[test]
fn reloaded_retires_only_older_runtime_faults_in_exact_incarnation_and_module_and_keeps_console() {
    let mut store = store();
    let mut view = CompilationView::default();
    for code in [910, 911, 912] {
        emit(
            &mut store,
            LIVE.fault("Gameplay", code, &format!("fault-{code}")),
        );
    }
    emit(&mut store, LIVE.fault("Other", 910, "other module"));
    emit(
        &mut store,
        Scope {
            world: "play:two",
            ..LIVE
        }
        .fault("Gameplay", 910, "other world"),
    );
    emit(
        &mut store,
        Scope { player: 1, ..LIVE }.fault("Gameplay", 910, "other player"),
    );
    emit(
        &mut store,
        Scope {
            source: "replaced-source",
            ..LIVE
        }
        .fault("Gameplay", 910, "other source"),
    );
    emit(
        &mut store,
        Scope { binding: 2, ..LIVE }.fault("Gameplay", 910, "other binding"),
    );
    emit(&mut store, LIVE.fault("Gameplay", 900, "compile error"));
    poll(&mut view, &store);
    emit(&mut store, LIVE.marker(1, "reloaded", &["Gameplay"]));
    poll(&mut view, &store);
    let active = active(&view, &store);
    for code in [910, 911, 912] {
        assert!(!active.contains(&format!("fault-{code}")));
    }
    for message in [
        "other module",
        "other world",
        "other player",
        "other source",
        "other binding",
        "compile error",
    ] {
        assert!(
            active.contains(&message.into()),
            "{message} must remain visible"
        );
    }
    assert_eq!(
        store.query(&Filter::default()).len(),
        10,
        "Console retains every original row"
    );
    assert_eq!(view.report()["recovery"]["moduleRetirementRecords"], 1);
}

#[test]
fn unchanged_failed_refused_and_compile_success_never_retire_runtime_faults() {
    let mut store = store();
    let mut view = CompilationView::default();
    emit(&mut store, LIVE.fault("Gameplay", 910, "still faulted"));
    poll(&mut view, &store);
    for (ticket, outcome) in [(1, "unchanged"), (2, "failed"), (3, "refused")] {
        emit(&mut store, LIVE.marker(ticket, outcome, &["Gameplay"]));
        poll(&mut view, &store);
        assert!(active(&view, &store).contains(&"still faulted".into()));
    }
    let scope = metadata::Key {
        producer: "warm".into(),
        module: "Gameplay".into(),
        profile: "gameplay".into(),
        world: Some(LIVE.world.into()),
        player: Some(0),
    };
    view.retire_errors(
        &scope,
        metadata::Order {
            play_binding: 1,
            compile_binding: 4,
            attempt: 100,
        },
    );
    assert!(
        active(&view, &store).contains(&"still faulted".into()),
        "compile retirement does not own runtime faults"
    );
}

#[test]
fn repeated_fault_after_reload_uses_latest_store_sequence_and_becomes_active_again() {
    let mut store = store();
    let mut view = CompilationView::default();
    let fault = LIVE.fault("Gameplay", 912, "repeat command fault");
    emit(&mut store, fault.clone());
    poll(&mut view, &store);
    emit(&mut store, LIVE.marker(1, "reloaded", &["Gameplay"]));
    poll(&mut view, &store);
    assert!(active(&view, &store).is_empty());
    emit(&mut store, fault);
    poll(&mut view, &store);
    assert_eq!(active(&view, &store), ["repeat command fault"]);
    let rows = store.query(&Filter::default());
    let row = rows
        .iter()
        .find(|row| row.diagnostic.code == ara_diag::Code(912))
        .unwrap();
    assert_eq!(row.first, 1);
    assert_eq!(row.count, 2);
    assert!(row.seq > 2);
}

#[test]
fn incomplete_page_marker_does_not_clear_but_later_clean_exact_marker_can() {
    let mut store = store();
    let mut view = CompilationView::default();
    emit(
        &mut store,
        LIVE.fault("Gameplay", 911, "retained across gap"),
    );
    poll(&mut view, &store);
    for index in 0..600 {
        emit(
            &mut store,
            Diagnostic::info(
                ara_diag::codes::INVALID_PARAMS,
                format!("unrelated {index}"),
            ),
        );
    }
    emit(&mut store, LIVE.marker(1, "reloaded", &["Gameplay"]));
    poll(&mut view, &store);
    assert!(active(&view, &store).contains(&"retained across gap".into()));
    assert_eq!(view.report()["recovery"]["gapMarkers"], 1);
    assert!(
        view.notice()
            .unwrap()
            .contains("incomplete diagnostic pages")
    );
    emit(&mut store, LIVE.marker(2, "reloaded", &["Gameplay"]));
    poll(&mut view, &store);
    assert!(!active(&view, &store).contains(&"retained across gap".into()));
    assert!(
        store
            .query(&Filter::default())
            .iter()
            .any(|row| row.diagnostic.message == "retained across gap")
    );
}

#[test]
fn malformed_or_unscoped_markers_and_unowned_fault_families_cannot_clear_problems() {
    let mut store = store();
    let mut view = CompilationView::default();
    emit(&mut store, LIVE.fault("Gameplay", 910, "scoped fault"));
    emit(
        &mut store,
        Diagnostic::error(ara_diag::Code(910), "legacy fault").data(json!({"module":"Gameplay"})),
    );
    emit(&mut store, LIVE.fault("Gameplay", 913, "future fault"));
    emit(
        &mut store,
        Diagnostic::error(ara_diag::Code(910), "missing module").data(json!({"play":LIVE.value()})),
    );
    emit(
        &mut store,
        Diagnostic::warning(ara_diag::Code(910), "warning contribution")
            .data(json!({"module":"Gameplay","play":LIVE.value()})),
    );
    poll(&mut view, &store);
    for index in 0..4 {
        let mut marker = LIVE.marker(index + 1, "reloaded", &["Gameplay"]);
        match index {
            0 => marker.data.as_mut().unwrap()["recovery"]["schema"] = json!(2),
            1 => marker.data.as_mut().unwrap()["recovery"]["modules"] = json!(["Gameplay", null]),
            2 => {
                marker
                    .data
                    .as_mut()
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove("play");
            }
            _ => marker.data.as_mut().unwrap()["play"]["binding"] = json!(0),
        }
        emit(&mut store, marker);
        poll(&mut view, &store);
        assert!(active(&view, &store).contains(&"scoped fault".into()));
    }
    emit(&mut store, LIVE.marker(10, "reloaded", &["Gameplay"]));
    poll(&mut view, &store);
    let active = active(&view, &store);
    assert!(!active.contains(&"scoped fault".into()));
    for message in [
        "legacy fault",
        "future fault",
        "missing module",
        "warning contribution",
    ] {
        assert!(active.contains(&message.into()));
    }
    assert_eq!(view.report()["recovery"]["invalidMarkers"], 4);
}

#[test]
fn retired_faults_stay_out_of_tail_fallback_without_clearing_console_records() {
    let mut store = store();
    let mut view = CompilationView::default();
    emit(
        &mut store,
        LIVE.fault("Gameplay", 910, "historical runtime fault"),
    );
    poll(&mut view, &store);
    let old = entry(&json!(store.query(&Filter::default())[0])).unwrap();
    emit(&mut store, LIVE.marker(1, "reloaded", &["Gameplay"]));
    poll(&mut view, &store);
    assert!(
        !view.fallback(&old),
        "cached retirement must also own ordinary bus-tail fallback"
    );
    assert!(
        store.get(old.seq).is_some(),
        "Console still owns the record"
    );
}

#[test]
fn bounded_ledger_rejects_extra_modules_without_falsely_retiring_them() {
    let mut recovery = Recovery::default();
    let modules = (0..MAX_MODULES)
        .map(|index| format!("Module{index}"))
        .collect::<Vec<_>>();
    let refs = modules.iter().map(String::as_str).collect::<Vec<_>>();
    let as_entry = |seq, diagnostic| {
        entry(&json!({"seq":seq,"first":seq,"count":1,"diagnostic":diagnostic})).unwrap()
    };
    assert!(recovery.observe(&as_entry(2, LIVE.marker(1, "reloaded", &refs)), false));
    assert_eq!(recovery.modules, MAX_MODULES);
    assert!(recovery.observe(
        &as_entry(4, LIVE.marker(2, "reloaded", &["Overflow"])),
        false
    ));
    assert_eq!(recovery.modules, MAX_MODULES);
    assert_eq!(recovery.omitted, 1);
    assert!(!recovery.retired(&as_entry(
        1,
        LIVE.fault("Overflow", 910, "cannot evict this fault")
    )));
    assert!(recovery.retired(&as_entry(
        1,
        LIVE.fault("Module0", 910, "known covered fault")
    )));
}

#[test]
fn bounded_incarnation_ledger_keeps_new_unknown_incarnation_faults_visible() {
    let mut recovery = Recovery::default();
    let as_entry = |seq, diagnostic| {
        entry(&json!({"seq":seq,"first":seq,"count":1,"diagnostic":diagnostic})).unwrap()
    };
    for binding in 1..=MAX_INCARNATIONS as u64 {
        let scope = Scope { binding, ..LIVE };
        recovery.observe(
            &as_entry(
                binding * 2,
                scope.marker(binding, "reloaded", &["Gameplay"]),
            ),
            false,
        );
    }
    let extra = Scope {
        binding: MAX_INCARNATIONS as u64 + 1,
        ..LIVE
    };
    recovery.observe(
        &as_entry(1000, extra.marker(1000, "reloaded", &["Gameplay"])),
        false,
    );
    assert_eq!(recovery.incarnations, MAX_INCARNATIONS);
    assert_eq!(recovery.omitted, 1);
    assert!(!recovery.retired(&as_entry(
        1,
        extra.fault("Gameplay", 910, "new incarnation must remain faulted")
    )));
    assert!(recovery.retired(&as_entry(
        1,
        LIVE.fault("Gameplay", 910, "known retired incarnation")
    )));
}
