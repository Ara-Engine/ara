use super::*;

fn extension(ids: &[&str], generation: &str) -> Catalog {
    Catalog {
        commands: ids
            .iter()
            .map(|id| crate::extensions::Command {
                id: (*id).into(),
                title: (*id).into(),
                summary: "Coverage fixture".into(),
                enabled: false,
                disabled_reason: Some("Fixture callback is disabled".into()),
            })
            .collect(),
        generation: Some(generation.into()),
        ..Default::default()
    }
}
fn response(catalog: &Catalog) -> Value {
    json!({"commands":commands::entries(catalog).map(|c| json!({"id":c.id})).collect::<Vec<_>>(),
        "generation":catalog.generation})
}
fn offer_pending(state: &mut State) -> Vec<String> {
    let mut offered = Vec::new();
    while let Some(Action::Command { id, target }) = state.next() {
        state.record(&id, actions::edge_case(&target).unwrap());
        offered.push(id);
    }
    offered
}

#[test]
fn late_registrations_are_sorted_and_new_generations_require_their_own_cases() {
    let mut state = State::default();
    let native = Catalog::default();
    state.observe(&native);
    offer_pending(&mut state);
    state.reconcile_final(&response(&native), false);
    assert!(
        !state.complete(),
        "Jobs must settle before final coverage is accepted"
    );
    state.reconcile_final(&response(&native), true);
    assert!(state.complete());

    let loaded = extension(&["tools.zeta", "tools.alpha"], "first");
    state.observe(&loaded);
    assert!(!state.complete());
    assert!(state.drain_pending());
    let expected: Vec<_> = ["tools.alpha", "tools.zeta"]
        .into_iter()
        .flat_map(|id| std::iter::repeat_n(id.to_owned(), actions::edge_case_count()))
        .collect();
    assert_eq!(offer_pending(&mut state), expected);
    state.reconcile_final(&response(&loaded), true);
    assert!(state.complete());

    let changed = extension(&["tools.alpha"], "second");
    state.observe(&changed);
    assert!(
        !state.complete(),
        "The old implementation cannot cover a newly loaded generation"
    );
    assert_eq!(state.missing().len(), 1);
    assert_eq!(state.missing()[0]["generation"], "second");
    assert_eq!(
        offer_pending(&mut state),
        vec!["tools.alpha"; actions::edge_case_count()]
    );
    state.reconcile_final(&response(&changed), true);
    assert!(state.complete());
    assert_eq!(
        state.report()["shellCommandCatalogChanges"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn removed_queued_implementations_and_unloaded_final_ids_cannot_receive_credit() {
    let mut state = State::default();
    state.observe(&Catalog::default());
    offer_pending(&mut state);
    let late = extension(&["tools.vanished"], "first");
    state.observe(&late);
    state.observe(&Catalog::default());
    let offered = offer_pending(&mut state);
    assert_eq!(offered.len(), actions::edge_case_count());
    state.reconcile_final(&response(&Catalog::default()), true);
    assert!(state.final_matched());
    assert!(!state.drain_pending());
    assert!(!state.complete());
    assert_eq!(state.missing()[0]["command"], "tools.vanished");
    assert_eq!(
        state.missing()[0]["cases"].as_array().unwrap().len(),
        actions::edge_case_count()
    );
    assert_eq!(
        state.report()["shellRegistrationProbesSkipped"],
        actions::edge_case_count()
    );

    state.reconcile_final(&response(&late), true);
    assert!(!state.final_matched());
    assert!(state.drain_pending());
    assert_eq!(
        state.report()["shellCommandCatalogFinalUnloaded"],
        json!(["tools.vanished"])
    );
}

#[test]
fn malformed_final_catalogs_fail_closed() {
    for response in [
        json!({}),
        json!({"commands":[{"id":""}]}),
        json!({"commands":[{"id":"edit.undo"},{"id":"edit.undo"}]}),
        json!({"commands":[],"generation":42}),
    ] {
        let mut state = State::default();
        state.observe(&Catalog::default());
        offer_pending(&mut state);
        state.reconcile_final(&response, true);
        assert!(!state.complete());
        assert!(
            !state.report()["shellCommandCatalogErrors"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn teardown_defers_unfinished_ordinary_work_but_covers_genuinely_late_registrations() {
    let mut incomplete = State::default();
    incomplete.observe(&Catalog::default());
    incomplete.observe(&extension(&["tools.early"], "early"));
    incomplete.begin_drain();
    assert!(
        incomplete.next().is_none(),
        "Teardown must not launch an initial or early command sweep"
    );
    assert!(!incomplete.missing().is_empty());
    incomplete.observe(&extension(&["tools.late_without_baseline"], "later"));
    assert!(
        incomplete.next().is_none(),
        "Incomplete baseline work cannot become drain-eligible through reload"
    );
    assert!(!incomplete.complete());
    assert!(
        incomplete.report()["shellDrainDeferredProbes"]
            .as_u64()
            .unwrap()
            > 0
    );

    let mut completed = State::default();
    completed.observe(&Catalog::default());
    offer_pending(&mut completed);
    let late = extension(&["tools.after_sweep"], "late");
    completed.observe(&late);
    completed.begin_drain();
    assert_eq!(
        offer_pending(&mut completed),
        vec!["tools.after_sweep"; actions::edge_case_count()]
    );
    completed.reconcile_final(&response(&late), true);
    assert!(completed.complete());
    assert_eq!(completed.report()["shellDrainDeferredProbes"], 0);
}
