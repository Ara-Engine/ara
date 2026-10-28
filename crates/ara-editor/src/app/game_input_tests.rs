//! Capture hit testing must use logical pixels at every native density and UI scale.
use super::*;

#[test]
fn captured_motion_then_press_keeps_the_game_target_at_independent_window_scales()
-> ara_ui::Result<()> {
    let root = crate::test_target().join(format!("game-input-scales-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    std::fs::write(
        root.join("ara.project.json"),
        "{\"name\":\"Game input scale fixture\"}",
    )?;
    let mut editor = Editor::open_with(&root, Some(root.join(".ara/user.json")))?;
    let mut shell = Shell::new(&mut editor, [900., 600.])?;
    shell.offscreen = true;
    shell.ui = ara_ui::Ui::editor(
        "<ui id='root'><panel id='game-frame-0' style='position:absolute;left:300px;top:200px;width:400px;height:200px' /></ui>",
        ara_ui::theme::EDITOR_CSS,
        [900., 600.],
    )?;
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
    assert_eq!(editor.host.store().revision(), revision);
    drop(shell);
    drop(editor);
    assert!(root.starts_with(crate::test_target()));
    std::fs::remove_dir_all(root)?;
    Ok(())
}
