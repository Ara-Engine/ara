use ara_ui::{
    Result, Ui,
    binding::{BindingKey, Bindings, ReadBinding},
    docking::{Axis, DockDrop, DockLayout, DockNode},
    input::{Direction, InputEvent, InputState},
    source::stable_id,
    theme::{EDITOR_CSS, EditorTheme},
    virtualize::{Rows, TreeItem, VirtualTree, VirtualView},
    widgets::Widget,
};
use ara_ui_text::{Clipboard, EditEvent, TextSystem};
#[derive(Default)]
struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, s: &str) -> Result<()> {
        self.0 = s.into();
        Ok(())
    }
}
#[test]
fn incremental_lanes_and_runtime_theme_switch() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='r'><button id='b' text='Resume' /><field id='f' text='Outpost' /></ui>",
        EDITOR_CSS,
        [400.0, 300.0],
    )?;
    ui.frame(&mut text)?;
    let b = ui.find("b").expect("fixture declares this value");
    let mut state = ui.nodes[b].state;
    state.focus = true;
    ui.set_state(b, state);
    let work = ui.frame(&mut text)?;
    assert_eq!(work.restyled, 1);
    assert_eq!(work.shaped, 0);
    assert_eq!(work.layout_passes, 0);
    ui.switch_theme(EditorTheme::Light)?;
    let work = ui.frame(&mut text)?;
    assert_eq!(work.shaped, 0);
    assert_eq!(work.layout_passes, 0);
    assert_eq!(
        ui.nodes[0].computed.background,
        ara_ui::style::Color(252, 252, 252, 255)
    );
    assert_eq!(ui.frame(&mut text)?.paint_nodes, 0);
    let error = Ui::editor(
        "<ui id='r' style='background:#000000' />",
        EDITOR_CSS,
        [400.0, 300.0],
    )
    .err()
    .expect("fixture declares this value");
    assert!(error.to_string().contains("ARA7018"));
    Ok(())
}
#[test]
fn hundred_thousand_tree_scroll_retains_shaped_survivors() -> Result<()> {
    let items = (0..100_000)
        .map(|i| TreeItem {
            key: i as u64,
            parent: if i % 1000 == 0 {
                None
            } else {
                Some(i / 1000 * 1000)
            },
            text: format!("Entity {i:06}"),
            expanded: true,
        })
        .collect();
    let mut tree = VirtualTree::new(items)?;
    assert_eq!(tree.len(), 100_000);
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='r'><scroll id='v' style='height:260px' /></ui>",
        EDITOR_CSS,
        [420.0, 300.0],
    )?;
    let v = ui.find("v").expect("fixture declares this value");
    let mut view = VirtualView::mount(&mut ui, v, 26.0, 260.0, vec![350.0])?;
    view.scroll(&mut ui, &tree, 0.0, 260.0)?;
    ui.frame(&mut text)?;
    let id = view.identity(1);
    let retained = ui.by_id(id).expect("fixture declares this value");
    view.scroll(&mut ui, &tree, 26.0, 260.0)?;
    let work = ui.frame(&mut text)?;
    assert_eq!(ui.by_id(id), Some(retained));
    assert_eq!(work.shaped, 1);
    assert!(ui.nodes.len() < 30);
    view.scroll(&mut ui, &tree, 50_000.5 * 26.0, 260.0)?;
    ui.frame(&mut text)?;
    let first = view.visible_nodes()[0];
    let viewport = ui.nodes[v].rect;
    assert!(
        (ui.nodes[first].rect.y - (viewport.y - 13.0)).abs() < 0.01,
        "fractional translation must survive entering-row restyle"
    );
    assert!(ui.hit_test(30.0, 290.0).is_none());
    tree.set_expanded(0, false)?;
    assert_eq!(tree.len(), 99_001);
    Ok(())
}
#[test]
fn focus_scope_capture_clipping_actions_and_input() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='r'><button id='a' nav-down='c' text='First' action='Go' /><button id='b' disabled='true' text='Disabled' /><field id='c' text='Name' action='Rename' /><modal id='m' style='position:absolute; left:20px; top:150px; width:200px; height:100px'><button id='ok' text='OK' /></modal></ui>",
        EDITOR_CSS,
        [400.0, 400.0],
    )?;
    ui.frame(&mut text)?;
    let mut input = InputState::default();
    let mut clip = Clip::default();
    input.push(&mut ui, stable_id("r"), Some(stable_id("a")), false)?;
    input.event(&mut ui, InputEvent::Tab { reverse: false }, &mut clip)?;
    assert_eq!(input.focused(), Some(stable_id("c")));
    input.focus(&mut ui, stable_id("a"))?;
    input.event(&mut ui, InputEvent::Navigate(Direction::Down), &mut clip)?;
    assert_eq!(input.focused(), Some(stable_id("c")));
    input.push(&mut ui, stable_id("m"), Some(stable_id("ok")), true)?;
    input.event(&mut ui, InputEvent::Tab { reverse: false }, &mut clip)?;
    assert_eq!(input.focused(), Some(stable_id("ok")));
    assert!(input.focus(&mut ui, stable_id("a")).is_err());
    input.pop(&mut ui);
    assert_eq!(input.focused(), Some(stable_id("c")));
    input.event(&mut ui, InputEvent::Edit(EditEvent::SelectAll), &mut clip)?;
    input.event(
        &mut ui,
        InputEvent::Edit(EditEvent::Insert("Ara".into())),
        &mut clip,
    )?;
    assert_eq!(
        ui.drain_actions()
            .next()
            .expect("fixture declares this value")
            .name,
        "Rename"
    );
    let r = ui.nodes[ui.find("a").expect("fixture declares this value")].rect;
    input.event(
        &mut ui,
        InputEvent::PointerDown([r.x + 5.0, r.y + 5.0]),
        &mut clip,
    )?;
    assert_eq!(input.captured(), Some(stable_id("a")));
    input.event(
        &mut ui,
        InputEvent::PointerUp([r.x + 5.0, r.y + 5.0]),
        &mut clip,
    )?;
    assert_eq!(
        ui.drain_actions()
            .next()
            .expect("fixture declares this value")
            .name,
        "Go"
    );
    let a11y = ara_ui::accessibility::accessibility(&ui, input.focused());
    assert_eq!(a11y.focus, accesskit::NodeId(stable_id("a")));
    assert!(ui.inspect()["focus_graph"].is_array());
    Ok(())
}
#[test]
fn docking_moves_splits_serializes_resets_and_rejects_collisions() -> Result<()> {
    let initial = DockNode::Area {
        id: "main".into(),
        tabs: vec!["Scene".into(), "Inspector".into(), "Assets".into()],
        active: 0,
    };
    let mut dock = DockLayout::new(initial.clone())?;
    dock.drop_tab(
        "Inspector",
        DockDrop::Split {
            area: "main".into(),
            new_area: "right".into(),
            axis: Axis::Horizontal,
            before: false,
        },
    )?;
    dock.drop_tab(
        "Assets",
        DockDrop::Tab {
            area: "right".into(),
            index: 1,
        },
    )?;
    let saved = dock.serialize()?;
    dock.reset();
    assert_eq!(dock.root(), &initial);
    dock.restore(&saved)?;
    assert_eq!(dock.serialize()?, saved);
    let before = dock.serialize()?;
    assert!(
        dock.drop_tab(
            "Assets",
            DockDrop::Split {
                area: "main".into(),
                new_area: "right".into(),
                axis: Axis::Vertical,
                before: true
            }
        )
        .is_err()
    );
    assert_eq!(dock.serialize()?, before);
    ara_ui_document::compile(&format!("<ui id='r'>{}</ui>", dock.markup()))?;
    Ok(())
}
#[test]
fn typed_notifications_read_only_subscribers_and_emit_actions() -> Result<()> {
    struct Model {
        reads: std::cell::Cell<usize>,
        name: String,
    }
    impl ReadBinding<String> for Model {
        fn read(&self, _: BindingKey<String>) -> Result<String> {
            self.reads.set(self.reads.get() + 1);
            Ok(self.name.clone())
        }
    }
    let mut ui = Ui::editor(
        "<ui id='r'><field id='name' text='initial' /><toggle id='enabled' checked='false' /><slider id='volume' min='0' max='10' step='2' value='4' /></ui>",
        EDITOR_CSS,
        [400.0, 300.0],
    )?;
    let model = Model {
        reads: std::cell::Cell::new(0),
        name: "Ara".into(),
    };
    let mut bindings = Bindings::default();
    let key = BindingKey::new(1);
    bindings.subscribe(key, stable_id("name"), &model, &mut ui)?;
    let mut text = TextSystem::bundled()?;
    ui.frame(&mut text)?;
    ui.frame(&mut text)?;
    assert_eq!(model.reads.get(), 1);
    assert_eq!(bindings.changed([key, key], &model, &mut ui)?, 1);
    assert_eq!(model.reads.get(), 2);
    let enabled = ui.find("enabled").expect("fixture declares this value");
    ui.set_checked(enabled, true)?;
    assert!(matches!(
        ui.widgets.get(&stable_id("enabled")),
        Some(Widget::Toggle(true))
    ));
    let volume = ui.find("volume").expect("fixture declares this value");
    ui.set_value(volume, 7.0)?;
    assert!(matches!(
        ui.widgets.get(&stable_id("volume")),
        Some(Widget::Slider { value: 8.0, .. })
    ));
    Ok(())
}

