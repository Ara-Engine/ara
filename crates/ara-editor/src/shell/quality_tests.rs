//! Mounted-editor gates: shared control choices and every reachable transient surface.
#![allow(clippy::unwrap_used)]
mod captures;
mod fixture;
mod measure;
mod row_sizing;

use super::*;
use commands::{Target, menus::Context};
use fixture::{Clip, Project, show};
use measure::Sweep;
use menu::{Row, Source};
use std::path::PathBuf;

#[test]
fn editor_overlays_and_control_choices_fit_all_mounted_contexts() {
    let project = Project::new();
    let output = project.0.join("measurements");
    let sweep = run_sweep(&project, &mut |_, _| {});
    sweep.write(&output);
    let retained = project
        .0
        .parent()
        .unwrap()
        .join("latest-quality-measurements.json");
    std::fs::copy(output.join("quality-measurements.json"), retained).unwrap();
    assert!(
        sweep.measurements.len() > 100,
        "sweep must actually mount and measure overlays"
    );
    assert!(
        sweep.violations.is_empty(),
        "{}\nMeasurements: {}",
        sweep
            .violations
            .iter()
            .take(80)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
        output.display()
    );
    // Keep the report after the scratch project is dropped so failed local gates
    // and successful checkpoints both have a reviewable measurement index.
}

#[test]
fn retained_main_tooltips_fit_their_final_shaped_text() {
    let project = Project::new();
    let (_editor, mut shell) = project.open([1440., 900.]);
    let mut measurements = Vec::new();
    for id in ["play", "crumbs", "fold-outline"] {
        let index = shell
            .shown_node(id)
            .unwrap_or_else(|| panic!("missing hover target {id}"));
        let bounds = shell.ui.nodes[index]
            .rect
            .intersection(shell.ui.nodes[index].clip);
        let mut hovered = false;
        for point in [
            [bounds.x + bounds.w * 0.5, bounds.y + bounds.h * 0.5],
            [bounds.x + 1., bounds.y + 1.],
        ] {
            shell.pointer = point;
            shell.hover_tooltip().unwrap();
            if shell.tooltip_target.as_ref().map(|(target, _)| *target)
                == Some(ara_ui::source::stable_id(id))
            {
                hovered = true;
                break;
            }
        }
        assert!(hovered, "{id} is a real hover target");
        shell
            .advance_tooltip_at(ara_ui::popover::SHOW_DELAY_MS + 10_000)
            .unwrap();
        shell.frame().unwrap();
        let tip = &shell.ui.nodes[shell.shown_node("tooltip").expect("visible tooltip")];
        let copy = &shell.ui.nodes[shell
            .shown_node("tooltip-text")
            .expect("visible tooltip copy")];
        let outer = shell.ui.tree.layout(tip.layout_id).unwrap();
        let text = copy.text.as_ref().expect("shaped copy");
        let mut unbroken = text.layout.clone();
        unbroken.break_all_lines(None);
        let required = text.layout.width()
            + outer.padding.left
            + outer.padding.right
            + outer.border.left
            + outer.border.right;
        measurements.push(json!({"target":id,"text":text.value,"textFontSize":text.size,"computedFontSize":copy.computed.font_size,"fontFamily":text.family,"textWidth":text.layout.width(),"unbrokenWidth":unbroken.width(),"lines":text.layout.len(),"requiredWidth":required,"box":tip.rect,"textBox":copy.rect,"padding":[outer.padding.left,outer.padding.right],"border":[outer.border.left,outer.border.right],"layout":format!("{:?}",shell.ui.tree.style(tip.layout_id).unwrap()),"fits":tip.rect.w<=required.max(ara_ui::popover::MIN_WIDTH)+16.}));
    }
    assert!(
        measurements
            .iter()
            .all(|measurement| measurement["fits"] == true),
        "{}",
        ara_json::to_canonical_string(&json!(measurements))
    );
}

