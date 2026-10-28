//! Capture hit testing must use logical pixels at every native density and UI scale.
use super::*;

#[test]
fn captured_motion_then_press_keeps_the_game_target_at_independent_window_scales()
-> ara_ui::Result<()> {
    let root = crate::test_target().join(format!("game-input-scales-{}", std::process::id()));
    std::fs::create_dir_all(root.join("levels"))?;
    std::fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(
            &json!({"name":"Game input scale fixture","main":"levels/main.scene"}),
        ),
    )?;
    std::fs::write(
        root.join("levels/main.scene"),
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
            game_input(&event, &mut editor, &mut shell, density / dpi),
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
                density / dpi
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
                    1.
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