#[test]
fn choice_pages_popup_layers_and_rounded_clip_hits() -> Result<()> {
    use ara_ui::layers::{LayerKind, LayerStack};
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='r'><tabs id='pages'><panel id='p0'><button id='a' text='First page' /></panel><panel id='p1'><button id='b' text='Second page' /></panel></tabs><popup id='popup' class='card' style='position:absolute; width:160px; height:80px'><button id='menu-item' text='Select' /></popup><panel id='rounded' style='width:100px; height:100px; border-radius:40px; overflow:hidden'><button id='rounded-button' text='Clipped' style='width:100px; height:100px' /></panel></ui>",
        EDITOR_CSS,
        [400.0, 400.0],
    )?;
    let popup = ui.find("popup").expect("popup exists");
    ui.set_visible(popup, false)?;
    ui.frame(&mut text)?;
    assert!(ui.paint_order.contains(&ui.find("a").expect("first page")));
    assert!(!ui.paint_order.contains(&ui.find("b").expect("second page")));
    let tabs = ui.find("pages").expect("tabs");
    ui.select(tabs, 1)?;
    ui.frame(&mut text)?;
    assert!(!ui.paint_order.contains(&ui.find("a").expect("first page")));
    assert!(ui.paint_order.contains(&ui.find("b").expect("second page")));
    let r = ui.nodes[ui.find("rounded").expect("rounded")].rect;
    assert!(ui.hit_test(r.x + 1.0, r.y + 1.0).is_none());
    assert_eq!(
        ui.hit_test(r.x + 50.0, r.y + 50.0),
        ui.find("rounded-button")
    );
    let mut input = InputState::default();
    input.push(&mut ui, stable_id("r"), Some(stable_id("b")), false)?;
    let mut layers = LayerStack::default();
    layers.open(
        &mut ui,
        &mut input,
        stable_id("popup"),
        LayerKind::Popup,
        [20.0, 20.0],
        Some(stable_id("menu-item")),
    )?;
    ui.frame(&mut text)?;
    let mut clip = Clip::default();
    layers.event(&mut ui, &mut input, InputEvent::Cancel, &mut clip)?;
    ui.frame(&mut text)?;
    assert_eq!(input.focused(), Some(stable_id("b")));
    assert!(!ui.paint_order.contains(&popup));
    Ok(())
}
