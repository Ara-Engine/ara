use super::*;
use crate::{input::InputState, theme::EDITOR_CSS};
use ara_ui_text::TextSystem;

const WINDOW: [f32; 2] = [400.0, 300.0];

fn anchor(x: f32, y: f32, w: f32, h: f32) -> Box2 {
    Box2 { x, y, w, h }
}

#[test]
fn placements_parse_as_side_and_alignment() {
    let p: Placement = "top-end".parse().expect("valid");
    assert_eq!((p.side, p.align), (Side::Top, Align::End));
    let p: Placement = "right".parse().expect("valid");
    assert_eq!((p.side, p.align), (Side::Right, Align::Start));
    assert!("middle".parse::<Placement>().is_err());
    assert!("bottom-left".parse::<Placement>().is_err());
}

/// Below and aligned when it fits; above when only the top has room; the larger side
/// clamped when neither does.
#[test]
fn a_popover_flips_when_its_side_has_no_room_and_clamps_when_neither_has() {
    let below = place(
        anchor(20., 40., 80., 20.),
        [120., 100.],
        WINDOW,
        Placement::default(),
        4.,
    );
    assert_eq!(below.origin, [20., 64.]);
    assert_eq!(below.size, [120., 100.]);
    assert_eq!(below.side, Side::Bottom);

    let flipped = place(
        anchor(20., 250., 80., 20.),
        [120., 100.],
        WINDOW,
        Placement::default(),
        4.,
    );
    assert_eq!(flipped.side, Side::Top);
    assert_eq!(flipped.origin, [20., 146.]);
    assert_eq!(flipped.size[1], 100.);

    // 500 px of content: the room above (250 - 4 - 4 = 242) beats below (300 - 4 - 274 = 22).
    let clamped = place(
        anchor(20., 250., 80., 20.),
        [120., 500.],
        WINDOW,
        Placement::default(),
        4.,
    );
    assert_eq!(clamped.side, Side::Top);
    assert_eq!(clamped.size[1], 242.);
    assert_eq!(clamped.origin[1], MARGIN);
}

/// Shifted along the cross axis to stay inside the window; right-side menus flip left.
#[test]
fn a_popover_shifts_inside_the_window_and_submenus_flip_left() {
    let shifted = place(
        anchor(360., 40., 30., 20.),
        [120., 50.],
        WINDOW,
        Placement::default(),
        4.,
    );
    assert_eq!(shifted.origin[0], WINDOW[0] - MARGIN - 120.);

    let right: Placement = "right-start".parse().expect("valid");
    let sub = place(anchor(300., 40., 80., 26.), [120., 60.], WINDOW, right, 0.);
    assert_eq!(sub.side, Side::Left);
    assert_eq!(sub.origin, [180., 40.]);

    let centered: Placement = "bottom-center".parse().expect("valid");
    let c = place(anchor(100., 0., 0., 0.), [80., 20.], WINDOW, centered, 0.);
    assert_eq!(c.origin, [60., 0.]);
    // Wider than the window: the window's width, at the margin.
    let wide = place(anchor(100., 0., 0., 0.), [900., 20.], WINDOW, centered, 0.);
    assert_eq!(wide.origin[0], MARGIN);
    assert_eq!(wide.size[0], WINDOW[0] - 2. * MARGIN);
}

#[test]
fn signed_gaps_overlap_anchor_edges_and_stay_bounded_when_flipped() {
    let right: Placement = "right-start".parse().expect("placement");
    let overlap = place(anchor(40., 30., 80., 24.), [120., 60.], WINDOW, right, -2.);
    assert_eq!(overlap.side, Side::Right);
    assert_eq!(overlap.origin, [118., 30.]);
    let flipped = place(anchor(300., 30., 80., 24.), [120., 60.], WINDOW, right, -2.);
    assert_eq!(flipped.side, Side::Left);
    assert_eq!(flipped.origin, [182., 30.]);
    for offset in [-2., -10_000., f32::MIN] {
        for placement in ["bottom-start", "top-end", "left-center", "right-start"] {
            for window in [[2., 2.], WINDOW] {
                let placed = place(
                    anchor(390., 280., 80., 24.),
                    [900., 600.],
                    window,
                    placement.parse().expect("placement"),
                    offset,
                );
                assert!(
                    placed
                        .origin
                        .iter()
                        .chain(&placed.size)
                        .all(|value| value.is_finite() && *value >= 0.)
                );
                assert!(
                    placed.origin[0] + placed.size[0] <= window[0] + 0.5,
                    "{placed:?}"
                );
                assert!(
                    placed.origin[1] + placed.size[1] <= window[1] + 0.5,
                    "{placed:?}"
                );
            }
        }
    }
}

