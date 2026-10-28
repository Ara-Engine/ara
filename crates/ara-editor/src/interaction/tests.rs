use super::*;

fn state() -> EditorState {
    EditorState {
        doc: "sc_original".into(),
        selection: vec!["e_original".into()],
        focus: "inspector".into(),
        focus_field: Some("Transform.position".into()),
        ..Default::default()
    }
}

#[test]
fn continued_typing_in_the_same_field_invalidates_only_the_navigation_owner() {
    let interaction = Interaction::default();
    let input = interaction.clone();
    let state = state();
    let pending = interaction.capture(&state, "human", false);
    assert!(pending.suppression(&interaction, &state, "human").is_none());
    input.note();
    assert_eq!(state.focus_field.as_deref(), Some("Transform.position"));
    assert!(pending.suppression(&interaction, &state, "human").is_some());
    let next = interaction.capture(&state, "human", false);
    assert!(next.suppression(&interaction, &state, "human").is_none());
}

#[test]
fn navigation_context_changes_suppress_without_an_authoring_revision_or_native_event() {
    let interaction = Interaction::default();
    let original = state();
    let changes: [fn(&mut EditorState); 10] = [
        |s: &mut EditorState| s.doc = "sc_elsewhere".into(),
        |s: &mut EditorState| s.selection.clear(),
        |s: &mut EditorState| s.focus = "assets".into(),
        |s: &mut EditorState| s.focus_field = Some("Transform.scale".into()),
        |s: &mut EditorState| s.resource = Some("gr_elsewhere".into()),
        |s: &mut EditorState| s.camera.distance += 1.,
        |s: &mut EditorState| s.asset_path = Some("game/other.graph".into()),
        |s: &mut EditorState| s.asset_folder = "game/other".into(),
        |s: &mut EditorState| s.canvas.pan[0] += 1.,
        |s: &mut EditorState| {
            s.canvas_selection.insert("n_elsewhere".into());
        },
    ];
    for change in changes {
        let pending = interaction.capture(&original, "human", false);
        let mut changed = original.clone();
        change(&mut changed);
        assert!(
            pending
                .suppression(&interaction, &changed, "human")
                .is_some()
        );
    }
}

#[test]
fn absent_agent_presence_never_borrows_later_human_navigation() {
    let interaction = Interaction::default();
    let mut state = state();
    let pending = interaction.capture(&state, "agent", true);
    interaction.note();
    state.doc = "sc_human_elsewhere".into();
    state.selection.clear();
    assert!(pending.suppression(&interaction, &state, "agent").is_none());
    state.presence.insert(
        "agent".into(),
        AgentPresence {
            author: "agent:test".into(),
            doc: "sc_agent_elsewhere".into(),
            selection: vec![],
            camera: Camera::default(),
        },
    );
    assert!(pending.suppression(&interaction, &state, "agent").is_some());
}

#[test]
fn agents_compare_only_their_own_private_presence() {
    let interaction = Interaction::default();
    let mut state = state();
    let presence = AgentPresence {
        author: "agent:test".into(),
        doc: "sc_original".into(),
        selection: vec!["e_original".into()],
        camera: Camera::default(),
    };
    state.presence.insert("agent".into(), presence.clone());
    let pending = interaction.capture(&state, "agent", true);
    state.presence.insert("other-agent".into(), presence);
    state.camera.distance += 1.;
    assert!(pending.suppression(&interaction, &state, "agent").is_none());
    state
        .presence
        .get_mut("agent")
        .expect("presence")
        .selection
        .clear();
    assert!(pending.suppression(&interaction, &state, "agent").is_some());
}
