//! Every ara-kit component in both schemes, at 2x, with real hover, press and focus states
//! driven through the runtime: `target/ara-kit/gallery-{dark,light}.png`.
//! `cargo run -p ara-kit --example gallery`.
use std::time::Duration;

use ara_kit::icons;
use ara_kit::look::Look;
use ara_kit::ui::*;
use stillkit::el::{Align, El, Runtime, col, el, row, text};

struct S {
    look: Look,
}

fn view(s: &S) -> El<S> {
    let l = &s.look;
    let c = &l.c;
    let group = |title: &str, items: Vec<El<S>>| {
        col()
            .gap(10.0)
            .child(label(l, title))
            .child(row().gap(10.0).align(Align::Center).children(items))
    };
    col()
        .w_full()
        .h_full()
        .fill(c.background.0)
        .child(title_bar(l, 0.0, vec![text("Ara Hub").text_size(12.5).weight(600).color(c.foreground.0)], vec![], vec![icon_button(l, Tone::Ghost, Size::Sm, icons::SUN, "Theme")], Some(false)))
        .child(
            col().pad(24.0).gap(22.0)
                .child(page_header(l, "Components", Some("Every ara-kit part, at rest and in its states"), vec![button(l, Tone::Secondary, Size::Md, Some(icons::FOLDER_OPEN), "Open"), button(l, Tone::Primary, Size::Md, Some(icons::PLUS), "New project")]))
                .child(group("Buttons", vec![
                    button(l, Tone::Primary, Size::Md, Some(icons::PLAY), "Primary").key("b-hover").on_press(|_| {}),
                    button(l, Tone::Secondary, Size::Md, None, "Secondary").key("b-press").on_press(|_| {}),
                    button(l, Tone::Ghost, Size::Md, Some(icons::REFRESH_CW), "Ghost").key("b-ghost").on_press(|_| {}),
                    button(l, Tone::Danger, Size::Md, Some(icons::TRASH), "Delete").on_press(|_| {}),
                    enabled(button(l, Tone::Primary, Size::Md, None, "Disabled").on_press(|_| {}), false),
                    button(l, Tone::Secondary, Size::Sm, None, "Small").on_press(|_| {}),
                    button(l, Tone::Primary, Size::Lg, Some(icons::ROCKET), "Large").on_press(|_| {}),
                    icon_button(l, Tone::Ghost, Size::Md, icons::ELLIPSIS, "More").on_press(|_| {}),
                    icon_button(l, Tone::Secondary, Size::Md, icons::SETTINGS, "Settings").on_press(|_| {}),
                    row().gap(2.0).child(toggle_button(l, Size::Md, icons::LAYOUT_GRID, "Grid", true).on_press(|_| {})).child(toggle_button(l, Size::Md, icons::LIST, "List", false).on_press(|_| {})),
                ]))
                .child(group("Fields", vec![
                    field(l, Size::Md, "name", Some(icons::USER), "Project name", "Night Market", |f| f).w(220.0),
                    field(l, Size::Md, "empty", None, "Placeholder", "", |f| f).w(180.0),
                    search(l, "search", "Search projects", "", Some("/"), |f| f, |_| {}).w(220.0),
                    search(l, "search2", "Search", "outpost", None, |f| f, |_| {}).w(200.0),
                    select(l, Size::Md, "select", "0.0.2-alpha").w(150.0).on_press(|_| {}),
                ]))
                .child(group("Choices", vec![
                    segmented(l, &[("s1", None, "Last opened"), ("s2", None, "Name")], 0, |_| |_: &mut S| {}),
                    segmented(l, &[("v1", Some(icons::LAYOUT_GRID), "Grid"), ("v2", Some(icons::LIST), "List")], 1, |_| |_: &mut S| {}),
                    switch(l, "sw1", "On", true).on_press(|_| {}),
                    switch(l, "sw2", "Off", false).on_press(|_| {}),
                    checkbox(l, "cb1", "Checked", true).on_press(|_| {}),
                    checkbox(l, "cb2", "Unchecked", false).on_press(|_| {}),
                ]))
                .child(group("Status", vec![
                    chip(l, Status::Success, "Open in editor", true),
                    chip(l, Status::Accent, "Running headless", true),
                    chip(l, Status::Neutral, "0.0.2-alpha", false),
                    chip(l, Status::Warning, "Update", false),
                    chip(l, Status::Danger, "Missing engine", true),
                    count(l, 5, false),
                    count(l, 2, true),
                    dot(l, Status::Success),
                    kbd(l, "⌘K"),
                    kbd(l, "Ctrl+Shift+P"),
                    spinner(l, 16.0),
                    avatar(l, "Night Market", 3, 32.0),
                    avatar(l, "Courier Run", 2, 32.0),
                    el().w(160.0).child(progress(l, 0.62, Status::Accent)),
                ]))
                .child(row().gap(16.0).align(stillkit::el::Align::Start)
                    .child(menu_panel(l, vec![
                        menu_heading(l, "Project"),
                        menu_item(l, "m1", Some(icons::PLAY), "Open in editor", Some("↩"), false).on_press(|_| {}),
                        menu_item(l, "m2", Some(icons::SERVER), "Open headless", None, false).on_press(|_| {}),
                        menu_item(l, "m3", Some(icons::PENCIL), "Rename", Some("F2"), false).on_press(|_| {}),
                        menu_check(l, "m4", "Pin to top", true).on_press(|_| {}),
                        menu_separator(l),
                        menu_item(l, "m5", Some(icons::TRASH), "Remove from list", Some("⌫"), true).on_press(|_| {}),
                    ]))
                    .child(dialog_panel(l, Some((Status::Danger, icons::TRIANGLE_ALERT)), "Uninstall 0.0.1-alpha?", dialog_text(l, "Two projects pin this engine. They will ask for an installed engine the next time they open. Projects stay on disk."), vec![secondary(l, "Cancel").on_press(|_| {}), button(l, Tone::Danger, Size::Md, None, "Uninstall").on_press(|_| {})], |_| {}))
                    .child(col().gap(10.0)
                        .child(toast(l, "t1", Status::Success, "Created Night Market", Some(button(l, Tone::Ghost, Size::Sm, None, "Open").on_press(|_| {})), |_| {}))
                        .child(toast(l, "t2", Status::Neutral, "Removed Prototype 7 from the list", Some(button(l, Tone::Ghost, Size::Sm, None, "Undo").on_press(|_| {})), |_| {}))
                        .child(banner(l, Status::Warning, icons::TRIANGLE_ALERT, "Hub service offline. Reconnecting…", vec![button(l, Tone::Ghost, Size::Sm, None, "View log").on_press(|_| {})]))))
                .child(row().gap(16.0)
                    .child(col().w(220.0).gap(2.0).pad(8.0).fill(c.chrome.0).radius(l.radius.md)
                        .child(nav_item(l, "n1", icons::LAYOUT_GRID, "Projects", true, Some((5, false))))
                        .child(nav_item(l, "n2", icons::HARD_DRIVE, "Installs", false, Some((1, true))))
                        .child(nav_item(l, "n3", icons::BOT, "Agents", false, None).on_press(|_| {}))
                        .child(nav_item(l, "n4", icons::GRADUATION_CAP, "Learn", false, None)))
                    .child(hover_card(l).key("card").w(240.0).h(140.0).on_press(|_| {}).child(col().pad(14.0).gap(6.0).child(text("Hover card").font(stillkit::TextRole::Headline)).child(text("Lifts on hover").text_size(12.0).color(c.muted_foreground.0))))
                    .child(card(l).w(320.0).child(empty(l, icons::FOLDER_OPEN, "No projects yet", "Create one from a template, or add a folder you already have.", Some(empty_action(l, icons::PLUS, "New project"))))))
        )
}

fn snap(dark: bool) -> anyhow::Result<()> {
    let look = Look::new(dark);
    let theme = look.theme.clone();
    let mut rt = Runtime::new(S { look }, view, theme, (2600, 1900), 2.0)?;
    rt.frame(Duration::ZERO);
    // Real interaction states: hover one button, press another, focus a field.
    let hover = rt.find("b-hover").expect("button");
    rt.pointer_move(hover.center().x, hover.center().y);
    let card = rt.find("card").expect("card");
    let _ = card;
    rt.focus("name");
    for _ in 0..40 {
        rt.frame(Duration::from_millis(16));
    }
    let press = rt.find("b-press").expect("button");
    rt.pointer_move(press.center().x, press.center().y);
    rt.pointer_down(press.center().x, press.center().y);
    for _ in 0..40 {
        rt.frame(Duration::from_millis(16));
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/ara-kit/gallery-{}.png",
        if dark { "dark" } else { "light" }
    ));
    std::fs::create_dir_all(out.parent().expect("parent"))?;
    std::fs::write(&out, ara_kit::capture::png(&rt.snap()?)?)?;
    println!("{}", out.display());
    Ok(())
}

fn main() -> anyhow::Result<()> {
    snap(true)?;
    snap(false)
}