#[test]
fn frame_accepts_native_submenu_overlap_but_rejects_nonfinite_gaps() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let markup = |offset| {
        format!(
            "<ui id='root'><button id='row' text='Submenu' style='position:absolute;left:300px;top:30px;width:80px;height:24px;padding:0px' /><popover id='submenu' anchor='row' placement='right-start' offset='{offset}' style='width:120px'><button id='child' text='Child' style='height:60px;padding:0px' /></popover></ui>"
        )
    };
    let mut ui = Ui::editor(&markup("-2"), EDITOR_CSS, WINDOW)?;
    ui.frame(&mut text)?;
    let submenu = ui.nodes[ui.find("submenu").expect("submenu")].rect;
    assert_eq!(
        [submenu.x, submenu.y, submenu.w, submenu.h],
        [182., 30., 120., 60.]
    );
    assert_eq!(
        ui.frame(&mut text)?.layout_passes,
        0,
        "overlap placement is stable while idle"
    );
    for offset in ["NaN", "inf", "-inf"] {
        ui.reconcile(&markup(offset))?;
        let error = ui
            .frame(&mut text)
            .expect_err("nonfinite authored gap must be diagnosed");
        assert!(error.to_string().contains("ARA4046"), "{error}");
    }
    Ok(())
}

fn rows(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|i| {
            format!("<button id='{prefix}-{i}' text='Row {i}' style='height:20px;padding:0px' />")
        })
        .collect()
}

/// `Ui::frame` places a popover against its anchor's painted rect: below it, above it
/// when the bottom has no room, and clamped and scrolling when it is taller than the
/// room it has.
#[test]
fn frame_places_popovers_against_painted_anchors() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let markup = format!(
        "<ui id='r'><button id='top' text='Top' style='position:absolute;left:20px;top:10px;width:60px;height:20px;padding:0px' /><button id='low' text='Low' style='position:absolute;left:20px;top:260px;width:60px;height:20px;padding:0px' /><popover id='menu' anchor='top'>{}</popover><popover id='up' anchor='low' placement='bottom-start' offset='0'>{}</popover><popover id='tall' at='200,10'>{}</popover></ui>",
        rows("a", 3),
        rows("b", 2),
        rows("row", 40)
    );
    let mut ui = Ui::editor(&markup, EDITOR_CSS, WINDOW)?;
    ui.frame(&mut text)?;
    let rect = |ui: &Ui, id: &str| ui.nodes[ui.find(id).expect("node")].rect;
    let menu = rect(&ui, "menu");
    assert_eq!([menu.x, menu.y], [20., 34.]);
    assert_eq!(menu.h, 60.);
    let up = rect(&ui, "up");
    assert_eq!(up.y + up.h, 260.);
    let tall = rect(&ui, "tall");
    assert_eq!(tall.y, 14.);
    assert_eq!(tall.y + tall.h, WINDOW[1] - MARGIN);
    // The clamped popover scrolls its rows; the last one is reachable.
    let index = ui.find("tall").expect("tall");
    ui.scroll_to(index, [0., 10_000.])?;
    ui.frame(&mut text)?;
    let last = rect(&ui, "row-39");
    assert!(
        (last.y + last.h - (WINDOW[1] - MARGIN)).abs() < 0.5,
        "{last:?}"
    );
    // A second frame with nothing changed places nothing again.
    let work = ui.frame(&mut text)?;
    assert_eq!(work.layout_passes, 0);
    Ok(())
}

