use super::*;
use crate::{Work, input::InputState, theme::EDITOR_CSS};
use ara_ui_text::TextSystem;

fn fits(child: Box2, parent: Box2) {
    assert!(
        child.x >= parent.x - 0.5 && child.y >= parent.y - 0.5,
        "{child:?} in {parent:?}"
    );
    assert!(
        child.x + child.w <= parent.x + parent.w + 0.5,
        "{child:?} in {parent:?}"
    );
    assert!(
        child.y + child.h <= parent.y + parent.h + 0.5,
        "{child:?} in {parent:?}"
    );
}

#[test]
fn measured_feedback_wraps_unbroken_copy_and_moves_the_complete_subtree_without_layout()
-> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><button id='behind' text='Drop target' /><feedback id='feedback' style='left:0px;top:0px;width:140px;height:24px;padding:8px;border-width:1px;font-size:12px'><text id='copy' text='' style='font-size:24px;white-space:nowrap' /></feedback></ui>",
        EDITOR_CSS,
        [1000.0, 600.0],
    )?;
    let feedback = ui.find("feedback").expect("feedback");
    let copy = ui.find("copy").expect("copy");
    let avoid = Box2 {
        x: 0.0,
        y: 70.0,
        w: 260.0,
        h: 26.0,
    };
    let long = format!(
        "Move {} inside this folder",
        "UnbrokenAssetReference".repeat(8)
    );
    ui.set_text(copy, &long)?;
    ui.place_feedback(feedback, [120.0, 85.0], Some(avoid))?;
    ui.frame(&mut text)?;
    let parent = ui.nodes[feedback].rect;
    assert!(
        (MIN_WIDTH..=crate::popover::MAX_WIDTH + 0.5).contains(&parent.w),
        "{parent:?}"
    );
    assert!(
        parent.w > 140.0,
        "legacy fixed width must not constrain feedback: {parent:?}"
    );
    assert!(parent.x >= avoid.x + avoid.w + GAP - 0.5, "{parent:?}");
    assert!(!parent.intersects(avoid));
    let paragraph = ui.nodes[copy].text.as_ref().expect("paragraph");
    assert!(
        paragraph.layout.len() > 1,
        "unbroken source paths must wrap"
    );
    assert!(paragraph.layout.width() <= ui.nodes[copy].rect.w + 0.5);
    assert_eq!(ui.nodes[copy].computed.font_size, 12.0);
    assert!(ui.nodes[copy].computed.wrap_text);
    fits(ui.nodes[copy].rect, parent);
    assert!(
        parent.h > 24.0,
        "legacy chrome height must not clip wrapped copy"
    );
    assert!(!ui.widgets.contains_key(&stable_id("feedback")));
    assert!(!ui.accepts_input(copy));
    assert_eq!(
        ui.popover_nodes().count(),
        0,
        "feedback must not take popup focus"
    );

    ui.set_text(copy, "Move inside")?;
    ui.frame(&mut text)?;
    let short = ui.nodes[feedback].rect;
    assert!(
        short.w < parent.w && short.h < parent.h,
        "copy change must refit: {short:?} vs {parent:?}"
    );
    fits(ui.nodes[copy].rect, short);
    let child_offset = [
        ui.nodes[copy].rect.x - short.x,
        ui.nodes[copy].rect.y - short.y,
    ];
    for point in [[140.0, 110.0], [160.0, 130.0], [160.0, 130.0]] {
        ui.place_feedback(feedback, point, Some(avoid))?;
        let work = ui.frame(&mut text)?;
        assert_eq!(
            work.layout_passes, 0,
            "stable label motion is paint-only: {work:?}"
        );
        assert_eq!(
            work.shaped, 0,
            "stable label motion keeps shaping: {work:?}"
        );
        let rect = ui.nodes[feedback].rect;
        fits(ui.nodes[copy].rect, rect);
        assert_eq!(
            [
                ui.nodes[copy].rect.x - rect.x,
                ui.nodes[copy].rect.y - rect.y
            ],
            child_offset
        );
    }
    assert_eq!(ui.frame(&mut text)?, Work::default());
    Ok(())
}

