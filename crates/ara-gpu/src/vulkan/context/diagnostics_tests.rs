use super::*;

fn emit(
    state: &Validation,
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    kind: vk::DebugUtilsMessageTypeFlagsEXT,
    message: &CStr,
) {
    let data = vk::DebugUtilsMessengerCallbackDataEXT::default()
        .message_id_name(c"callback-regression")
        .message(message);
    // SAFETY: Both callback arguments and their C strings remain live for this
    // synchronous call; Validation uses atomics/a diagnostic-only mutex.
    unsafe {
        debug_callback(
            severity,
            kind,
            &data,
            std::ptr::from_ref(state).cast_mut().cast(),
        )
    };
}

#[test]
fn environment_errors_are_informational_once_across_instances() {
    let first = Validation::default();
    let second = Validation::default();
    let message = c"test-only loader manifest missing: environment once across instances";
    emit(
        &first,
        vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        vk::DebugUtilsMessageTypeFlagsEXT::GENERAL,
        message,
    );
    emit(
        &first,
        vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
        vk::DebugUtilsMessageTypeFlagsEXT::GENERAL,
        message,
    );
    emit(
        &second,
        vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        vk::DebugUtilsMessageTypeFlagsEXT::GENERAL,
        message,
    );
    let messages = first.messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "ARA_GPU_ENVIRONMENT");
    assert_eq!(messages[0].severity, "INFO");
    assert!(second.messages.lock().unwrap().is_empty());
    for state in [&first, &second] {
        assert_eq!(state.errors.load(Ordering::Relaxed), 0);
        assert_eq!(state.warnings.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn mixed_general_validation_and_performance_remain_counted_and_unsuppressed() {
    let state = Validation::default();
    let message = c"test-only validation hazard remains an error even with GENERAL";
    for _ in 0..2 {
        emit(
            &state,
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION,
            message,
        );
    }
    emit(
        &state,
        vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
        vk::DebugUtilsMessageTypeFlagsEXT::GENERAL | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        message,
    );
    assert_eq!(state.errors.load(Ordering::Relaxed), 2);
    assert_eq!(state.warnings.load(Ordering::Relaxed), 1);
    let messages = state.messages.lock().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].code, "ARA_GPU_VALIDATION");
    assert_eq!(messages[0].severity, "ERROR");
    assert_eq!(messages[2].severity, "WARNING");
}

#[test]
fn child_environment_notices_join_one_informational_console_record() {
    let mut store = ara_diag::store::Store::default();
    for child in 0..16 {
        let mut metadata = environment::metadata("test-only overlay manifest is stale");
        metadata.origin = Some(ara_diag::bus::Origin::Agent(
            format!("child-{child}").into(),
        ));
        store.push(
            ara_diag::Diagnostic::info(
                ara_diag::codes::GPU_ENVIRONMENT,
                "test-only overlay manifest is stale",
            ),
            metadata,
        );
    }
    assert_eq!(store.len(), 1);
    assert!(
        store
            .query(&ara_diag::store::Filter {
                severity: Some(ara_diag::Severity::Error),
                ..Default::default()
            })
            .is_empty()
    );
}