/// A click inside a lower popover closes those above it; a click outside every
/// popover closes all; the top popover's anchor is reported. Focus moves into a new
/// popover's field and back when it closes.
#[test]
fn popovers_dismiss_on_click_away_and_return_focus() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let base = "<button id='title' text='File' style='position:absolute;left:10px;top:10px;width:50px;height:20px;padding:0px' /><field id='name' text='Box' style='position:absolute;left:200px;top:200px;width:80px;height:20px' />";
    let open = format!(
        "<ui id='r'>{base}<popover id='menu' anchor='title'><field id='filter' text='' style='height:20px' />{}</popover><popover id='sub' anchor='row-1' placement='right-start' offset='0'>{}</popover></ui>",
        rows("row", 3),
        rows("sub", 2)
    );
    let mut ui = Ui::editor(&format!("<ui id='r'>{base}</ui>"), EDITOR_CSS, WINDOW)?;
    let mut input = InputState::default();
    input.push(&mut ui, stable_id("r"), Some(stable_id("name")), false)?;
    ui.frame(&mut text)?;
    let mut popovers = Popovers::default();
    ui.reconcile(&open)?;
    ui.frame(&mut text)?;
    popovers.sync(&mut ui, &mut input)?;
    assert_eq!(popovers.open().collect::<Vec<_>>(), ["menu", "sub"]);
    assert_eq!(popovers.top(), Some("sub"));
    assert_eq!(input.focused(), Some(stable_id("filter")));

    let menu = ui.nodes[ui.find("menu").expect("menu")].rect;
    let inside_menu = popovers.pointer_down(&ui, [menu.x + 4., menu.y + menu.h - 4.]);
    assert_eq!(inside_menu.close, ["sub"]);
    let sub = ui.nodes[ui.find("sub").expect("sub")].rect;
    assert!(
        popovers
            .pointer_down(&ui, [sub.x + 4., sub.y + 4.])
            .close
            .is_empty()
    );
    let away = popovers.pointer_down(&ui, [390., 290.]);
    assert_eq!(away.close, ["sub", "menu"]);
    assert!(!away.on_anchor);
    assert!(popovers.contains(&ui, [menu.x + 1., menu.y + 1.]));

    // Closing both returns focus to the field focused before the menu opened.
    ui.reconcile(&format!("<ui id='r'>{base}</ui>"))?;
    ui.frame(&mut text)?;
    popovers.sync(&mut ui, &mut input)?;
    assert!(popovers.is_empty());
    assert_eq!(input.focused(), Some(stable_id("name")));

    // A click on the anchor of the top popover says so.
    ui.reconcile(&format!(
        "<ui id='r'>{base}<popover id='menu' anchor='title'>{}</popover></ui>",
        rows("row", 2)
    ))?;
    ui.frame(&mut text)?;
    popovers.sync(&mut ui, &mut input)?;
    let title = ui.nodes[ui.find("title").expect("title")].rect;
    let toggle = popovers.pointer_down(&ui, [title.x + 2., title.y + 2.]);
    assert_eq!(toggle.close, ["menu"]);
    assert!(toggle.on_anchor);

    // Focus returns best effort: closing still succeeds when the control that had it
    // was disabled meanwhile.
    let disabled = base.replace("<field id='name'", "<field id='name' disabled='true'");
    ui.reconcile(&format!("<ui id='r'>{disabled}</ui>"))?;
    ui.frame(&mut text)?;
    popovers.sync(&mut ui, &mut input)?;
    assert!(popovers.is_empty());
    Ok(())
}

/// CSS sizes stand unless the window is too small: a 260 px picker stays 260 px wide;
/// a fixed-height flex popover (a palette) clamps to the room and stays clamped.
#[test]
fn popovers_keep_css_sizes_until_clamped() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let markup = "<ui id='r'><button id='swatch' text='S' style='position:absolute;left:300px;top:20px;width:20px;height:20px;padding:0px' /><popover id='picker' anchor='swatch' placement='left-start' style='width:260px'><button id='p0' text='Hue' style='height:20px;padding:0px' /></popover><popover id='palette' at='200,100' placement='bottom-center' offset='0' style='display:flex;width:300px;height:400px'><scroll id='results' style='flex-grow:1' /></popover></ui>";
    let mut ui = Ui::editor(markup, EDITOR_CSS, WINDOW)?;
    ui.frame(&mut text)?;
    let rect = |ui: &Ui, id: &str| ui.nodes[ui.find(id).expect("node")].rect;
    let picker = rect(&ui, "picker");
    assert_eq!(picker.w, 260.);
    assert_eq!(picker.x + picker.w, 296.);
    let palette = rect(&ui, "palette");
    assert_eq!(palette.w, 300.);
    // 400 px tall in a 300 px window: below the point there are 300 - 100 - 4 = 196 px,
    // above it 96; it opens below, clamped.
    assert_eq!([palette.y, palette.h], [100., 196.]);
    assert_eq!(ui.frame(&mut text)?.layout_passes, 0);
    Ok(())
}

#[test]
fn transient_width_limits_wrap_long_copy_and_fit_short_copy() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><tooltip id='short' at='20,20'><text id='short-copy' text='Save' /></tooltip><popover id='long' at='390,260'><text id='long-copy' text='A longer explanation of this property and the available recovery actions that must wrap within the shared width limit rather than extend beyond the edge of the editor window.' /></popover></ui>",
        EDITOR_CSS,
        WINDOW,
    )?;
    ui.frame(&mut text)?;
    let short = ui.nodes[ui.find("short").expect("short")].rect;
    assert!(short.w < 100., "short hint should fit its text: {short:?}");
    let long = ui.nodes[ui.find("long").expect("long")].rect;
    let copy = ui.nodes[ui.find("long-copy").expect("copy")].rect;
    assert!(long.w <= MAX_WIDTH + 0.5, "{long:?}");
    assert!(
        long.x >= MARGIN && long.x + long.w <= WINDOW[0] - MARGIN + 0.5,
        "{long:?}"
    );
    assert!(copy.h > 20., "long copy wraps before placement: {copy:?}");
    assert_eq!(
        ui.frame(&mut text)?.layout_passes,
        0,
        "limits must not create idle relayout"
    );
    Ok(())
}

