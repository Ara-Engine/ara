#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
//! Play starts in the background: the click returns at once and the Game view and
//! status bar follow the start's phase and progress, a start that fails, and Stop.
//! Its own test binary: it points `ARA_PLAYER` at a missing player.
use ara_editor::{Editor, shell::Shell, shell::commands::Target};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

fn text(shell: &Shell, id: &str) -> Option<String> {
    let i = shell.ui.find(id)?;
    shell.ui.nodes[i].text.as_ref().map(|t| t.value.clone())
}

#[test]
fn play_starts_in_the_background_and_shows_its_progress() {
    // SAFETY: the only test in this binary; set before anything reads the variable.
    unsafe { std::env::set_var("ARA_PLAYER", "missing-ara-player.exe") };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-editor/play-start")
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
    let write = |path: &str, value: Value| {
        std::fs::write(
            if path == "ara.project.json" {
                root.join(path)
            } else {
                root.join(ara_doc::AUTHORING_ROOT).join(path)
            },
            ara_json::to_canonical_string(&value),
        )
        .unwrap();
    };
    write(
        "ara.project.json",
        json!({"name":"Play start","layoutVersion":ara_host::project_layout::VERSION,"main":"levels/main.scene"}),
    );
    write(
        "levels/main.scene",
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{
            "e_crate001":{"name":"Crate","Transform":{},"MeshRenderer":{}}}}),
    );
    let mut editor = Editor::open(&root).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    // The click returns at once with a starting world, shown in the Game view.
    let clicked = Instant::now();
    shell
        .run_command(&mut editor, "play.start", &Target::None)
        .unwrap();
    assert!(
        clicked.elapsed() < Duration::from_millis(250),
        "{:?}",
        clicked.elapsed()
    );
    assert_eq!(editor.state.playing, Some(true));
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(
        text(&shell, "game-starting-title").as_deref(),
        Some("Starting Play…")
    );
    // A phase with a total shows a bar; the status bar says Play is starting.
    shell.show_play_row_for_test(
        &mut editor,
        json!({"status":"starting","progress":{"phase":"assets","completed":3,"total":12,"elapsedMs":2450}}),
    ).unwrap();
    assert_eq!(
        text(&shell, "game-starting-phase").as_deref(),
        Some("Preparing assets")
    );
    assert_eq!(
        text(&shell, "game-starting-count").as_deref(),
        Some("3 / 12")
    );
    assert_eq!(text(&shell, "game-starting-time").as_deref(), Some("2.4 s"));
    assert!(shell.ui.find("game-starting-fill").is_some());
    // Progress within the phase updates the screen in place, exactly as a rebuild
    // would write it; a phase without a total changes its shape and rebuilds.
    let rebuilds = shell.take_stats().rebuilds;
    assert!(shell.move_play_row_for_test(
        json!({"status":"starting","progress":{"phase":"assets","completed":5,"total":12,"elapsedMs":3150}})
    ));
    assert_eq!(shell.take_stats().rebuilds, 0, "{rebuilds} before");
    let patched = [
        "game-starting-phase",
        "game-starting-count",
        "game-starting-time",
    ]
    .map(|id| text(&shell, id));
    let fill = |shell: &Shell| {
        let i = shell.ui.find("game-starting-fill").unwrap();
        shell.ui.nodes[i].element.attrs.get("style").cloned()
    };
    let patched_fill = fill(&shell);
    assert_eq!(patched[1].as_deref(), Some("5 / 12"));
    assert_eq!(patched[2].as_deref(), Some("3.1 s"));
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(
        [
            "game-starting-phase",
            "game-starting-count",
            "game-starting-time"
        ]
        .map(|id| text(&shell, id)),
        patched
    );
    assert_eq!(fill(&shell), patched_fill);
    assert!(!shell.move_play_row_for_test(
        json!({"status":"starting","progress":{"phase":"world","elapsedMs":3300}})
    ));
    shell.show_play_row_for_test(
        &mut editor,
        json!({"status":"starting","progress":{"phase":"assets","completed":3,"total":12,"elapsedMs":2450}}),
    ).unwrap();
    assert!(shell.ui.find("game-stop").is_some());
    assert_eq!(
        text(&shell, "status-play-text").as_deref(),
        Some("Starting Play…")
    );
    shell.show_play_row_for_test(
        &mut editor,
        json!({"status":"starting","restart":true,"commandsRefreshing":true,"progress":{"phase":"pool","elapsedMs":3500}}),
    ).unwrap();
    for id in ["game-starting-title", "status-play-text"] {
        assert_eq!(
            text(&shell, id).as_deref(),
            Some("Restarting Play…"),
            "{id}; playing={:?}; panel error={:?}; game text={:?}",
            editor.state.playing,
            text(&shell, "panel-error-game-detail"),
            shell
                .ui
                .nodes
                .iter()
                .filter(|node| node.element.id.starts_with("game-"))
                .filter_map(|node| node
                    .text
                    .as_ref()
                    .map(|text| (&node.element.id, &text.value)))
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(
        text(&shell, "game-command-refresh").as_deref(),
        Some("Refreshing C# commands…")
    );
    assert!(shell.move_play_row_for_test(
        json!({"status":"starting","restart":true,"progress":{"phase":"pool","elapsedMs":3700}})
    ));
    assert_eq!(
        text(&shell, "game-starting-title").as_deref(),
        Some("Restarting Play…")
    );
    assert!(!shell.move_play_row_for_test(
        json!({"status":"starting","progress":{"phase":"pool","elapsedMs":3800}})
    ));
    shell.show_play_row_for_test(
        &mut editor,
        json!({"status":"starting","progress":{"phase":"assets","completed":3,"total":12,"elapsedMs":2450}}),
    ).unwrap();
    // Over a camera preview the start is a hint on the view Play starts with, with
    // the same progress updated in place; Play's first (lit) frame replaces it.
    shell.show_game_preview_for_test(&mut editor, [640, 360]);
    let image = shell.ui.find("game-image").unwrap();
    assert_eq!(
        shell.ui.nodes[image]
            .element
            .attrs
            .get("src")
            .map(String::as_str),
        Some("game-preview")
    );
    assert_eq!(
        text(&shell, "game-starting-phase").as_deref(),
        Some("Preparing assets")
    );
    assert!(shell.move_play_row_for_test(
        json!({"status":"starting","progress":{"phase":"assets","completed":5,"total":12,"elapsedMs":3150}})
    ));
    assert_eq!(
        text(&shell, "game-starting-count").as_deref(),
        Some("5 / 12")
    );
    // Without a player the start fails, and says why.
    let start = Instant::now();
    loop {
        editor.tick();
        shell.read_play_status(&mut editor);
        shell.rebuild(&mut editor).unwrap();
        if let Some(title) = text(&shell, "game-fault-title") {
            assert_eq!(title, "Play could not start");
            break;
        }
        assert!(start.elapsed().as_secs() < 30, "the start never failed");
        std::thread::sleep(Duration::from_millis(20));
    }
    let hint = text(&shell, "game-fault-hint").unwrap();
    assert!(hint.contains("player"), "{hint}");
    // Stop clears it: the Game view offers Play again.
    shell
        .run_command(&mut editor, "play.stop", &Target::None)
        .unwrap();
    editor.tick();
    shell.read_play_status(&mut editor);
    shell.rebuild(&mut editor).unwrap();
    assert!(editor.state.playing.is_none());
    assert!(shell.ui.find("game-empty").is_some());
    drop(editor);
    let _ = std::fs::remove_dir_all(&root);
}
