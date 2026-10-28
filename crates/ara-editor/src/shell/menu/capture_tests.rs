//! Retained pointer captures never change command, target or menu root on rebuild.
use super::*;
use ara_ui::input::InputEvent;

#[test]
fn native_view_show_gizmo_and_orientation_rows_toggle_both_ways_and_update_checkmarks()
-> ara_ui::Result<()> {
    for command in ["view.show.gizmos", "view.show.orientation"] {
        let (mut editor, mut shell) = prefab_source_tests::open(true)?;
        let mut clipboard = Clipboard::default();
        let before = (editor.state.hide_gizmos, editor.state.hide_orientation);
        let content_revision = editor.host.store().revision();
        let original = if command == "view.show.gizmos" {
            before.0
        } else {
            before.1
        };
        for expected_hidden in [!original, original] {
            let (id, point) = header(&shell, "View");
            press(&mut shell, &mut editor, &mut clipboard, &id, point)?;
            release(&mut shell, &mut editor, &mut clipboard, point)?;
            paint(&mut shell, &mut editor)?;
            assert_eq!(shell.menu.as_ref().and_then(MenuState::bar), Some("View"));
            let show = shell
                .menu_item_node(&editor, "Show")
                .expect("View Show submenu");
            let node = &shell.ui.nodes[shell.ui.find(&show).expect("Retained Show submenu")];
            let point = [
                node.rect.x + node.rect.w * 0.5,
                node.rect.y + node.rect.h * 0.5,
            ];
            press(&mut shell, &mut editor, &mut clipboard, &show, point)?;
            release(&mut shell, &mut editor, &mut clipboard, point)?;
            paint(&mut shell, &mut editor)?;
            assert_eq!(
                shell.menu.as_ref().expect("Open Show submenu").open,
                ["Show"]
            );
            let (id, point) = row(&shell, command);
            let visible_before = if command == "view.show.gizmos" {
                !editor.state.hide_gizmos
            } else {
                !editor.state.hide_orientation
            };
            let icon = &shell.ui.nodes[shell
                .ui
                .find(&format!("{id}-c-icon"))
                .expect("Visible menu check or command icon")];
            assert_eq!(
                icon.element.attrs.get("src").map(String::as_str) == Some("icon-check"),
                visible_before,
                "Before the click, the checkmark must represent visibility"
            );
            press(&mut shell, &mut editor, &mut clipboard, &id, point)?;
            release(&mut shell, &mut editor, &mut clipboard, point)?;
            assert!(shell.menu.is_none());
            assert_eq!(
                shell.recent_commands.first().map(String::as_str),
                Some(command)
            );
            let actual = if command == "view.show.gizmos" {
                editor.state.hide_gizmos
            } else {
                editor.state.hide_orientation
            };
            assert_eq!(
                actual, expected_hidden,
                "Native menu command must toggle {command}"
            );
            assert_eq!(
                editor.host.store().revision(),
                content_revision,
                "Visibility is navigation, not authored content"
            );
        }
        assert_eq!(
            (editor.state.hide_gizmos, editor.state.hide_orientation),
            before
        );
    }
    Ok(())
}

