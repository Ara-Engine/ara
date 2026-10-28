//! Pure kit-modal presentation shared by native input and offscreen layout checks.
use super::State;
use ara_ui::source::escape;
use std::fmt::Write as _;

pub(super) const CSS: &str = include_str!("open_project.css");

#[derive(Clone, Copy)]
pub(super) enum Action {
    Cancel,
    Migrate,
    Retry,
}

pub(super) fn markup(project: &str, state: &State) -> String {
    let class = if matches!(state, State::Ready(_)) {
        "card card-ready"
    } else {
        "card"
    };
    let mut out = format!(
        "<ui id='project-open'><modal id='project-migration' class='layer' label='Project layout migration'><scroll id='migration-frame' class='frame'><panel id='migration-card' class='{class}'>"
    );
    text(
        &mut out,
        "title",
        &format!("Update {project}'s folder layout"),
    );
    match state {
        State::Checking => {
            text(&mut out, "copy", "Checking what needs to move…");
            text(
                &mut out,
                "copy muted",
                "This check only reads the project. Nothing moves until you choose Migrate.",
            );
        }
        State::Ready(preview) => {
            out.push_str("<scroll id='migration-details-scroll' class='details-scroll'><panel id='migration-details' class='details'>");
            text(
                &mut out,
                "copy",
                "This project uses an older folder layout. Migrate it before opening in this preview. Asset IDs and references are preserved, and the migration is recorded as one undoable change.",
            );
            text(&mut out, "label", "WHAT MOVES");
            out.push_str("<scroll id='migration-moves-scroll' class='moves-scroll'><panel id='migration-moves' class='moves'>");
            for operation in &preview.operations {
                if operation["op"] == "move" {
                    let from = operation["from"].as_str().unwrap_or_default();
                    let to = operation["to"].as_str().unwrap_or_default();
                    text(&mut out, "copy mono", &format!("{from} → {to}"));
                }
            }
            out.push_str("</panel></scroll><panel id='migration-preview' class='preview'>");
            text(&mut out, "label", "AFTER MIGRATION");
            out.push_str("<scroll id='migration-tree-scroll' class='tree-scroll'><panel id='migration-tree' class='tree'>");
            for path in preview.preview_tree.iter().take(160) {
                text(&mut out, "tree-line", path);
            }
            if preview.preview_tree.len() > 160 {
                text(
                    &mut out,
                    "copy muted",
                    &format!("…and {} more paths", preview.preview_tree.len() - 160),
                );
            }
            out.push_str("</panel></scroll></panel>");
            text(
                &mut out,
                "copy muted",
                &format!(
                    "Authored files live in {}/. Cancel keeps the project closed and leaves its files unchanged.",
                    preview.authored_root.trim_end_matches('/')
                ),
            );
            out.push_str("</panel></scroll>");
        }
        State::Working {
            progress,
            phase,
            cancellable,
        } => {
            text(&mut out, "copy", phase);
            let progress = if progress.is_finite() {
                progress.clamp(0., 1.)
            } else {
                0.
            };
            let _ = write!(
                out,
                "<panel id='migration-track' class='track'><panel id='migration-progress' class='progress' style='width:{}%' /></panel>",
                progress * 100.
            );
            text(
                &mut out,
                "copy muted",
                if *cancellable {
                    "The project opens once its files and history have finished saving. Cancel is available before the commit begins."
                } else {
                    "Finishing the project files and history. The project will open when everything has been saved."
                },
            );
        }
        State::Error(diagnostics) => {
            out.push_str("<scroll id='migration-errors' class='errors'><panel id='migration-error-lines' class='moves'>");
            for diagnostic in diagnostics {
                text(&mut out, "copy", &diagnostic.message);
                text(&mut out, "copy muted", &diagnostic.code.to_string());
            }
            out.push_str("</panel></scroll>");
        }
    }
    out.push_str("<panel id='migration-actions' class='actions'>");
    button(
        &mut out,
        "migration-cancel",
        "Cancel",
        "",
        state.cancellable(),
    );
    match state {
        State::Ready(_) => button(&mut out, "migration-submit", "Migrate", "primary", true),
        State::Error(_) => button(&mut out, "migration-retry", "Check again", "primary", true),
        _ => {}
    }
    out.push_str("</panel></panel></scroll></modal></ui>");
    out
}

