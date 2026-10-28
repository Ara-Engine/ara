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