#[derive(Default)]
struct Clipboard(String);
impl ara_ui_text::Clipboard for Clipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.into();
        Ok(())
    }
}
fn catalog(editor: &mut Editor, generation: Option<&str>, placements: &[(&str, &str)]) {
    editor.extensions = crate::extensions::Catalog {
        generation: generation.map(str::to_owned),
        menus: placements
            .iter()
            .map(|(path, command)| crate::extensions::Menu {
                path: (*path).into(),
                command: (*command).into(),
            })
            .collect(),
        ..Default::default()
    };
}
fn paint(shell: &mut Shell, editor: &mut Editor) -> ara_ui::Result<()> {
    shell.rebuild(editor)?;
    shell.frame()?;
    Ok(())
}
fn row(shell: &Shell, command: &str) -> (String, [f32; 2]) {
    let node=shell.ui.nodes.iter().find(|node|node.element.id.starts_with('m')&&matches!(shell.bindings.get(&ara_ui::source::stable_id(&node.element.id)),Some(Binding::Command(id,_) ) if *id==command)).expect("Visible command row");
    assert!(
        shell
            .ui
            .shown(shell.ui.find(&node.element.id).expect("Retained menu row"))
    );
    (
        node.element.id.clone(),
        [
            node.rect.x + node.rect.w * 0.5,
            node.rect.y + node.rect.h * 0.5,
        ],
    )
}
fn header(shell: &Shell, title: &str) -> (String, [f32; 2]) {
    let node=shell.ui.nodes.iter().find(|node|matches!(shell.bindings.get(&ara_ui::source::stable_id(&node.element.id)),Some(Binding::MenuBar(bound)) if bound==title)).expect("Visible header");
    (
        node.element.id.clone(),
        [
            node.rect.x + node.rect.w * 0.5,
            node.rect.y + node.rect.h * 0.5,
        ],
    )
}
fn press(
    shell: &mut Shell,
    editor: &mut Editor,
    clipboard: &mut Clipboard,
    id: &str,
    point: [f32; 2],
) -> ara_ui::Result<()> {
    assert_eq!(
        shell
            .ui
            .hit_test(point[0], point[1])
            .map(|index| shell.ui.nodes[index].element.id.as_str()),
        Some(id)
    );
    shell.pointer = point;
    shell.event(InputEvent::PointerDown(point), clipboard)?;
    shell.dispatch(editor)?;
    assert_eq!(shell.input.captured(), Some(ara_ui::source::stable_id(id)));
    Ok(())
}
fn release(
    shell: &mut Shell,
    editor: &mut Editor,
    clipboard: &mut Clipboard,
    point: [f32; 2],
) -> ara_ui::Result<()> {
    shell.event(InputEvent::PointerUp(point), clipboard)?;
    shell.dispatch(editor)?;
    Ok(())
}

#[test]
fn held_row_cannot_rebind_to_another_command_or_new_callback_generation() -> ara_ui::Result<()> {
    for next_generation in [None, Some("next")] {
        let (mut editor, mut shell) = prefab_source_tests::open(true)?;
        let mut clipboard = Clipboard::default();
        catalog(&mut editor, None, &[("Custom/Action", "view.show.grid")]);
        shell.open_bar_title("Custom", false);
        paint(&mut shell, &mut editor)?;
        let (id, point) = row(&shell, "view.show.grid");
        let before = editor.state.settings["viewport.grid"].clone();
        let gizmos = editor.state.hide_gizmos;
        press(&mut shell, &mut editor, &mut clipboard, &id, point)?;
        catalog(
            &mut editor,
            next_generation,
            &[("Custom/Action", "view.show.gizmos")],
        );
        paint(&mut shell, &mut editor)?;
        assert!(
            shell.ui.find(&id).is_none(),
            "Changed semantics must retire the captured node"
        );
        release(&mut shell, &mut editor, &mut clipboard, point)?;
        assert_eq!(editor.state.settings["viewport.grid"], before);
        assert_eq!(
            editor.state.hide_gizmos, gizmos,
            "Release must not run the replacement"
        );
        assert!(
            shell.recent_commands.is_empty(),
            "Stale release must not dispatch either command"
        );
        let (fresh, fresh_point) = row(&shell, "view.show.gizmos");
        press(&mut shell, &mut editor, &mut clipboard, &fresh, fresh_point)?;
        release(&mut shell, &mut editor, &mut clipboard, fresh_point)?;
        assert!(
            shell.menu.is_none(),
            "Fresh click must activate and close the menu"
        );
        assert_eq!(
            shell.recent_commands.first().map(String::as_str),
            Some("view.show.gizmos")
        );
        assert_ne!(
            editor.state.hide_gizmos, gizmos,
            "Fresh current-menu input remains usable"
        );
        shell.open_bar_title("Custom", false);
        paint(&mut shell, &mut editor)?;
        let (again, again_point) = row(&shell, "view.show.gizmos");
        press(&mut shell, &mut editor, &mut clipboard, &again, again_point)?;
        release(&mut shell, &mut editor, &mut clipboard, again_point)?;
        assert_eq!(
            editor.state.hide_gizmos, gizmos,
            "A second native click restores the original visibility"
        );
    }
    let (mut editor, mut shell) = prefab_source_tests::open(true)?;
    let mut clipboard = Clipboard::default();
    catalog(
        &mut editor,
        Some("before"),
        &[("Custom/Action", "view.show.gizmos")],
    );
    shell.open_bar_title("Custom", false);
    paint(&mut shell, &mut editor)?;
    let (id, point) = row(&shell, "view.show.gizmos");
    let before = editor.state.hide_gizmos;
    press(&mut shell, &mut editor, &mut clipboard, &id, point)?;
    catalog(
        &mut editor,
        Some("after"),
        &[("Custom/Action", "view.show.gizmos")],
    );
    paint(&mut shell, &mut editor)?;
    assert!(
        shell.ui.find(&id).is_none(),
        "Identical command IDs can contain new extension code"
    );
    release(&mut shell, &mut editor, &mut clipboard, point)?;
    assert!(
        shell.recent_commands.is_empty(),
        "An old callback generation must not dispatch"
    );
    assert_eq!(editor.state.hide_gizmos, before);
    Ok(())
}

