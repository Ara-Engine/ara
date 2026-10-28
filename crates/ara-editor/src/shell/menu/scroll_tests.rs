//! Real menu rows keep their complete labels reachable through retained wheel and keys.
use super::*;
use ara_ui::input::InputEvent;
use ara_ui::source::stable_id;

struct Clipboard;
impl ara_ui_text::Clipboard for Clipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}

fn popup(shell: &Shell) -> ara_ui::Result<Box2> {
    let index = shell.shown_node("menu-pop-0").ok_or("Menu popup missing")?;
    let rect = shell.ui.nodes[index].rect;
    assert!(rect.w > 80. && rect.h > 40.);
    assert!(
        rect.x >= -0.5
            && rect.y >= -0.5
            && rect.x + rect.w <= shell.ui.size[0] + 0.5
            && rect.y + rect.h <= shell.ui.size[1] + 0.5,
        "Menu leaves its logical viewport: {rect:?} in {:?}",
        shell.ui.size
    );
    Ok(rect)
}

fn visible_row(shell: &Shell, id: &str) -> ara_ui::Result<[f32; 2]> {
    let index = shell.shown_node(id).ok_or("Menu row missing")?;
    let node = &shell.ui.nodes[index];
    let shown = node.rect.intersection(node.clip);
    assert!(
        node.rect.h >= 25.5 && shown.w >= node.rect.w - 0.5 && shown.h >= node.rect.h - 0.5,
        "{id} has only a partial pointer target: {:?} in {:?}",
        node.rect,
        node.clip
    );
    let label = shell
        .shown_node(&format!("{id}-label"))
        .ok_or("Menu label missing")?;
    let label = &shell.ui.nodes[label];
    let shown = label.rect.intersection(label.clip);
    assert!(
        shown.w >= label.rect.w - 0.5 && shown.h >= label.rect.h - 0.5,
        "{id} label is clipped: {:?} in {:?}",
        label.rect,
        label.clip
    );
    let point = [
        node.rect.x + node.rect.w * 0.5,
        node.rect.y + node.rect.h * 0.5,
    ];
    assert!(
        shell.ui.scrollbar_at(point).is_none(),
        "Scrollbar obscures {id}"
    );
    assert_eq!(
        shell.ui.hit_test(point[0], point[1]),
        Some(index),
        "{id} is obstructed"
    );
    Ok(point)
}

fn wheel_to(shell: &mut Shell, id: &str) -> ara_ui::Result<()> {
    for _ in 0..128 {
        let index = shell.shown_node(id).ok_or("Menu row missing")?;
        let node = &shell.ui.nodes[index];
        let shown = node.rect.intersection(node.clip);
        if shown.w >= node.rect.w - 0.5 && shown.h >= node.rect.h - 0.5 {
            visible_row(shell, id)?;
            return Ok(());
        }
        let delta = if node.rect.y < node.clip.y {
            node.rect.y - node.clip.y
        } else {
            node.rect.y + node.rect.h - node.clip.y - node.clip.h
        }
        .clamp(-64., 64.);
        let rect = popup(shell)?;
        shell.event(
            InputEvent::Wheel {
                position: [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5],
                delta: [0., delta],
            },
            &mut Clipboard,
        )?;
        shell.frame()?;
    }
    Err(format!("Menu row {id} cannot be reached by wheel").into())
}

fn active_row(shell: &Shell, editor: &Editor) -> ara_ui::Result<String> {
    let menu = shell.menu.as_ref().ok_or("Menu closed during navigation")?;
    let levels = shell.menu_levels(editor);
    let level = levels.len().checked_sub(1).ok_or("Menu has no rows")?;
    let row = menu
        .active
        .and_then(|active| levels[level].get(active))
        .ok_or("No active menu row")?;
    Ok(shell.menu_row_id(editor, level, row))
}

fn click_intent(shell: &mut Shell, id: &str) -> ara_ui::Result<()> {
    let point = visible_row(shell, id)?;
    shell.event(InputEvent::PointerDown(point), &mut Clipboard)?;
    shell.event(InputEvent::PointerUp(point), &mut Clipboard)?;
    assert!(
        shell
            .ui
            .drain_actions()
            .any(|action| action.node == stable_id(id)),
        "Menu click emitted no action for {id}"
    );
    Ok(())
}

