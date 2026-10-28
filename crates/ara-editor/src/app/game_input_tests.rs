//! Capture hit testing must use logical pixels at every native density and UI scale.
use super::*;

fn logical_pointer(actual: [f32; 2], expected: [f32; 2]) {
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| (*actual - expected).abs() <= 0.001),
        "Logical pointer {actual:?}, expected {expected:?}"
    );
}

#[test]
fn captured_motion_then_press_keeps_the_game_target_at_independent_window_scales()
-> ara_ui::Result<()> {
    let root = crate::test_target().join(format!("game-input-scales-{}", std::process::id()));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels"))?;
    std::fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(
            &json!({"name":"Game input scale fixture","main":"levels/main.scene","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
        ),
    )?;
    std::fs::write(
        root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
        ara_json::to_canonical_string(
            &json!({"$schema":"ara://schema/scene/1","id":"sc_gamescal1","records":{}}),
        ),
    )?;
    let mut editor = Editor::open_with(&root, Some(root.join(".ara/user.json")))?;
    let mut shell = Shell::new(&mut editor, [900., 600.])?;
    shell.offscreen = true;
    shell.ui = ara_ui::Ui::editor(
        "<ui id='root'><panel id='game-frame-0' style='position:absolute;left:300px;top:200px;width:400px;height:200px' /></ui>",
        ara_ui::theme::EDITOR_CSS,
        [900., 600.],
    )?;
    shell.frame()?;
    let viewport = shell
        .game_input_rect()
        .ok_or("Missing retained Game target")?;
    assert!(viewport.contains(600., 350.), "{viewport:?}");
    let revision = editor.host.store().revision();
    for (density, dpi) in [(1., 1.), (2., 2.), (1., 1.25), (2., 2.5)] {
        shell.game_captured = true;
        let target = [600., 350.];
        let event = Event::MouseMotion {
            position: target.map(|value| value * dpi / density),
            delta: [4., 2.],
        };
        assert_eq!(
            game_input(&event, &mut editor, &mut shell, density / dpi, None),
            Some(false)
        );
        assert_eq!(shell.pointer, target);
        assert_eq!(
            game_input(
                &Event::MouseButton {
                    button: 1,
                    pressed: true
                },
                &mut editor,
                &mut shell,
                density / dpi,
                None,
            ),
            Some(false),
        );
        assert!(
            shell.game_captured,
            "The press remains inside the logical Game viewport at density {density}, DPI {dpi}"
        );
    }
    for scancode in 224..=231 {
        for pressed in [true, false] {
            assert_eq!(
                game_input(
                    &Event::Key {
                        scancode,
                        pressed,
                        repeat: false
                    },
                    &mut editor,
                    &mut shell,
                    1.,
                    None,
                ),
                None,
                "Modifier transitions must also reach the editor's global shortcut state",
            );
            assert!(shell.game_captured);
        }
    }
    assert_eq!(editor.host.store().revision(), revision);
    drop(shell);
    drop(editor);
    assert!(root.starts_with(crate::test_target()));
    std::fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn floating_game_capture_routes_motion_and_press_to_its_retained_surface() -> ara_ui::Result<()> {
    let root =
        crate::test_target().join(format!("floating-game-input-scales-{}", std::process::id()));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels"))?;
    for (path, value) in [
        (
            "ara.project.json",
            json!({"name":"Floating Game input","main":"levels/main.scene","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
        ),
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_floatinput","records":{}}),
        ),
    ] {
        let physical = if path == "ara.project.json" {
            root.join(path)
        } else {
            root.join(ara_doc::AUTHORING_ROOT).join(path)
        };
        std::fs::write(physical, ara_json::to_canonical_string(&value))?;
    }
    let mut editor = Editor::open(&root)?;
    let mut shell = Shell::new(&mut editor, [1280., 800.])?;
    shell.offscreen = true;
    editor.action("editor.view", json!({"play":"paused"}))?;
    // Cached surface presentation only: there is no player, SDL window or GPU.
    shell.game_players = vec![0];
    shell.play_controls.players.insert(
        0,
        crate::shell::play_controls::PlayerInfo {
            live: true,
            size: [640, 360],
            fps: 60,
        },
    );
    shell.game_client = 0;
    shell.game_running = true;
    editor.action(
        "editor.layout.move",
        json!({"panel":"game","slot":"float","rect":[80,80,640,420]}),
    )?;
    shell.rebuild(&mut editor)?;
    let float = editor.state.floating.iter().find(|window| matches!(&window.root, ara_ui::docking::DockNode::Area {tabs,..} if tabs.iter().any(|panel|panel=="game")))
        .ok_or("The typed layout command did not float Game")?.id.clone();
    shell.resize_surface(&float, [640., 420.])?;
    assert!(shell.enter(&float));
    shell.frame()?;
    let rect = shell
        .game_input_rect()
        .ok_or("The floating Game tile is not mounted")?;
    assert!(rect.w > 100. && rect.h > 100., "{rect:?}");
    let target = [rect.x + rect.w * 0.6, rect.y + rect.h * 0.6];
    shell.leave();
    assert!(
        shell.game_input_rect().is_none(),
        "The main surface must not retain a Game tile after floating it"
    );
    let main_pointer = [1111., 733.];
    shell.pointer = main_pointer;
    let revision = editor.host.store().revision();
    for scale in [1., 1.25, 2., 2.5, 0.8] {
        shell.game_captured = true;
        assert_eq!(
            game_input(
                &Event::MouseMotion {
                    position: target.map(|value| value / scale),
                    delta: [4., 2.]
                },
                &mut editor,
                &mut shell,
                scale,
                Some(&float)
            ),
            Some(false)
        );
        assert_eq!(shell.current(), None);
        assert_eq!(
            shell.pointer, main_pointer,
            "Floating motion must not change the main pointer at scale {scale}"
        );
        assert!(shell.enter(&float));
        assert_eq!(shell.current(), Some(float.as_str()));
        logical_pointer(shell.pointer, target);
        assert!(
            shell
                .game_input_rect()
                .is_some_and(|rect| rect.contains(target[0], target[1]))
        );
        shell.leave();
        // Every press follows leave(), just like the normal editor frame. It
        // must re-enter the floating source rather than consult Main's rect.
        for button in [1, 3] {
            for pressed in [true, false] {
                assert_eq!(
                    game_input(
                        &Event::MouseButton { button, pressed },
                        &mut editor,
                        &mut shell,
                        scale,
                        Some(&float)
                    ),
                    Some(false)
                );
                assert!(
                    shell.game_captured,
                    "Floating Game lost capture at scale {scale}, button {button}, pressed {pressed}"
                );
                assert_eq!(shell.current(), None);
                assert_eq!(shell.pointer, main_pointer);
            }
        }
        assert!(shell.enter(&float));
        logical_pointer(shell.pointer, target);
        shell.leave();
    }
    shell.game_captured = true;
    assert_eq!(
        game_input(
            &Event::MouseButton {
                button: 1,
                pressed: true
            },
            &mut editor,
            &mut shell,
            1.,
            Some("closed-game-surface")
        ),
        None
    );
    assert!(
        !shell.game_captured,
        "A removed event source must release capture rather than target Main"
    );
    assert_eq!(shell.current(), None);
    assert_eq!(shell.pointer, main_pointer);
    assert_eq!(
        editor.host.store().revision(),
        revision,
        "Capture routing must not author content"
    );
    drop(shell);
    drop(editor);
    assert!(root.starts_with(crate::test_target()));
    std::fs::remove_dir_all(root)?;
    Ok(())
}