#[test]
fn unchanged_held_command_survives_sibling_removal_and_keyboard_focus_tracks_identity()
-> ara_ui::Result<()> {
    let (mut editor, mut shell) = prefab_source_tests::open(true)?;
    let mut clipboard = Clipboard::default();
    catalog(
        &mut editor,
        Some("same"),
        &[
            ("Custom/A first", "view.show.grid"),
            ("Custom/Z last", "view.show.gizmos"),
        ],
    );
    shell.open_bar_title("Custom", false);
    paint(&mut shell, &mut editor)?;
    shell.menu.as_mut().expect("Open menu").active = Some(1);
    paint(&mut shell, &mut editor)?;
    let (id, point) = row(&shell, "view.show.gizmos");
    let before = editor.state.hide_gizmos;
    press(&mut shell, &mut editor, &mut clipboard, &id, point)?;
    catalog(
        &mut editor,
        Some("same"),
        &[("Custom/Z last", "view.show.gizmos")],
    );
    paint(&mut shell, &mut editor)?;
    assert_eq!(shell.menu.as_ref().expect("Open menu").active, Some(0));
    assert_eq!(row(&shell, "view.show.gizmos").0, id);
    assert_ne!(
        row(&shell, "view.show.gizmos").1,
        point,
        "The actual row must have moved"
    );
    release(&mut shell, &mut editor, &mut clipboard, point)?;
    assert!(
        shell.menu.is_none(),
        "The retained capture must activate and close its menu"
    );
    assert_eq!(
        shell.recent_commands.first().map(String::as_str),
        Some("view.show.gizmos")
    );
    assert_ne!(
        editor.state.hide_gizmos, before,
        "The retained capture still means the same command"
    );
    Ok(())
}