#[test]
fn entity_context_menu_reveals_the_complete_prefab_row_with_wheel_and_keyboard()
-> ara_ui::Result<()> {
    let (mut editor, mut shell) = prefab_source_tests::open()?;
    editor.action("editor.select", json!({"entities":["e_unrelated"]}))?;
    editor.action(
        "editor.layout.move",
        json!({"panel":"assets","slot":"float","rect":[80,80,320,260]}),
    )?;
    shell.rebuild(&mut editor)?;
    let window = editor
        .state
        .floating
        .first()
        .ok_or("Floating host missing")?
        .id
        .clone();
    let revision = editor.host.store().revision();
    for (size, floating) in [
        ([1440., 900.], false),
        ([720., 450.], false),
        ([320., 260.], true),
    ] {
        shell.leave();
        if floating {
            shell.resize_surface(&window, size)?;
            assert!(shell.enter(&window));
        } else {
            shell.ui.resize(size)?;
        }
        shell.open_menu_at(
            Source::Context(Context::Entity, Target::Entity("e_unrelated".into())),
            [size[0] * 0.4, size[1] * 0.4],
        );
        shell.rebuild(&mut editor)?;
        shell.frame()?;
        popup(&shell)?;
        let id = shell
            .ui
            .nodes
            .iter()
            .find(|node| {
                matches!(
                    shell.bindings.get(&stable_id(&node.element.id)),
                    Some(Binding::Command("create.prefab", _))
                )
            })
            .map(|node| node.element.id.clone())
            .ok_or("Prefab command row missing")?;
        wheel_to(&mut shell, &id)?;
        click_intent(&mut shell, &id)?;
        shell.key(&mut editor, 74, true, &mut Clipboard)?;
        visible_row(&shell, &active_row(&shell, &editor)?)?;
        for _ in 0..32 {
            if active_row(&shell, &editor)? == id {
                break;
            }
            shell.key(&mut editor, 81, true, &mut Clipboard)?;
            visible_row(&shell, &active_row(&shell, &editor)?)?;
        }
        assert_eq!(active_row(&shell, &editor)?, id);
        visible_row(&shell, &id)?;
        assert_eq!(editor.host.store().revision(), revision);
    }
    Ok(())
}

#[test]
fn long_registered_menu_last_row_scrolls_at_one_and_two_x_logical_viewports() -> ara_ui::Result<()>
{
    let (mut editor, mut shell) = prefab_source_tests::open()?;
    editor.extensions = crate::extensions::Catalog {
        menus: (0..36)
            .map(|index| crate::extensions::Menu {
                path: format!(
                    "Scroll proof/{index:02} Descriptive command with a wrapped label · إعدادات"
                ),
                command: "view.show.orientation".into(),
            })
            .collect(),
        ..Default::default()
    };
    shell.rebuild(&mut editor)?;
    let revision = editor.host.store().revision();
    // Native hosts divide physical size by monitor/UI scale before retaining Ui
    // geometry. Cover both logical viewports without opening an OS window.
    for scale in [1., 2.] {
        shell.ui.resize([640. / scale, 520. / scale])?;
        shell.open_menu_at(Source::Bar("Scroll proof".into()), [20., 20.]);
        shell.rebuild(&mut editor)?;
        shell.frame()?;
        let rect = popup(&shell)?;
        assert!(
            rect.x * scale + rect.w * scale <= 640.5 && rect.y * scale + rect.h * scale <= 520.5
        );
        let levels = shell.menu_levels(&editor);
        assert_eq!(
            levels[0].len(),
            36,
            "Overflow must preserve every registration"
        );
        let last = shell.menu_row_id(&editor, 0, levels[0].last().ok_or("Final row missing")?);
        let root = shell.shown_node("menu-pop-0").ok_or("Menu popup missing")?;
        assert!(
            shell.ui.scroll_extent(root)?[1] > shell.ui.nodes[root].rect.h,
            "Fixture must exercise overflow"
        );
        wheel_to(&mut shell, &last)?;
        click_intent(&mut shell, &last)?;
        shell.key(&mut editor, 74, true, &mut Clipboard)?;
        visible_row(&shell, &active_row(&shell, &editor)?)?;
        shell.key(&mut editor, 77, true, &mut Clipboard)?;
        assert_eq!(active_row(&shell, &editor)?, last);
        visible_row(&shell, &last)?;
        assert_eq!(editor.host.store().revision(), revision);
    }
    Ok(())
}
