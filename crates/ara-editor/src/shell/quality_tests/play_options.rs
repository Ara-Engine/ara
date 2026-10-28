//! Explicit Play-options states supplement the general panel and menu catalogue.
use super::*;
use ara_ui::input::InputEvent;

pub(super) fn sweep(
    editor: &mut Editor,
    shell: &mut Shell,
    sweep: &mut Sweep,
    scale: &str,
    size: [f32; 2],
    capture: &mut impl FnMut(&mut Shell, &str),
) {
    for (key, value) in [("play.mode", json!("host")), ("play.clients", json!(2))] {
        editor
            .action("editor.settings.set", json!({"key":key,"value":value}))
            .unwrap();
    }
    show(editor, shell, "game");
    options_state(
        editor,
        shell,
        sweep,
        &format!("{scale}-play-options-main"),
        capture,
    );
    shell.click("game-options", &mut Clip::default()).unwrap();
    shell.dispatch(editor).unwrap();

    editor
        .action("editor.view", json!({"maximized":""}))
        .unwrap();
    let floating_size = [size[0] * 0.5, (size[1] * (2. / 3.)).min(480.)];
    editor.action("editor.layout.move", json!({"panel":"game","slot":"float","rect":[80,80,floating_size[0] as i32,floating_size[1] as i32]})).unwrap();
    shell.rebuild(editor).unwrap();
    let window = editor.state.floating.iter().find(|window| matches!(
        &window.root,
        ara_ui::docking::DockNode::Area { tabs, .. } if tabs.iter().any(|panel| panel == "game")
    )).expect("floating Game view").id.clone();
    shell.resize_surface(&window, floating_size).unwrap();
    assert!(shell.enter(&window));
    shell.frame().unwrap();
    options_state(
        editor,
        shell,
        sweep,
        &format!("{scale}-play-options-floating"),
        capture,
    );
    shell.click("game-options", &mut Clip::default()).unwrap();
    shell.dispatch(editor).unwrap();
    shell.leave();
    editor
        .action("editor.layout.move", json!({"panel":"game","slot":"view"}))
        .unwrap();
    shell.rebuild(editor).unwrap();
}

fn options_state(
    editor: &mut Editor,
    shell: &mut Shell,
    sweep: &mut Sweep,
    name: &str,
    capture: &mut impl FnMut(&mut Shell, &str),
) {
    assert!(!shell.play_controls.options, "options start closed: {name}");
    shell.click("game-options", &mut Clip::default()).unwrap();
    shell.dispatch(editor).unwrap();
    shell.frame().unwrap();
    let rail = &shell.ui.nodes[shell.shown_node("play-options").expect("Play options")];
    let viewport = rail.rect.intersection(rail.clip);
    if viewport.w <= 1. || viewport.h <= 1. || rail.rect.h > 260.5 {
        sweep.violations.push(format!(
            "{name}: Play options need a positive bounded viewport: rect {:?}, clip {:?}",
            rail.rect, rail.clip
        ));
    }
    for id in [
        "play-options-mode",
        "play-process-mode",
        "play-client-count",
        "play-start-paused",
        "play-network-1",
        "play-network-2",
    ] {
        reveal_control(shell, sweep, name, id);
    }
    sweep.context(shell, editor, name, capture);
    super::sweep_selects(editor, shell, sweep, name, capture);
}

fn reveal_control(shell: &mut Shell, sweep: &mut Sweep, context: &str, id: &str) {
    let mut reachable = false;
    for _ in 0..40 {
        let Some(index) = shell.shown_node(id) else {
            break;
        };
        let node = &shell.ui.nodes[index];
        let visible = node.rect.intersection(node.clip);
        if node.rect.w > 1.
            && node.rect.h > 1.
            && visible.w + 0.5 >= node.rect.w
            && visible.h + 0.5 >= node.rect.h
        {
            reachable = true;
            break;
        }
        let delta = if node.rect.y < node.clip.y {
            node.rect.y - node.clip.y
        } else {
            node.rect.y + node.rect.h - node.clip.y - node.clip.h
        }
        .clamp(-32., 32.);
        let rail = &shell.ui.nodes[shell.shown_node("play-options").expect("Play options")];
        let viewport = rail.rect.intersection(rail.clip);
        if viewport.w <= 1. || viewport.h <= 1. {
            break;
        }
        shell
            .event(
                InputEvent::Wheel {
                    position: [viewport.x + viewport.w * 0.5, viewport.y + viewport.h * 0.5],
                    delta: [0., delta],
                },
                &mut Clip::default(),
            )
            .unwrap();
        shell.frame().unwrap();
    }
    let geometry = shell.shown_node(id).map(|index| {
        let node = &shell.ui.nodes[index];
        json!({"rect":node.rect,"clip":node.clip})
    });
    sweep.measurements.push(json!({"context":context,"playControl":id,"geometry":geometry,"reachable":reachable,"viewport":shell.ui.size,"surface":shell.current()}));
    if !reachable {
        sweep.violations.push(format!(
            "{context}: Play control {id} is not fully wheel-reachable: {geometry:?}"
        ));
    }
}

#[test]
fn play_options_and_open_selects_pass_surface_identity_and_reachability_audits() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open([1440., 900.]);
    let mut measured = Sweep::default();
    for (scale, size) in [
        ("1x", [1440., 900.]),
        ("2x", [1280., 720.]),
        ("narrow", [640., 480.]),
    ] {
        shell.ui.resize(size).unwrap();
        sweep(
            &mut editor,
            &mut shell,
            &mut measured,
            scale,
            size,
            &mut |_, _| {},
        );
        for surface in ["main", "floating"] {
            let context = format!("{scale}-play-options-{surface}");
            assert!(measured.contexts.contains(&context));
            assert!(
                measured
                    .contexts
                    .contains(&format!("{context}-play-options-mode")),
                "the actual Play mode Select must open on {surface}"
            );
        }
    }
    assert!(
        measured.violations.is_empty(),
        "{}",
        measured.violations.join("\n")
    );
}