#[test]
fn top_level_capture_and_open_submenu_follow_names_instead_of_positions() -> ara_ui::Result<()> {
    let (mut editor, mut shell) = prefab_source_tests::open(true)?;
    let mut clipboard = Clipboard::default();
    catalog(
        &mut editor,
        Some("same"),
        &[("Zulu/Nested/Action", "view.show.grid")],
    );
    paint(&mut shell, &mut editor)?;
    let (id, point) = header(&shell, "Zulu");
    press(&mut shell, &mut editor, &mut clipboard, &id, point)?;
    catalog(
        &mut editor,
        Some("same"),
        &[
            ("Alpha/Action", "view.show.gizmos"),
            ("Zulu/Nested/Action", "view.show.grid"),
        ],
    );
    paint(&mut shell, &mut editor)?;
    assert_eq!(header(&shell, "Zulu").0, id);
    assert_ne!(header(&shell, "Zulu").1, point);
    release(&mut shell, &mut editor, &mut clipboard, point)?;
    assert_eq!(shell.menu.as_ref().and_then(MenuState::bar), Some("Zulu"));
    shell
        .menu
        .as_mut()
        .expect("Open menu")
        .open
        .push("Nested".into());
    paint(&mut shell, &mut editor)?;
    catalog(
        &mut editor,
        Some("reload"),
        &[
            ("Zulu/Before/Action", "view.show.gizmos"),
            ("Zulu/Nested/Action", "view.show.grid"),
            ("Alpha/Action", "view.show.gizmos"),
        ],
    );
    paint(&mut shell, &mut editor)?;
    assert_eq!(shell.menu.as_ref().expect("Open menu").open, ["Nested"]);
    assert!(
        matches!(shell.menu_levels(&editor).last().and_then(|rows|rows.first()),Some(Row::Command{id,..}) if id=="view.show.grid")
    );
    catalog(
        &mut editor,
        Some("next"),
        &[("Zulu/Before/Action", "view.show.gizmos")],
    );
    paint(&mut shell, &mut editor)?;
    assert!(
        shell
            .menu
            .as_ref()
            .expect("Root remains valid")
            .open
            .is_empty()
    );
    catalog(
        &mut editor,
        Some("removed"),
        &[("Alpha/Action", "view.show.gizmos")],
    );
    paint(&mut shell, &mut editor)?;
    assert!(
        shell.menu.is_none(),
        "Removed root must not become a different root"
    );
    Ok(())
}

#[test]
fn old_header_generation_and_changed_display_title_cancel_the_capture() -> ara_ui::Result<()> {
    let (mut editor, mut shell) = prefab_source_tests::open(true)?;
    let mut clipboard = Clipboard::default();
    catalog(
        &mut editor,
        Some("old"),
        &[("Custom/Old title", "view.show.gizmos")],
    );
    paint(&mut shell, &mut editor)?;
    let (id, point) = header(&shell, "Custom");
    press(&mut shell, &mut editor, &mut clipboard, &id, point)?;
    catalog(
        &mut editor,
        Some("new"),
        &[("Custom/Old title", "view.show.gizmos")],
    );
    paint(&mut shell, &mut editor)?;
    release(&mut shell, &mut editor, &mut clipboard, point)?;
    assert!(shell.menu.is_none());
    shell.open_bar_title("Custom", false);
    paint(&mut shell, &mut editor)?;
    let (id, point) = row(&shell, "view.show.gizmos");
    let before = editor.state.hide_gizmos;
    press(&mut shell, &mut editor, &mut clipboard, &id, point)?;
    catalog(
        &mut editor,
        Some("new"),
        &[("Custom/New title", "view.show.gizmos")],
    );
    paint(&mut shell, &mut editor)?;
    assert!(shell.ui.find(&id).is_none());
    release(&mut shell, &mut editor, &mut clipboard, point)?;
    assert_eq!(editor.state.hide_gizmos, before);
    Ok(())
}