#[test]
fn retained_tooltip_intrinsic_width_does_not_round_down_and_wrap_short_copy() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><tooltip id='tip' at='20,20' style='padding:8px;border-width:1px;max-width:360px'><text id='copy' text='' style='font-size:12px;white-space:normal' /></tooltip></ui>",
        EDITOR_CSS,
        WINDOW,
    )?;
    for value in [
        "Play · F5",
        "Project · scenes/main.scene",
        "Collapse Hierarchy",
        "إعدادات العرض · éclairage",
        "Play · F5",
    ] {
        let mut reference = ara_ui_text::Text::new(value.into());
        let natural = text.measure(&mut reference, None, 12.);
        assert!(natural.width + 18. < MAX_WIDTH);
        let copy = ui.find("copy").expect("copy");
        ui.set_text(copy, value)?;
        ui.frame(&mut text)?;
        let copy = &ui.nodes[copy];
        let tip = &ui.nodes[ui.find("tip").expect("tip")];
        let paragraph = copy.text.as_ref().expect("paragraph");
        assert_eq!(
            paragraph.layout.len(),
            reference.layout.len(),
            "intrinsic width must keep short copy on its natural line: {value:?}, natural={natural:?}, copy={:?}",
            copy.rect
        );
        assert!(
            tip.rect.w >= natural.width + 18. - 0.5 && tip.rect.w <= natural.width + 19.,
            "tooltip fits its unbroken text plus padding and borders: {value:?}, natural={natural:?}, tip={:?}",
            tip.rect
        );
        assert!(copy.rect.h + 0.5 >= natural.height);
        assert_eq!(ui.frame(&mut text)?.layout_passes, 0);
    }
    Ok(())
}

#[test]
fn explicit_large_surface_override_still_clamps_to_the_viewport() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><popover id='palette' at='0,0' style='width:640px;max-width:640px'><text id='copy' text='Commands' /></popover></ui>",
        EDITOR_CSS,
        [800., 600.],
    )?;
    ui.frame(&mut text)?;
    assert_eq!(ui.nodes[ui.find("palette").expect("palette")].rect.w, 640.);
    ui.resize([300., 200.])?;
    ui.frame(&mut text)?;
    assert!(ui.nodes[ui.find("palette").expect("palette")].rect.w <= 292.5);
    Ok(())
}

#[test]
fn hover_delay_tracks_stable_targets_and_restarts_after_clock_reset() {
    let mut delay = HoverDelay::default();
    assert!(!delay.update(Some(1), 100));
    assert!(!delay.update(Some(1), 599));
    assert!(delay.update(Some(1), 600));
    assert!(!delay.update(Some(2), 601));
    assert!(!delay.update(Some(2), 3));
    assert!(delay.update(Some(2), 503));
    assert!(!delay.update(None, 900));
    assert!(!delay.update(Some(2), 901));
}

#[test]
fn placement_is_finite_and_inside_small_or_detached_surfaces() {
    for window in [[2., 2.], [400., 300.], [0., 0.]] {
        for placement in ["bottom-start", "top-end", "left-center", "right-start"] {
            let placed = place(
                anchor(-10_000., 10_000., 0., 0.),
                [200., 100.],
                window,
                placement.parse().expect("placement"),
                4.,
            );
            assert!(
                placed
                    .origin
                    .iter()
                    .chain(&placed.size)
                    .all(|v| v.is_finite() && *v >= 0.)
            );
            assert!(
                placed.origin[0] + placed.size[0] <= window[0] + 0.5,
                "{placed:?}"
            );
            assert!(
                placed.origin[1] + placed.size[1] <= window[1] + 0.5,
                "{placed:?}"
            );
        }
    }
}

