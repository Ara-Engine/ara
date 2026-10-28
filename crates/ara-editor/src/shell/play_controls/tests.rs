//! The mounted Play options remain reachable under the editor scroll reset.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_ui::input::InputEvent;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
mod lifecycle;

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("play-options-layout")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Play controls","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_playlayout","records":{"e_playlayout":{"name":"Play camera","Transform":{},"Camera":{}}}}),
            ),
        ] {
            std::fs::write(
                root.join(if path == "ara.project.json" {
                    std::path::PathBuf::from(path)
                } else {
                    std::path::PathBuf::from(ara_doc::AUTHORING_ROOT).join(path)
                }),
                ara_json::to_canonical_string(&value),
            )
            .unwrap();
        }
        Self(root)
    }
    fn open(&self) -> (Editor, Shell) {
        let mut editor = Editor::open(&self.0).unwrap();
        editor
            .action("editor.preferences", json!({"reduceMotion":true}))
            .unwrap();
        editor
            .action("editor.layout.show", json!({"panel":"game"}))
            .unwrap();
        editor
            .action(
                "editor.settings.set",
                json!({"key":"play.mode","value":"host"}),
            )
            .unwrap();
        editor
            .action(
                "editor.settings.set",
                json!({"key":"play.clients","value":2}),
            )
            .unwrap();
        let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
        shell.offscreen = true;
        (editor, shell)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(
            self.0.starts_with(
                std::path::absolute(crate::test_target())
                    .unwrap()
                    .join("play-options-layout")
            )
        );
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
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

fn unique_interactive_ids(shell: &Shell) {
    let mut seen = std::collections::BTreeSet::new();
    for (index, node) in shell.ui.nodes.iter().enumerate() {
        if shell.ui.shown(index)
            && matches!(
                node.element.tag.as_str(),
                "button" | "dropdown" | "toggle" | "field" | "input" | "slider"
            )
        {
            assert!(
                seen.insert(&node.element.id),
                "Duplicate interactive identity: {}",
                node.element.id
            );
        }
    }
}

fn surface(editor: &mut Editor, shell: &mut Shell, size: [f32; 2], floating: bool) {
    shell.leave();
    if floating {
        editor
            .action(
                "editor.layout.move",
                json!({"panel":"game","slot":"float","rect":[80,80,640,360]}),
            )
            .unwrap();
        shell.rebuild(editor).unwrap();
        let window = editor.state.floating.iter().find(|window| matches!(&window.root, ara_ui::docking::DockNode::Area {tabs,..} if tabs.iter().any(|panel|panel=="game"))).unwrap().id.clone();
        shell.resize_surface(&window, size).unwrap();
        assert!(shell.enter(&window));
    } else {
        shell.ui.resize(size).unwrap();
    }
    shell.rebuild(editor).unwrap();
    shell.frame().unwrap();
}

fn reveal(shell: &mut Shell, id: &str) {
    for _ in 0..40 {
        let node = &shell.ui.nodes[shell
            .shown_node(id)
            .unwrap_or_else(|| panic!("Play control {id} is missing"))];
        let visible = node.rect.intersection(node.clip);
        if node.rect.w > 1.
            && node.rect.h > 1.
            && visible.w + 0.5 >= node.rect.w
            && visible.h + 0.5 >= node.rect.h
        {
            return;
        }
        let target = node.rect;
        let mut scrolls = Vec::new();
        let mut parent = node.element.parent;
        while let Some(index) = parent {
            if shell.ui.nodes[index].element.tag == "scroll" {
                scrolls.push(index);
            }
            parent = shell.ui.nodes[index].element.parent;
        }
        let scroll = scrolls
            .iter()
            .copied()
            .find(|index| {
                let rect = shell.ui.nodes[*index].rect;
                target.y < rect.y - 0.5 || target.y + target.h > rect.y + rect.h + 0.5
            })
            .or_else(|| scrolls.last().copied())
            .expect("A clipped Play control must have a scroll ancestor");
        let rail = &shell.ui.nodes[scroll];
        let viewport = rail.rect.intersection(rail.clip);
        assert!(
            viewport.w > 1. && viewport.h > 1.,
            "Play options have no usable scroll viewport: {:?}",
            rail.rect
        );
        let delta = if target.y < viewport.y {
            target.y - viewport.y
        } else {
            target.y + target.h - viewport.y - viewport.h
        }
        .clamp(-32., 32.);
        // The parent's gutter remains a real pointer target beside a nested
        // scroll; wheel there to move its viewport rather than its child.
        shell
            .event(
                InputEvent::Wheel {
                    position: [viewport.x + viewport.w - 2., viewport.y + viewport.h * 0.5],
                    delta: [0., delta],
                },
                &mut Clipboard,
            )
            .unwrap();
        shell.frame().unwrap();
    }
    let node = &shell.ui.nodes[shell.shown_node(id).unwrap()];
    panic!(
        "Play control {id} did not become fully reachable: rect {:?}, clip {:?}",
        node.rect, node.clip
    );
}

#[test]
fn runtime_keep_field_rail_and_actions_remain_scroll_reachable() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    // Seed only a cached diff presentation. Native J6 supplies the real player
    // and exercises Apply; this test isolates its nested retained scroll layout.
    let fields = [
        "Camera.fov",
        "Camera.near",
        "Camera.far",
        "Camera.priority",
        "Transform.position",
        "Transform.rotation",
        "Transform.scale",
    ];
    let rows: Vec<_> = fields.iter().map(|path| json!({"entity":"e_playlayout","path":path,"eligible":true,"driven":false,"playStart":1,"play":2,"edit":1,"status":"changed","reason":null})).collect();
    editor.play_world = Some("pw_playlayout".into());
    editor
        .action("editor.view", json!({"play":"paused"}))
        .unwrap();
    shell.play_controls.keep = Some(
        keep::Choice::read(
            "pw_playlayout".into(),
            vec!["e_playlayout".into()],
            false,
            &json!({"keepCandidates":rows}),
        )
        .unwrap(),
    );
    shell.play_controls.options = true;
    let revision = editor.host.store().revision();
    for (size, floating) in [
        ([1440., 900.], false),
        ([1000., 480.], false),
        ([640., 360.], true),
    ] {
        surface(&mut editor, &mut shell, size, floating);
        let inner = &shell.ui.nodes[shell.shown_node("keep-fields-scroll").unwrap()];
        assert!(
            (1. ..=280.5).contains(&inner.rect.h),
            "Keep fields collapsed under the editor scroll reset: {:?}",
            inner.rect
        );
        for path in fields {
            let id = format!(
                "keep-field-{:016x}",
                ara_ui::source::stable_id(&format!("e_playlayout#{path}"))
            );
            reveal(&mut shell, &id);
        }
        for id in [
            "keep-fields-refresh",
            "keep-fields-cancel",
            "keep-fields-apply",
        ] {
            reveal(&mut shell, id);
        }
        unique_interactive_ids(&shell);
    }
    shell.leave();
    assert_eq!(
        editor.host.store().revision(),
        revision,
        "Browsing cached Keep choices must not author content"
    );
}