#[test]
fn feedback_flips_away_from_target_rows_and_stays_inside_small_detached_viewports() -> Result<()> {
    let content = [320.0, 70.0];
    let left = Box2 {
        x: 0.0,
        y: 10.0,
        w: 260.0,
        h: 24.0,
    };
    let right = Box2 {
        x: 640.0,
        y: 10.0,
        w: 260.0,
        h: 24.0,
    };
    for (avoid, expected_right) in [(left, true), (right, false)] {
        let result = place(
            Anchor {
                point: [avoid.x + 20.0, 20.0],
                avoid: Some(avoid),
            },
            content,
            [900.0, 300.0],
        );
        assert!(!result.intersects(avoid));
        if expected_right {
            assert!(result.x >= avoid.x + avoid.w + GAP);
        } else {
            assert!(result.x + result.w + GAP <= avoid.x);
        }
    }
    for avoid in [
        Box2 { x: 2000., ..right },
        Box2 {
            y: 2000.,
            w: 900.,
            ..left
        },
    ] {
        let result = place(
            Anchor {
                point: [250., 250.],
                avoid: Some(avoid),
            },
            content,
            [900., 300.],
        );
        fits(
            result,
            Box2 {
                x: 0.,
                y: 0.,
                w: 900.,
                h: 300.,
            },
        );
    }
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><feedback id='feedback' style='padding:8px;font-size:12px'><text id='copy' text='A localized drag explanation with enough words to wrap after the detached surface becomes narrow.' /></feedback></ui>",
        EDITOR_CSS,
        [500.0, 300.0],
    )?;
    let feedback = ui.find("feedback").expect("feedback");
    let copy = ui.find("copy").expect("copy");
    for size in [[500.0, 300.0], [180.0, 280.0], [420.0, 300.0]] {
        ui.resize(size)?;
        ui.place_feedback(feedback, [size[0] - 2.0, size[1] - 2.0], None)?;
        ui.frame(&mut text)?;
        fits(
            ui.nodes[feedback].rect,
            Box2 {
                x: 0.0,
                y: 0.0,
                w: size[0],
                h: size[1],
            },
        );
        fits(ui.nodes[copy].rect, ui.nodes[feedback].rect);
        assert!(ui.nodes[feedback].rect.w <= crate::popover::MAX_WIDTH + 0.5);
        assert_eq!(ui.frame(&mut text)?, Work::default());
    }
    Ok(())
}

#[test]
fn inline_banner_keeps_intrinsic_height_and_host_width_without_popup_limits() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><panel id='rail' style='width:600px;height:auto;min-height:56px'><banner id='status' style='height:24px;padding:8px;font-size:13px'><text id='copy' text='A recoverable import diagnostic that must remain readable and wrap into the status rail instead of being clipped to the height of one toolbar control.' style='white-space:nowrap' /></banner></panel></ui>",
        EDITOR_CSS,
        [800.0, 400.0],
    )?;
    ui.frame(&mut text)?;
    let status = ui.find("status").expect("banner");
    let copy = ui.find("copy").expect("copy");
    assert!(
        ui.nodes[status].rect.w > crate::popover::MAX_WIDTH,
        "inline banners keep their host width"
    );
    assert!(ui.nodes[status].rect.h > 24.0);
    assert_eq!(ui.nodes[copy].computed.font_size, 13.0);
    assert!(ui.nodes[copy].computed.wrap_text);
    fits(ui.nodes[copy].rect, ui.nodes[status].rect);
    let rail = ui.nodes[ui.find("rail").expect("rail")].rect;
    fits(ui.nodes[status].rect, rail);
    assert_eq!(ui.frame(&mut text)?, Work::default());
    Ok(())
}

fn banner_rail() -> Result<Ui> {
    Ui::editor(
        "<ui id='root'><panel id='host' style='width:100%;height:260px'><scroll id='rail' style='height:auto;min-height:56px;max-height:180px;flex-basis:auto;flex-shrink:1;padding:4px;border-width:1px'><panel id='cards' style='gap:6px;padding:2px;flex-shrink:0'><banner id='first' style='padding:6px;gap:4px;font-size:12px'><panel id='heading' style='height:22px;flex-shrink:0'><text id='title' style='font-size:12px' text='Import files' /></panel><text id='message' text='Importing content' /><progress id='progress' style='height:3px' value='0.5' /></banner><banner id='second' style='padding:6px;font-size:12px'><text id='next-message' text='Next import' /></banner></panel></scroll><panel id='browser' style='min-height:0px;flex-grow:1;flex-shrink:1;flex-basis:1000px' /></panel></ui>",
        EDITOR_CSS,
        [330.0, 500.0],
    )
}

#[test]
fn banner_rail_reserves_the_measured_first_card_and_releases_it_after_reflow_or_dismissal()
-> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = banner_rail()?;
    let first = ui.find("first").expect("first card");
    let second = ui.find("second").expect("second card");
    let rail = ui.find("rail").expect("rail");
    let message = ui.find("message").expect("message");
    let mut short_height = 0.0;
    let mut long_height = 0.0;
    for (copy, size, long) in [
        ("Importing content", [330.0, 500.0], false),
        (
            "Preparing authored content with a detailed progress message that must wrap into additional readable lines.",
            [330.0, 500.0],
            true,
        ),
        ("Importing content", [330.0, 500.0], false),
        ("Importing content", [230.0, 500.0], false),
        ("Importing content", [330.0, 500.0], false),
    ] {
        ui.set_text(message, copy)?;
        ui.resize(size)?;
        ui.frame(&mut text)?;
        let card = ui.nodes[first].rect;
        let viewport = ui.nodes[rail].rect;
        fits(card, viewport);
        fits(ui.nodes[message].rect, ui.nodes[message].clip);
        let progress = ui.find("progress").expect("progress");
        fits(ui.nodes[progress].rect, ui.nodes[progress].clip);
        assert!(
            viewport.h <= 180.5,
            "authored cap remains authoritative: {viewport:?}"
        );
        if long {
            long_height = viewport.h;
        } else if short_height == 0.0 {
            short_height = viewport.h;
        } else if size[0] == 330.0 {
            assert!((viewport.h - short_height).abs() < 0.5);
        }
        assert_eq!(ui.frame(&mut text)?, Work::default());
    }
    assert!(
        long_height > short_height,
        "wrapped messages grow their first-card minimum"
    );
    ui.set_visible(first, false)?;
    ui.frame(&mut text)?;
    fits(ui.nodes[second].rect, ui.nodes[rail].rect);
    assert!(
        ui.nodes[rail].rect.h < short_height,
        "dismissing the first card releases its fit"
    );
    ui.set_visible(second, false)?;
    ui.frame(&mut text)?;
    let restored = ui.tree.style(ui.nodes[rail].layout_id)?.min_size.height;
    assert_eq!(
        restored, ui.nodes[rail].computed.layout.min_size.height,
        "an empty rail retains no stale runtime fit"
    );
    assert_eq!(ui.frame(&mut text)?, Work::default());
    Ok(())
}

