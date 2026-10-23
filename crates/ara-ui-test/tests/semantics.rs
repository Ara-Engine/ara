use ara_schema::SchemaDb;
use ara_ui::input::{Direction, InputEvent};
use ara_ui_stories::{InputMode, Story, catalogue};
use ara_ui_test::{Harness, Key, Query, Result};
use ara_ui_text::EditEvent;
use serde_json::json;

fn example(markup: &str) -> Story {
    let mut story = catalogue().remove(0);
    story.markup = markup.into();
    story.stylesheet = String::new();
    story.transitions.clear();
    story.steps.clear();
    story
}
fn mount(story: &Story, variant: usize) -> Result<Harness> {
    let mut db = SchemaDb::new();
    ara_ui_stories::register(&mut db)?;
    Harness::mount(story, &story.views[variant], &db)
}

#[test]
fn localized_queries_are_unique_visible_and_accessible() -> Result<()> {
    let story = example(
        "<ui id='root'><button id='a' test-id='resume' text='loc:resume' label='loc:resume'/><button id='b' text='loc:resume'/><button id='hidden' text='loc:quit' style='translate-x:900px;'/></ui>",
    );
    let harness = mount(&story, 2)?;
    assert_eq!(harness.query(&Query::test_id("resume"))?, "a");
    assert_eq!(harness.query_all(&Query::label("loc:resume"))?.len(), 2);
    assert_eq!(harness.query_all(&Query::text("استئناف"))?.len(), 2);
    let error = harness
        .query(&Query::role("button", Some("loc:resume")))
        .expect_err("ambiguous")
        .to_string();
    assert!(error.contains("found 2") && error.contains("#a") && error.contains("#hidden"));
    assert!(harness.query(&Query::text("loc:quit")).is_err());
    Ok(())
}

#[test]
fn pointer_capture_keyboard_and_gamepad_use_native_focus_and_actions() -> Result<()> {
    let mut story = example(
        "<ui id='root'><button id='a' text='loc:resume' action='game.resume'/><button id='b' text='loc:quit' action='game.quit'/><button id='disabled' text='Disabled' disabled='true' action='bad'/></ui>",
    );
    let mut harness = mount(&story, 0)?;
    let i = harness.ui().find("a").ok_or("a")?;
    let r = harness.ui().nodes[i].rect;
    let p = [r.x + r.w / 2.0, r.y + r.h / 2.0];
    harness.event(InputEvent::PointerMove(p))?;
    harness.event(InputEvent::PointerDown(p))?;
    let state = harness.ui().nodes[i].state;
    assert!(state.hover && state.active && state.focus);
    harness.event(InputEvent::Cancel)?;
    assert!(!harness.ui().nodes[i].state.active);
    assert!(harness.actions().is_empty());
    harness.activate(&Query::test_id("a"))?;
    harness.expect_action("game.resume", Some(&json!(null)))?;
    harness.key(Key::Tab { reverse: false })?;
    harness.expect_focused(&Query::test_id("b"))?;
    harness.gamepad(Direction::Up)?;
    harness.expect_focused(&Query::test_id("a"))?;
    harness.key(Key::Confirm)?;
    assert_eq!(harness.actions().len(), 2);
    assert!(harness.focus(&Query::test_id("disabled")).is_err());
    story.views[0].input = InputMode::Gamepad;
    let mut gamepad = mount(&story, 0)?;
    gamepad.activate(&Query::test_id("b"))?;
    gamepad.expect_action("game.quit", None)?;
    Ok(())
}