#[test]
fn held_context_command_cannot_copy_a_different_asset_after_browser_reorder() -> ara_ui::Result<()>
{
    let (mut editor, mut shell) = prefab_source_tests::open(true)?;
    let mut clipboard = Clipboard::default();
    shell.browser.items = [("base", "pf_menubase"), ("outer", "pf_menuouter")]
        .into_iter()
        .map(|(name, id)| {
            let logical = format!("prefabs/{name}.prefab");
            assets::Asset {
                path: editor.host.store().workspace().storage_path(&logical),
                authored_path: Some(logical.clone()),
                document_kind: Some("prefab".into()),
                reference: Some(format!("@{logical}#{id}")),
                kind: "prefab".into(),
                domain: None,
                uses: Vec::new(),
                stamp: 0,
            }
        })
        .collect();
    let a = shell.browser.items[0].path.clone();
    let b = shell.browser.items[1].path.clone();
    shell.open_menu_at(
        Source::Context(Context::Asset, Target::Asset(a)),
        [240., 180.],
    );
    paint(&mut shell, &mut editor)?;
    let (id, point) = row(&shell, "assets.copy_reference");
    press(&mut shell, &mut editor, &mut clipboard, &id, point)?;
    shell.browser.items.reverse();
    shell.open_menu_at(
        Source::Context(Context::Asset, Target::Asset(b)),
        [240., 180.],
    );
    paint(&mut shell, &mut editor)?;
    assert!(shell.ui.find(&id).is_none());
    release(&mut shell, &mut editor, &mut clipboard, point)?;
    assert!(
        shell.pending_clipboard.is_none(),
        "A held Copy Reference must not copy B"
    );
    let (fresh, point) = row(&shell, "assets.copy_reference");
    press(&mut shell, &mut editor, &mut clipboard, &fresh, point)?;
    release(&mut shell, &mut editor, &mut clipboard, point)?;
    assert_eq!(
        shell.pending_clipboard.as_deref(),
        Some("@prefabs/outer.prefab#pf_menuouter")
    );
    Ok(())
}

#[test]
fn implicit_selection_target_change_cancels_held_entity_command() -> ara_ui::Result<()> {
    let (mut editor, mut shell) = prefab_source_tests::open(true)?;
    let mut clipboard = Clipboard::default();
    editor.action("editor.select", json!({"entities":["e_unrelated"]}))?;
    catalog(
        &mut editor,
        None,
        &[("Custom/Copy Reference", "edit.copy_reference")],
    );
    shell.open_bar_title("Custom", false);
    paint(&mut shell, &mut editor)?;
    let (id, point) = row(&shell, "edit.copy_reference");
    press(&mut shell, &mut editor, &mut clipboard, &id, point)?;
    editor.action("editor.select", json!({"entities":["e_instance"]}))?;
    paint(&mut shell, &mut editor)?;
    assert!(shell.ui.find(&id).is_none());
    release(&mut shell, &mut editor, &mut clipboard, point)?;
    assert!(shell.pending_clipboard.is_none());
    Ok(())
}

#[test]
fn single_segment_registration_uses_registered_title_and_deterministic_roots() -> ara_ui::Result<()>
{
    let (mut editor, mut shell) = prefab_source_tests::open(true)?;
    catalog(
        &mut editor,
        None,
        &[
            ("Zulu", "view.show.grid"),
            ("Tools", "view.show.gizmos"),
            ("Alpha", "view.show.grid"),
            ("Tools", "view.show.gizmos"),
        ],
    );
    assert_eq!(
        commands::bar_titles(&editor.extensions)
            .into_iter()
            .skip(MENUS.len())
            .collect::<Vec<_>>(),
        ["Alpha", "Tools", "Zulu"]
    );
    assert_eq!(
        commands::registered_menu_path(&editor.extensions, "view.show.gizmos"),
        "Tools"
    );
    shell.open_bar_title("Tools", false);
    paint(&mut shell, &mut editor)?;
    let rows = shell.open_menu_rows(&editor);
    let registered: Vec<_> = rows
        .iter()
        .filter(|(id, _, _)| id == "view.show.gizmos")
        .collect();
    assert_eq!(
        registered.len(),
        1,
        "Repeated exact placements must not duplicate interactive IDs"
    );
    assert_eq!(registered[0].1, "Transform Gizmo");
    let first = shell.menu_levels(&editor);
    editor.extensions.menus.reverse();
    paint(&mut shell, &mut editor)?;
    assert_eq!(
        first[0]
            .iter()
            .map(|row| shell.row_identity(&editor, row))
            .collect::<Vec<_>>(),
        shell.menu_levels(&editor)[0]
            .iter()
            .map(|row| shell.row_identity(&editor, row))
            .collect::<Vec<_>>()
    );
    Ok(())
}