#[test]
fn banner_rail_honors_its_explicit_cap_and_scrolls_an_oversized_first_card() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = banner_rail()?;
    let rail = ui.find("rail").expect("rail");
    let first = ui.find("first").expect("first card");
    let message = ui.find("message").expect("message");
    ui.set_text(
        message,
        &"A detailed import inventory with created and renamed logical paths. ".repeat(30),
    )?;
    ui.frame(&mut text)?;
    assert!(
        ui.nodes[first].rect.h > 180.0,
        "fixture must need internal scrolling"
    );
    assert!(
        (ui.nodes[rail].rect.h - 180.0).abs() <= 0.5,
        "the runtime minimum never defeats the authored cap"
    );
    assert!(ui.scroll_extent(rail)?[1] > ui.nodes[rail].rect.h);
    assert!(
        ui.scrollbars(rail)?
            .iter()
            .any(|bar| bar.axis == 1 && bar.maximum > 0.0)
    );
    assert_eq!(ui.frame(&mut text)?, Work::default());
    ui.set_text(message, "Importing content")?;
    ui.frame(&mut text)?;
    assert!(
        ui.nodes[rail].rect.h < 180.0,
        "shorter copy releases a previously saturated cap"
    );
    fits(ui.nodes[first].rect, ui.nodes[rail].rect);
    assert_eq!(ui.frame(&mut text)?, Work::default());
    Ok(())
}

#[test]
fn reconciled_message_copy_inherits_existing_feedback_typography() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><feedback id='feedback' style='padding:8px;font-size:12px' /></ui>",
        EDITOR_CSS,
        [500., 300.],
    )?;
    let feedback = ui.find("feedback").expect("feedback");
    ui.place_feedback(feedback, [30., 30.], None)?;
    ui.frame(&mut text)?;
    ui.reconcile("<ui id='root'><feedback id='feedback' style='padding:8px;font-size:12px'><text id='new-copy' style='font-size:24px;white-space:nowrap' text='A new descriptive destination whose complete label needs to wrap after a retained markup update.' /></feedback></ui>")?;
    ui.frame(&mut text)?;
    let copy = ui.find("new-copy").expect("new copy");
    let feedback = ui.find("feedback").expect("retained feedback");
    assert_eq!(ui.nodes[copy].computed.font_size, 12.);
    assert!(ui.nodes[copy].computed.wrap_text);
    fits(ui.nodes[copy].rect, ui.nodes[feedback].rect);
    assert_eq!(ui.frame(&mut text)?, Work::default());
    Ok(())
}

#[test]
fn feedback_geometry_validation_and_noninteractive_descendants_preserve_focus() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><button id='owner' text='Owner' /><feedback id='feedback'><button id='not-a-control' text='Passive copy' /></feedback><panel id='ordinary' /></ui>",
        EDITOR_CSS,
        [400.0, 300.0],
    )?;
    let feedback = ui.find("feedback").expect("feedback");
    assert!(ui.place_feedback(feedback, [f32::NAN, 0.0], None).is_err());
    assert!(
        ui.place_feedback(ui.find("ordinary").expect("ordinary"), [1.0, 1.0], None)
            .is_err()
    );
    assert!(
        ui.place_feedback(
            feedback,
            [1.0, 1.0],
            Some(Box2 {
                w: -1.0,
                ..Box2::default()
            })
        )
        .is_err()
    );
    ui.place_feedback(feedback, [10.0, 10.0], None)?;
    ui.frame(&mut text)?;
    let mut input = InputState::default();
    input.push(
        &mut ui,
        stable_id("root"),
        Some(stable_id("not-a-control")),
        false,
    )?;
    assert_eq!(input.focused(), Some(stable_id("owner")));
    input.focus(&mut ui, stable_id("owner"))?;
    assert!(input.focus(&mut ui, stable_id("not-a-control")).is_err());
    assert_eq!(input.focused(), Some(stable_id("owner")));
    Ok(())
}