#[test]
fn ime_preedit_is_visible_without_mutating_committed_bound_values() -> Result<()> {
    let story = example(
        "<ui id='root'><field id='field' test-id='name' text='' bind='text' label='loc:name' action='game.name'/></ui>",
    );
    let mut harness = mount(&story, 2)?;
    let field = Query::label("loc:name");
    harness.focus(&field)?;
    harness.event(InputEvent::Edit(EditEvent::SelectAll))?;
    harness.event(InputEvent::Edit(EditEvent::ImeStart))?;
    harness.event(InputEvent::Edit(EditEvent::ImePreedit {
        text: "سلام".into(),
        selection: 0.."سلام".len(),
    }))?;
    harness.expect_text(&field, "سلام")?;
    harness.expect_value(&field, &json!("Outpost"))?;
    harness.expect_bound("text", &json!("Outpost"))?;
    assert!(harness.actions().is_empty());
    harness.event(InputEvent::Edit(EditEvent::ImeCommit("سلام".into())))?;
    harness.expect_bound("text", &json!("سلام"))?;
    harness.expect_action("game.name", Some(&json!("سلام")))?;
    harness.event(InputEvent::Edit(EditEvent::Undo))?;
    harness.expect_bound("text", &json!("Outpost"))?;
    Ok(())
}

#[test]
fn deterministic_time_sampling_and_rtl_repaints_keep_geometry_stable() -> Result<()> {
    let story = catalogue()
        .into_iter()
        .find(|s| s.component == "button")
        .ok_or("button")?;
    let mut first = mount(&story, 0)?;
    let x0 = first.ui().nodes[first.ui().find("story-title").ok_or("title")?]
        .rect
        .x;
    first.advance(100)?;
    let x1 = first.ui().nodes[first.ui().find("story-title").ok_or("title")?]
        .rect
        .x;
    assert_eq!(x1, x0 + 2.0);
    first.advance(100)?;
    let mut second = mount(&story, 0)?;
    second.advance(200)?;
    assert_eq!(first.capture()?.png, second.capture()?.png);
    let mut rtl = mount(&story, 2)?;
    let i = rtl.ui().find("sample").ok_or("sample")?;
    let x = rtl.ui().nodes[i].rect.x;
    rtl.activate(&Query::role("button", Some("loc:resume")))?;
    rtl.expect_action("game.resume", None)?;
    rtl.capture()?;
    assert_eq!(rtl.ui().nodes[i].rect.x, x);
    Ok(())
}

#[test]
fn rtl_mirrors_rows_without_reversing_reading_order_or_pointer_targets() -> Result<()> {
    let story = example(
        "<ui id='root'><panel id='row' style='flex-direction:row;'><button id='left' text='loc:resume' action='game.resume' style='width:120px;'/><button id='right' text='loc:quit' style='width:100px;'/></panel></ui>",
    );
    let ltr = mount(&story, 0)?;
    let mut rtl = mount(&story, 3)?;
    for id in ["left", "right"] {
        let a = ltr.ui().nodes[ltr.ui().find(id).ok_or("id")?].rect;
        let b = rtl.ui().nodes[rtl.ui().find(id).ok_or("id")?].rect;
        assert_eq!(b.x, ltr.ui().size[0] - a.x - a.w);
    }
    rtl.activate(&Query::test_id("left"))?;
    rtl.expect_action("game.resume", None)?;
    rtl.key(Key::Tab { reverse: false })?;
    rtl.expect_focused(&Query::test_id("right"))?;
    Ok(())
}

#[test]
fn modal_scope_reports_only_reachable_controls_and_restores_focus() -> Result<()> {
    let story = example(
        "<ui id='root'><button id='launcher' text='loc:settings'/><modal id='modal'><button id='close' text='loc:resume'/></modal></ui>",
    );
    let mut harness = mount(&story, 0)?;
    harness.focus(&Query::test_id("launcher"))?;
    harness.push_scope(
        &Query::role("dialog", None),
        Some(&Query::test_id("close")),
        true,
    )?;
    harness.expect_focused(&Query::test_id("close"))?;
    assert!(harness.focus(&Query::test_id("launcher")).is_err());
    let capture = harness.capture()?;
    assert_eq!(capture.report.focus_graph.len(), 1);
    assert_eq!(capture.report.focus_graph[0].tab.as_deref(), Some("close"));
    harness.key(Key::Escape)?;
    harness.expect_focused(&Query::test_id("launcher"))?;
    Ok(())
}
