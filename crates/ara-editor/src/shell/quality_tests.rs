//! Mounted-editor gates: shared control choices and every reachable transient surface.
#![allow(clippy::unwrap_used)]
mod captures;
mod fixture;
mod measure;

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
    shell.click(&option, &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(editor.state.settings["input.keymap"], expected);
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