fn run_sweep(project: &Project, capture: &mut impl FnMut(&mut Shell, &str)) -> Sweep {
    let (mut editor, mut shell) = project.open([1440., 900.]);
    let mut sweep = Sweep::default();
    for (scale, size) in [
        ("1x", [1440., 900.]),
        ("2x", [1280., 720.]),
        ("narrow", [640., 480.]),
    ] {
        shell.ui.resize(size).unwrap();
        editor
            .action("editor.view", json!({"maximized":""}))
            .unwrap();
        editor.action("editor.layout.reset", json!({})).unwrap();
        shell.rebuild(&mut editor).unwrap();
        sweep.context(&mut shell, &mut editor, &format!("{scale}-main"), capture);
        for panel in [
            "scene",
            "game",
            "hierarchy",
            "inspector",
            "assets",
            "material",
            "vfx",
            "animation",
            "profiler",
            "console",
            "problems",
            "agents",
            "history",
        ] {
            editor
                .action("editor.workspace", json!({"asset":""}))
                .unwrap();
            show(&mut editor, &mut shell, panel);
            sweep.context(
                &mut shell,
                &mut editor,
                &format!("{scale}-panel-{panel}"),
                capture,
            );
        }
        shell.open_build_options(&mut editor).unwrap();
        sweep.context(
            &mut shell,
            &mut editor,
            &format!("{scale}-build-options"),
            capture,
        );
        shell.close_build_options(&mut editor).unwrap();
        show(&mut editor, &mut shell, "settings");
        for (scope, key) in [
            (crate::settings::Scope::User, "user"),
            (crate::settings::Scope::Project, "project"),
        ] {
            let mut categories = crate::settings::categories(scope);
            categories.insert(0, "");
            if key == "user" {
                categories.push("Keyboard shortcuts");
            }
            for category in categories {
                editor
                    .action(
                        "editor.settings.view",
                        json!({"scope":key,"category":category,"modified":false}),
                    )
                    .unwrap();
                shell.rebuild(&mut editor).unwrap();
                sweep.context(
                    &mut shell,
                    &mut editor,
                    &format!("{scale}-settings-{key}-{category}"),
                    capture,
                );
                sweep_selects(
                    &mut editor,
                    &mut shell,
                    &mut sweep,
                    &format!("{scale}-choice"),
                    capture,
                );
            }
        }
        editor
            .action("editor.workspace", json!({"asset":"textures/quality.png"}))
            .unwrap();
        show(&mut editor, &mut shell, "inspector");
        sweep.context(
            &mut shell,
            &mut editor,
            &format!("{scale}-import-texture"),
            capture,
        );
        sweep_selects(
            &mut editor,
            &mut shell,
            &mut sweep,
            &format!("{scale}-import-choice"),
            capture,
        );
        editor
            .action("editor.select", json!({"entities":["e_quality1"]}))
            .unwrap();
        show(&mut editor, &mut shell, "inspector");
        sweep_selects(
            &mut editor,
            &mut shell,
            &mut sweep,
            &format!("{scale}-inspector-enum"),
            capture,
        );
        for index in 0..commands::menus::MENUS.len() {
            shell.open_bar(index, false);
            shell.rebuild(&mut editor).unwrap();
            walk_menu(
                &mut editor,
                &mut shell,
                &mut sweep,
                &format!("{scale}-menu-{index}"),
                0,
                capture,
            );
        }
        for (index, context) in [
            Context::Entity,
            Context::Hierarchy,
            Context::Viewport,
            Context::Asset,
            Context::Folder,
            Context::Assets,
            Context::Field,
            Context::Component,
            Context::Tab,
            Context::Node,
            Context::Canvas,
            Context::Console,
            Context::History,
            Context::Profiler,
            Context::Status,
            Context::Play,
            Context::Module,
            Context::Camera,
            Context::CameraSpeed,
            Context::Snapping,
            Context::ViewModes,
            Context::Show,
            Context::GameAspect,
            Context::HierarchyFilter,
        ]
        .into_iter()
        .enumerate()
        {
            let target = match context {
                Context::Entity => Target::Entity("e_quality1".into()),
                Context::Field => Target::Field("Light.kind".into()),
                Context::Component => Target::Component("Light".into()),
                Context::Tab => Target::Panel("inspector".into()),
                _ => Target::None,
            };
            shell.open_menu_at(
                Source::Context(context, target),
                [size[0] - 2., size[1] - 2.],
            );
            shell.rebuild(&mut editor).unwrap();
            walk_menu(
                &mut editor,
                &mut shell,
                &mut sweep,
                &format!("{scale}-context-{index}"),
                0,
                capture,
            );
        }
        shell.menu = None;
        for kind in ["move", "rotate", "scale", "fov"] {
            shell.open_menu_at(Source::Values(kind), [size[0] - 2., size[1] - 2.]);
            shell.rebuild(&mut editor).unwrap();
            sweep.context(
                &mut shell,
                &mut editor,
                &format!("{scale}-value-{kind}"),
                capture,
            );
        }
        shell.menu = None;
        shell.rebuild(&mut editor).unwrap();
        shell.toggle_add_component(&mut editor).unwrap();
        sweep.context(
            &mut shell,
            &mut editor,
            &format!("{scale}-add-component"),
            capture,
        );
        shell.add_component = None;
        shell
            .show_color_picker(&mut editor, "Light.color", "rgb", None, false)
            .unwrap();
        sweep.context(
            &mut shell,
            &mut editor,
            &format!("{scale}-color-picker"),
            capture,
        );
        shell.close_color_picker(&mut editor, false).unwrap();
        editor
            .action("editor.palette", json!({"open":true}))
            .unwrap();
        shell.rebuild(&mut editor).unwrap();
        sweep.context(
            &mut shell,
            &mut editor,
            &format!("{scale}-palette"),
            capture,
        );
        editor
            .action("editor.palette", json!({"open":false}))
            .unwrap();
        shell.rebuild(&mut editor).unwrap();
    }
    sweep
}