#[test]
fn horizontally_clamped_popover_recovers_width_when_its_anchor_moves() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let markup = |x| {
        format!(
            "<ui id='root'><button id='anchor' text='A' style='position:absolute;left:{x}px;top:40px;width:20px;height:20px;padding:0px' /><popover id='menu' anchor='anchor' placement='right-start' style='width:160px'><text id='copy' text='A wrapping menu explanation with enough text to change its height when the available width changes.' /></popover></ui>"
        )
    };
    let mut ui = Ui::editor(&markup(90), EDITOR_CSS, [200., 300.])?;
    ui.frame(&mut text)?;
    let narrow = ui.nodes[ui.find("menu").expect("menu")].rect;
    assert!(
        narrow.w < 100.,
        "both sides should constrain the first opening: {narrow:?}"
    );
    ui.reconcile(&markup(5))?;
    ui.frame(&mut text)?;
    let wide = ui.nodes[ui.find("menu").expect("menu")].rect;
    assert_eq!(wide.w, 160., "old clamp must not stick: {wide:?}");
    assert!(
        wide.y >= MARGIN && wide.y + wide.h <= 300. - MARGIN + 0.5,
        "{wide:?}"
    );
    assert_eq!(ui.frame(&mut text)?.layout_passes, 0);
    Ok(())
}

#[test]
fn final_wrapped_tooltip_text_keeps_its_measured_height_and_bottom_padding() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><button id='anchor' text='Resume' /><tooltip id='tip' class='card' anchor='anchor' style='min-height:48px;max-width:200px'><text id='copy' text='[Cômpôñëñt prëvïëw~~~~~~]' /></tooltip></ui>",
        EDITOR_CSS,
        [320., 360.],
    )?;
    ui.frame(&mut text)?;
    let tip = &ui.nodes[ui.find("tip").expect("tip")];
    let copy = &ui.nodes[ui.find("copy").expect("copy")];
    let lines = copy.text.as_ref().expect("paragraph");
    assert!(
        copy.rect.h + 0.5 >= lines.layout.height(),
        "final paragraph must fit its own row"
    );
    assert!(
        tip.rect.y + tip.rect.h - copy.rect.y - lines.layout.height() >= 12.0 - 0.5,
        "wrapped copy must keep the authored bottom padding"
    );
    assert_eq!(ui.frame(&mut text)?.layout_passes, 0);
    Ok(())
}

#[test]
fn flex_scroll_popup_keeps_intrinsic_height_when_clamped_then_recovers_and_shrinks_to_content()
-> Result<()> {
    let mut text = TextSystem::bundled()?;
    let markup = |count| {
        format!(
            "<ui id='root'><button id='anchor' text='Open' style='position:absolute;left:8px;top:90px;width:60px;height:20px;padding:0px' /><popover id='popup' anchor='anchor' offset='0' style='display:flex;width:220px;max-height:180px;padding:4px;gap:4px'><field id='search' text='' style='height:26px;padding:0px' /><scroll id='list' style='min-height:0px;max-height:240px;flex-basis:auto;flex-shrink:1'>{}</scroll><button id='footer' text='Close' style='height:20px;padding:0px' /></popover></ui>",
            rows("option", count)
        )
    };
    let mut ui = Ui::editor(&markup(20), EDITOR_CSS, [320., 240.])?;
    ui.frame(&mut text)?;
    let bounded = |ui: &Ui| {
        let popup = &ui.nodes[ui.find("popup").expect("popup")];
        let list = &ui.nodes[ui.find("list").expect("list")];
        let footer = &ui.nodes[ui.find("footer").expect("footer")];
        assert!(
            popup.rect.y >= 0. && popup.rect.y + popup.rect.h <= ui.size[1] + 0.5,
            "{:?}",
            popup.rect
        );
        assert!(list.clip.h > 0., "list remains pointer-reachable");
        assert!(
            footer.rect.y + footer.rect.h <= popup.rect.y + popup.rect.h + 0.5,
            "footer must stay inside the clamped flex popup"
        );
    };
    bounded(&ui);
    let clamped = ui.nodes[ui.find("popup").expect("popup")].rect.h;
    assert!(clamped < 180.);
    assert_eq!(
        ui.frame(&mut text)?.layout_passes,
        0,
        "height clamp must not oscillate"
    );
    ui.resize([320., 400.])?;
    ui.frame(&mut text)?;
    bounded(&ui);
    assert_eq!(
        ui.nodes[ui.find("popup").expect("popup")].rect.h,
        180.,
        "authored maximum height remains effective"
    );
    assert_eq!(ui.frame(&mut text)?.layout_passes, 0);
    ui.reconcile(&markup(1))?;
    ui.frame(&mut text)?;
    bounded(&ui);
    assert!(
        ui.nodes[ui.find("popup").expect("popup")].rect.h < clamped,
        "a filtered list fits its remaining content"
    );
    assert_eq!(ui.frame(&mut text)?.layout_passes, 0);
    Ok(())
}
