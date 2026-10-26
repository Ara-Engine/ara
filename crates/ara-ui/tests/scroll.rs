use ara_ui::{
    Result, Ui,
    cursor::Cursor,
    input::{InputEvent, InputState},
    source::stable_id,
    virtualize::{VirtualList, VirtualView},
};
use ara_ui_text::{Clipboard, TextSystem};

struct Clip;
impl Clipboard for Clip {
    fn read(&mut self) -> Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
}
fn fixture() -> Result<(Ui, TextSystem, usize)> {
    let mut ui = Ui::parse(
        "<ui id='root'><scroll id='list'><button id='a' text='First' action='Choose' cursor='grab' active-cursor='grabbing'/><button id='b' text='Second' action='Choose'/><button id='c' text='Third' action='Choose'/><button id='d' text='Fourth' action='Choose'/></scroll></ui>",
        "ui{width:200px;height:120px} scroll{width:200px;height:120px;overflow:hidden} button{height:60px;padding:0px}",
        [200., 120.],
    )?;
    let mut text = TextSystem::bundled()?;
    ui.frame(&mut text)?;
    let list = ui.find("list").expect("list");
    Ok((ui, text, list))
}
#[test]
fn thumb_ratio_drag_capture_track_paging_and_fractional_wheel() -> Result<()> {
    let (mut ui, mut text, list) = fixture()?;
    let bar = ui.scrollbars(list)?[0];
    assert_eq!(bar.axis, 1);
    assert_eq!(bar.maximum, 120.);
    assert_eq!(bar.thumb.h, 60.);
    assert_eq!(
        ui.hit_test(195., 15.),
        Some(list),
        "bars block host scene input"
    );
    let mut input = InputState::default();
    let mut clip = Clip;
    let painted_width = |ui: &Ui| {
        ui.display.commands.iter().find_map(|draw| match draw {
            ara_ui::paint::Draw::RoundedRect { node, rect, .. } if *node == stable_id("list") => {
                Some(rect.w)
            }
            _ => None,
        })
    };
    assert_eq!(painted_width(&ui), Some(4.));
    input.event(&mut ui, InputEvent::PointerMove([195., 15.]), &mut clip)?;
    ui.frame(&mut text)?;
    assert_eq!(painted_width(&ui), Some(8.));
    input.event(&mut ui, InputEvent::PointerDown([195., 15.]), &mut clip)?;
    assert!(input.scrolling());
    assert_eq!(input.focused(), None, "scrollbar preserves keyboard focus");
    input.event(&mut ui, InputEvent::PointerMove([400., 75.]), &mut clip)?;
    assert_eq!(ui.scroll_position(list), Some([0., 120.]));
    input.event(&mut ui, InputEvent::PointerUp([400., 75.]), &mut clip)?;
    assert!(!input.scrolling());
    assert_eq!(
        ui.drain_actions().count(),
        0,
        "thumb release cannot activate the covered row"
    );
    ui.frame(&mut text)?;
    input.event(&mut ui, InputEvent::PointerDown([195., 10.]), &mut clip)?;
    input.event(&mut ui, InputEvent::PointerUp([195., 10.]), &mut clip)?;
    assert_eq!(ui.scroll_position(list), Some([0., 0.]));
    assert_eq!(
        ui.drain_actions().count(),
        0,
        "track clicks cannot choose rows"
    );
    input.event(
        &mut ui,
        InputEvent::Wheel {
            position: [60., 30.],
            delta: [0., 0.75],
        },
        &mut clip,
    )?;
    assert_eq!(ui.scroll_position(list), Some([0., 0.75]));
    ui.frame(&mut text)?;
    assert_eq!(
        ui.frame(&mut text)?.paint_nodes,
        0,
        "idle scrollbar does no paint/layout work"
    );
    Ok(())
}
#[test]
fn keyboard_reveals_offscreen_rows_and_content_shrink_clamps_offset() -> Result<()> {
    let (mut ui, mut text, list) = fixture()?;
    let mut input = InputState::default();
    let mut clip = Clip;
    input.focus(&mut ui, stable_id("b"))?;
    input.event(&mut ui, InputEvent::Tab { reverse: false }, &mut clip)?;
    assert_eq!(input.focused(), Some(stable_id("c")));
    assert_eq!(ui.scroll_position(list), Some([0., 60.]));
    ui.frame(&mut text)?;
    let c = ui.find("c").expect("row");
    assert!(ui.nodes[c].rect.intersects(ui.nodes[c].clip));
    input.focus(&mut ui, stable_id("d"))?;
    ui.frame(&mut text)?;
    let d = ui.find("d").expect("row");
    ui.set_visible(c, false)?;
    ui.set_visible(d, false)?;
    ui.frame(&mut text)?;
    assert_eq!(ui.scroll_position(list), Some([0., 0.]));
    assert!(ui.scrollbars(list)?.is_empty());
    Ok(())
}
#[test]
fn hiding_a_retained_tab_preserves_its_scroll_position() -> Result<()> {
    let (mut ui, mut text, list) = fixture()?;
    ui.scroll_to(list, [0., 120.])?;
    ui.frame(&mut text)?;
    ui.set_visible(list, false)?;
    ui.frame(&mut text)?;
    assert_eq!(ui.scroll_position(list), Some([0., 120.]));
    ui.set_visible(list, true)?;
    ui.frame(&mut text)?;
    assert_eq!(ui.scroll_position(list), Some([0., 120.]));
    Ok(())
}
#[test]
fn virtual_scrollbar_represents_100k_rows_with_a_bounded_pool() -> Result<()> {
    let mut ui = Ui::parse(
        "<ui id='root'><scroll id='list'/></ui>",
        "ui{width:200px;height:220px} scroll{width:200px;height:220px;overflow:hidden}",
        [200., 220.],
    )?;
    let mut text = TextSystem::bundled()?;
    ui.frame(&mut text)?;
    let list = ui.find("list").expect("list");
    let model = VirtualList {
        items: (0..100_000).map(|i| (i, format!("Row {i}"))).collect(),
    };
    let mut view = VirtualView::mount(&mut ui, list, 22., 220., vec![200.])?;
    view.scroll(&mut ui, &model, 0., 220.)?;
    ui.frame(&mut text)?;
    let nodes = ui.nodes.len();
    assert!(nodes < 40);
    let bar = ui.scrollbars(list)?[0];
    assert_eq!(bar.maximum, 2_199_780.);
    let mut input = InputState::default();
    input.event(&mut ui, InputEvent::PointerDown([195., 8.]), &mut Clip)?;
    input.event(&mut ui, InputEvent::PointerMove([195., 219.]), &mut Clip)?;
    let offset = ui.scroll_position(list).expect("scroll")[1];
    view.scroll(&mut ui, &model, offset, 220.)?;
    ui.frame(&mut text)?;
    assert_eq!(ui.nodes.len(), nodes);
    assert!(view.visible_keys().any(|key| key == 99_999));
    let last = ui.find("list/key-99999").expect("last virtual row");
    assert!(ui.nodes[last].rect.intersects(ui.nodes[list].rect));
    Ok(())
}
#[test]
fn virtual_columns_scroll_horizontally_without_resetting_on_row_recycling() -> Result<()> {
    let mut ui = Ui::parse(
        "<ui id='root'><scroll id='list'/></ui>",
        "ui{width:200px;height:100px} scroll{width:200px;height:100px}",
        [200., 100.],
    )?;
    let mut text = TextSystem::bundled()?;
    ui.frame(&mut text)?;
    let list = ui.find("list").expect("list");
    let model = VirtualList {
        items: (0..20).map(|i| (i, format!("Wide row {i}"))).collect(),
    };
    let mut view = VirtualView::mount(&mut ui, list, 20., 100., vec![400.])?;
    view.scroll(&mut ui, &model, 0., 100.)?;
    ui.frame(&mut text)?;
    assert_eq!(ui.scrollbars(list)?.len(), 2);
    ui.scroll_to(list, [100., 40.])?;
    view.scroll(&mut ui, &model, 40., 100.)?;
    ui.frame(&mut text)?;
    assert_eq!(ui.scroll_position(list), Some([100., 40.]));
    let first = view.visible_nodes()[0];
    assert_eq!(ui.nodes[first].rect.x, -100.);
    assert!(ui.nodes[first].rect.w >= 400.);
    assert_eq!(ui.hit_test(170., 10.), Some(first));
    let id = stable_id(&ui.nodes[first].element.id);
    InputState::default().focus(&mut ui, id)?;
    assert_eq!(
        ui.scroll_position(list),
        Some([100., 40.]),
        "focusing a wide row preserves horizontal position"
    );
    Ok(())
}
#[test]
fn cursors_follow_children_capture_disabled_state_and_split_orientation() -> Result<()> {
    let (mut ui, mut text, _) = fixture()?;
    let mut input = InputState::default();
    assert_eq!(ui.cursor(&input, [50., 80.]), Cursor::Pointer);
    let a = ui.find("a").expect("row");
    assert_eq!(ui.cursor(&input, [50., 20.]), Cursor::Grab);
    input.event(&mut ui, InputEvent::PointerDown([50., 20.]), &mut Clip)?;
    assert_eq!(ui.cursor(&input, [500., 500.]), Cursor::Grabbing);
    input.event(&mut ui, InputEvent::Cancel, &mut Clip)?;
    let mut state = ui.nodes[a].state;
    state.disabled = true;
    ui.set_state(a, state);
    ui.frame(&mut text)?;
    assert_eq!(ui.cursor(&input, [50., 20.]), Cursor::NotAllowed);
    let mut split = Ui::parse(
        "<ui id='root'><panel id='left'/><splitter id='s'/><field id='f' text='Value'/></ui>",
        "ui{width:200px;height:100px;flex-direction:row} panel{width:50px} splitter{width:5px} field{width:100px}",
        [200., 100.],
    )?;
    split.frame(&mut text)?;
    assert_eq!(split.cursor(&input, [52., 50.]), Cursor::ResizeEw);
    assert_eq!(split.cursor(&input, [90., 30.]), Cursor::Text);
    Ok(())
}