fn sweep_selects(
    editor: &mut Editor,
    shell: &mut Shell,
    sweep: &mut Sweep,
    name: &str,
    capture: &mut impl FnMut(&mut Shell, &str),
) {
    let controls: Vec<_> = shell
        .ui
        .nodes
        .iter()
        .enumerate()
        .filter(|(i, node)| {
            shell.ui.shown(*i)
                && node.element.tag == "dropdown"
                && node.element.attrs.get("class").is_some_and(|classes| {
                    classes
                        .split_whitespace()
                        .any(|class| class == "kit-select")
                })
        })
        .map(|(_, node)| node.element.id.clone())
        .collect();
    for id in controls {
        let Some(index) = shell.ui.find(&id) else {
            continue;
        };
        shell.ui.scroll_into_view(index).unwrap();
        shell.frame().unwrap();
        shell.click(&id, &mut Clip::default()).unwrap();
        shell.dispatch(editor).unwrap();
        assert!(
            shell.choice_open(),
            "{id} must open the actual shared select controller"
        );
        sweep.context(shell, editor, &format!("{name}-{id}"), capture);
        shell.key(editor, 41, true, &mut Clip::default()).unwrap();
        shell.key(editor, 41, false, &mut Clip::default()).unwrap();
        assert!(
            !shell.choice_open(),
            "Escape must close the actual select popup"
        );
    }
}

fn walk_menu(
    editor: &mut Editor,
    shell: &mut Shell,
    sweep: &mut Sweep,
    name: &str,
    level: usize,
    capture: &mut impl FnMut(&mut Shell, &str),
) {
    sweep.context(shell, editor, name, capture);
    let rows = shell
        .menu_levels(editor)
        .last()
        .cloned()
        .unwrap_or_default();
    for (index, row) in rows.into_iter().enumerate() {
        if !matches!(row, Row::Sub(..)) {
            continue;
        }
        if let Some(menu) = &mut shell.menu {
            menu.open.truncate(level);
            menu.open.push(index);
            menu.active = None;
        }
        shell.rebuild(editor).unwrap();
        walk_menu(
            editor,
            shell,
            sweep,
            &format!("{name}-{index}"),
            level + 1,
            capture,
        );
        if let Some(menu) = &mut shell.menu {
            menu.open.truncate(level);
        }
    }
}

