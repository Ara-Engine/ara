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
    std::fs::create_dir_all(root.join("levels")).unwrap();
    let write = |path: &str, value: Value| {
        std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
    };
    write(
        "ara.project.json",
        json!({"name":"Play start","main":"levels/main.scene"}),
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
    );
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
    assert!(shell.ui.find("game-stop").is_some());
    assert_eq!(
        text(&shell, "status-play-text").as_deref(),
        Some("Starting Play…")
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
