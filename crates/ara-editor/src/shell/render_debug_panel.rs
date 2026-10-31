//! Rendering Debug panel: switches that turn one effect off at a time (ambient
//! occlusion, shadows, temporal jitter) or Play's upscaling, to trace an artifact
//! such as flicker to its source. Each switch runs the same `view.debug.*` command
//! as View > Rendering Debug, so the Scene view, a running Play and the next Play
//! start all follow it.
use super::*;
use commands::Target;

/// One switch: its command, title, what it shows when changed, and its current state.
struct Switch {
    command: &'static str,
    title: &'static str,
    help: &'static str,
    on: fn(&ara_render::RenderDebug) -> bool,
}

const SWITCHES: [Switch; 6] = [
    Switch {
        command: "view.debug.ambient_occlusion",
        title: "Ambient occlusion",
        help: "Contact darkening in creases and corners.",
        on: |d| d.ambient_occlusion,
    },
    Switch {
        command: "view.debug.shadows",
        title: "Shadows",
        help: "Sun and local-light shadows at every distance.",
        on: |d| d.shadows,
    },
    Switch {
        command: "view.debug.temporal_jitter",
        title: "Temporal anti-aliasing jitter",
        help: "Off, edges go jagged but hold still: flicker that remains comes from something else.",
        on: |d| d.temporal_jitter,
    },
    Switch {
        command: "view.debug.reflections",
        title: "Reflections only",
        help: "Only the reflected light, exposed like the lit image.",
        on: |d| d.reflections == ara_render::ReflectionView::Reflections,
    },
    Switch {
        command: "view.debug.reflection_sources",
        title: "Reflection sources",
        help: "Where each reflection comes from: screen green, capture trace blue, box-projected captures cyan, probes orange, sky magenta.",
        on: |d| d.reflections == ara_render::ReflectionView::Sources,
    },
    Switch {
        command: "view.debug.full_resolution",
        title: "Full resolution in Play",
        help: "Render Play at display resolution instead of upscaling. The Scene view is always full resolution.",
        on: |d| d.full_resolution,
    },
];

impl Shell {
    pub(super) fn render_debug_panel(&mut self, editor: &Editor, out: &mut String) {
        let debug = editor.state.render_debug;
        out.push_str("<scroll id='render-debug' class='lighting-scroll'>");
        let applies = if editor.play_world.is_some() {
            "Changes apply to the Scene view and the running Play at once."
        } else {
            "Changes apply to the Scene view at once and to Play when it starts. Judge flicker in Play or while moving the camera: the Scene view stops drawing once its image settles."
        };
        out.push_str(&format!(
            "<text id='render-debug-applies' class='lighting-help' text='{}' />",
            escape(applies)
        ));
        for switch in &SWITCHES {
            let name = switch.command.trim_start_matches("view.debug.");
            let id = format!("render-debug-{name}");
            self.bind_command(&id, switch.command, Target::None);
            out.push_str(&format!(
                "<panel id='{id}-row' class='lighting-card render-debug-row'><toggle id='{id}' checked='{}' label='{}' description='{}' action='Dispatch' /><panel id='{id}-text' class='agent-text'><text id='{id}-title' class='lighting-title' text='{}' /><text id='{id}-help' class='lighting-help' text='{}' /></panel></panel>",
                (switch.on)(&debug),
                escape(switch.title),
                escape(switch.help),
                escape(switch.title),
                escape(switch.help),
            ));
        }
        if debug != ara_render::RenderDebug::default() {
            self.bind_command("render-debug-reset", "view.debug.reset", Target::None);
            out.push_str("<button id='render-debug-reset' class='lighting-button' text='Turn everything back on' description='Restore every rendering effect' action='Dispatch' />");
        }
        out.push_str("</scroll>");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    struct Clip;
    impl ara_ui_text::Clipboard for Clip {
        fn read(&mut self) -> ara_ui_text::Result<String> {
            Ok(String::new())
        }
        fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn switches_survive_view_updates_and_the_panel_flips_them() {
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("render-debug")
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Render debug","main":"levels/main.scene"}),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
            ara_json::to_canonical_string(
                &json!({"$schema":"ara://schema/scene/1","id":"sc_renderdebug","records":{}}),
            ),
        )
        .unwrap();
        let mut editor = Editor::open(&root).unwrap();
        let mut shell = Shell::new(&mut editor, [1100., 760.]).unwrap();
        shell.offscreen = true;
        shell
            .run_command(&mut editor, "view.debug.shadows", &Target::None)
            .unwrap();
        assert!(!editor.state.render_debug.shadows);
        // Every view change (entering Play is one) delivers a new navigation state;
        // the session-only switches used to come back on here.
        editor
            .action("editor.view", json!({"viewMode":"normals"}))
            .unwrap();
        editor.sync();
        assert_eq!(editor.state.view_mode, "normals");
        assert!(!editor.state.render_debug.shadows);
        shell
            .run_command(&mut editor, "window.render_debug", &Target::None)
            .unwrap();
        editor.sync();
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        assert!(shell.shown_node("render-debug-reset").is_some());
        shell.click("render-debug-shadows", &mut Clip).unwrap();
        shell.dispatch(&mut editor).unwrap();
        assert!(editor.state.render_debug.shadows);
        drop(shell);
        drop(editor);
        let _ = std::fs::remove_dir_all(&root);
    }
}
