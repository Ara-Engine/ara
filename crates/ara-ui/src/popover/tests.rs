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
fn placed_controls_keep_their_authored_width_when_they_fit() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='r'><popover id='picker' at='20,20' style='width:260px;padding:10px'><field id='hex' text='#FFFFFF' /><slider id='channel' min='0' max='1' /></popover></ui>",
        EDITOR_CSS,
        WINDOW,
    )?;
    ui.frame(&mut text)?;
    ui.frame(&mut text)?;
    let picker = ui.find("picker").expect("picker");
    assert_eq!(ui.nodes[picker].rect.w, 260.);
    let channel = ui.find("channel").expect("channel");
    assert!(ui.nodes[channel].rect.w >= 230.);
    ui.resize([200., 300.])?;
    ui.frame(&mut text)?;
    assert!(ui.nodes[picker].rect.w <= 192.);
    ui.resize(WINDOW)?;
    ui.frame(&mut text)?;
    assert_eq!(ui.nodes[picker].rect.w, 260.);
    Ok(())
}

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
    Ok(())
}