fn text(out: &mut String, class: &str, value: &str) {
    let id = out.len();
    let _ = write!(
        out,
        "<text id='migration-text-{id}' class='{class}' text='{}' />",
        escape(value, b'\'')
    );
}
fn button(out: &mut String, id: &str, label: &str, class: &str, enabled: bool) {
    let _ = write!(
        out,
        "<button id='{id}' class='{class}' label='{label}' action='Dispatch' {}><text id='{id}-label' text='{label}' /></button>",
        if enabled { "" } else { "disabled='true'" }
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ui::{
        Ui,
        input::{InputEvent, InputState},
        layers::{LayerKind, LayerStack},
        source::stable_id,
        theme::EditorTheme,
    };
    use serde_json::json;

    fn preview() -> State {
        State::Ready(serde_json::from_value(json!({"needsMigration":true,"authoredRoot":"content","planToken":"readonly-test-token","operations":[{"op":"move","from":"game/levels/","to":"content/levels/"},{"op":"move","from":"game/scripts/","to":"content/scripts/"}],"previewTree":(0..120).map(|index|format!("content/scenes/area-{index}/scene.scene")).collect::<Vec<_>>()})).expect("preview fixture"))
    }

    #[test]
    fn preview_keeps_actions_visible_and_enter_defaults_to_cancel_in_both_themes()
    -> ara_ui::Result<()> {
        let state = preview();
        for theme in [EditorTheme::Dark, EditorTheme::Light] {
            let mut ui = Ui::editor(
                &markup(
                    "A long project title with several authored scene collections",
                    &state,
                ),
                CSS,
                [648., 640.],
            )?;
            ui.switch_theme(theme)?;
            let mut text = crate::shell::text_system()?;
            ui.frame(&mut text)?;
            for id in ["migration-cancel", "migration-submit"] {
                let node = &ui.nodes[ui.find(id).expect(id)];
                assert!(node.rect.intersects(node.clip));
                assert!(
                    node.rect.x >= 0.
                        && node.rect.y >= 0.
                        && node.rect.x + node.rect.w <= 648.5
                        && node.rect.y + node.rect.h <= 640.5,
                    "{id} must stay bounded: {:?}",
                    node.rect
                );
            }
            let mut input = InputState::default();
            let mut layers = LayerStack::default();
            layers.open(
                &mut ui,
                &mut input,
                stable_id("project-migration"),
                LayerKind::Modal,
                [0., 0.],
                Some(stable_id("migration-cancel")),
            )?;
            layers.event(&mut ui, &mut input, InputEvent::Activate, &mut Clipboard)?;
            assert_eq!(
                ui.drain_actions()
                    .map(|action| action.node)
                    .collect::<Vec<_>>(),
                [stable_id("migration-cancel")]
            );
            layers.event(
                &mut ui,
                &mut input,
                InputEvent::Tab { reverse: false },
                &mut Clipboard,
            )?;
            layers.event(&mut ui, &mut input, InputEvent::Activate, &mut Clipboard)?;
            assert_eq!(
                ui.drain_actions()
                    .map(|action| action.node)
                    .collect::<Vec<_>>(),
                [stable_id("migration-submit")]
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires the UI Vulkan adapter; writes hidden offscreen captures only"]
    fn capture_open_migration_dialog_without_showing_a_window() -> ara_ui::Result<()> {
        let base = std::env::var_os("ARA_TEST_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    std::path::PathBuf::from("D:/AraBuild/ux-hub-evidence")
                } else {
                    std::env::temp_dir()
                }
            });
        let directory = base.join("editor-migration-ui");
        std::fs::create_dir_all(&directory)?;
        super::super::window::DialogWindow::capture("Outpost", &preview(), &directory, "preview")?;
        super::super::window::DialogWindow::capture(
            "Outpost",
            &State::Working {
                progress: 0.85,
                phase: "Saving project files and history".into(),
                cancellable: false,
            },
            &directory,
            "committing",
        )?;
        Ok(())
    }

    struct Clipboard;
    impl ara_ui_text::Clipboard for Clipboard {
        fn read(&mut self) -> ara_ui_text::Result<String> {
            Ok(String::new())
        }
        fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
            Ok(())
        }
    }
}