#[test]
fn settings_select_commits_mouse_keyboard_and_numeric_text_through_typed_actions() {
    use ara_ui::input::InputEvent;
    use ara_ui_text::EditEvent;
    let project = Project::new();
    let (mut editor, mut shell) = project.open([1100., 760.]);
    show(&mut editor, &mut shell, "settings");
    editor
        .action(
            "editor.settings.view",
            json!({"scope":"user","category":"Keyboard shortcuts"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let mut clipboard = Clip::default();
    shell.click("keymap-presets", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(shell.choice_open());
    let expected = json!(commands::keys::Preset::ALL[1].name());
    for scan in [74, 81, 40] {
        shell.key(&mut editor, scan, true, &mut clipboard).unwrap();
        shell.key(&mut editor, scan, false, &mut clipboard).unwrap();
    }
    assert_eq!(editor.state.settings["input.keymap"], expected);
    shell.click("keymap-presets", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(shell.choice_open());
    let expected = json!(commands::keys::Preset::ALL[2].name());
    let option = shell
        .ui
        .nodes
        .iter()
        .find(|node| {
            node.element.id.starts_with("choice-keymap-presets-option-")
                && node
                    .text
                    .as_ref()
                    .is_some_and(|text| text.value == commands::keys::Preset::ALL[2].title())
        })
        .unwrap()
        .element
        .id
        .clone();
    let before_pointer = select_pointer_report(&shell, &option);
    let node = shell.ui.find(&option).unwrap();
    let rect = shell.ui.nodes[node].rect;
    let pointer = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    assert_eq!(
        shell
            .ui
            .hit_test(pointer[0], pointer[1])
            .map(|i| shell.ui.nodes[i].element.id.as_str()),
        Some(option.as_str()),
        "{before_pointer}"
    );
    shell
        .event(InputEvent::PointerDown(pointer), &mut clipboard)
        .unwrap();
    assert_eq!(
        shell.input.captured(),
        Some(ara_ui::source::stable_id(&option)),
        "before={before_pointer}\nafter={}",
        select_pointer_report(&shell, &option)
    );
    let after_press = select_pointer_report(&shell, &option);
    shell
        .event(InputEvent::PointerUp(pointer), &mut clipboard)
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(
        editor.state.settings["input.keymap"],
        expected,
        "before={before_pointer}\npressed={after_press}\nafter={}",
        select_pointer_report(&shell, &option)
    );
    let definition = crate::settings::DEFS
        .iter()
        .find(|d| {
            d.scope == crate::settings::Scope::User
                && matches!(d.kind, crate::settings::Kind::Number { .. })
        })
        .unwrap();
    editor
        .action(
            "editor.settings.view",
            json!({"scope":"user","category":definition.category}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let id = format!("set-{}", definition.key.replace('.', "-"));
    let index = shell.ui.find(&id).unwrap();
    shell.ui.scroll_into_view(index).unwrap();
    shell.frame().unwrap();
    let crate::settings::Kind::Number { min, max, step, .. } = definition.kind else {
        panic!("number definition")
    };
    let value = (min + 2. * step).min(max);
    shell.click(&id, &mut clipboard).unwrap();
    shell
        .event(InputEvent::Edit(EditEvent::SelectAll), &mut clipboard)
        .unwrap();
    shell
        .event(
            InputEvent::Edit(EditEvent::Insert(value.to_string())),
            &mut clipboard,
        )
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.key(&mut editor, 40, true, &mut clipboard).unwrap();
    shell.key(&mut editor, 40, false, &mut clipboard).unwrap();
    assert_eq!(editor.state.settings[definition.key], json!(value));
}

fn select_pointer_report(shell: &Shell, id: &str) -> Value {
    let target=shell.ui.find(id).map(|i|{
        let node=&shell.ui.nodes[i];
        let pointer=[node.rect.x+node.rect.w*0.5,node.rect.y+node.rect.h*0.5];
        json!({"rect":node.rect,"clip":node.clip,"shown":shell.ui.shown(i),"pointer":pointer,"hit":shell.ui.hit_test(pointer[0],pointer[1]).map(|i|shell.ui.nodes[i].element.id.as_str())})
    });
    json!({"target":id,"geometry":target,"capture":shell.input.captured().and_then(|id|shell.ui.by_id(id)).map(|i|shell.ui.nodes[i].element.id.as_str()),"focus":shell.input.focused().and_then(|id|shell.ui.by_id(id)).map(|i|shell.ui.nodes[i].element.id.as_str()),"scope":shell.input.focus_graph(&shell.ui),"overlays":shell.ui.transient_nodes().map(|i|json!({"id":shell.ui.nodes[i].element.id,"rect":shell.ui.nodes[i].rect,"clip":shell.ui.nodes[i].clip})).collect::<Vec<_>>()})
}