fn choose(shell: &mut Shell, editor: &mut Editor, id: &str, option: usize) {
    reveal(shell, id);
    shell.click(id, &mut Clipboard).unwrap();
    shell.dispatch(editor).unwrap();
    assert!(shell.choice_open(), "Native pointer did not open {id}");
    for key in std::iter::once(74)
        .chain(std::iter::repeat_n(81, option))
        .chain(std::iter::once(40))
    {
        shell.key(editor, key, true, &mut Clipboard).unwrap();
        shell.key(editor, key, false, &mut Clipboard).unwrap();
    }
    shell.frame().unwrap();
    assert!(!shell.choice_open());
}

#[test]
fn play_options_have_unique_controls_and_scroll_reachability_in_main_short_and_floating_views() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    shell.frame().unwrap();
    shell.click("game-options", &mut Clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.frame().unwrap();
    assert!(shell.play_controls.options);
    assert_eq!(
        shell.ui.nodes[shell.shown_node("play-mode").unwrap()]
            .element
            .tag,
        "button"
    );
    assert_eq!(
        shell.ui.nodes[shell.shown_node("play-options-mode").unwrap()]
            .element
            .tag,
        "dropdown"
    );

    for (case, (size, floating)) in [
        ([1440., 900.], false),
        ([1000., 480.], false),
        ([640., 360.], true),
    ]
    .into_iter()
    .enumerate()
    {
        surface(&mut editor, &mut shell, size, floating);
        unique_interactive_ids(&shell);
        let rail = &shell.ui.nodes[shell.shown_node("play-options").unwrap()];
        assert!(
            (1. ..=260.5).contains(&rail.rect.h),
            "Play options are collapsed or unbounded: {:?}",
            rail.rect
        );
        for id in [
            "play-options-mode",
            "play-process-mode",
            "play-client-count",
            "play-start-paused",
            "play-network-1",
            "play-network-2",
        ] {
            reveal(&mut shell, id);
        }
        choose(
            &mut shell,
            &mut editor,
            "play-options-mode",
            if case == 1 { 1 } else { 2 },
        );
        assert_eq!(
            editor.state.settings["play.mode"],
            if case == 1 { "host" } else { "dedicated" }
        );
        choose(
            &mut shell,
            &mut editor,
            "play-process-mode",
            usize::from(case != 1),
        );
        assert_eq!(
            editor.state.settings["play.processMode"],
            if case == 1 { "separate" } else { "shared" }
        );
        unique_interactive_ids(&shell);
    }
    shell.leave();
}
