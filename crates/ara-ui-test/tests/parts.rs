//! `Ui::reconcile_parts`: a host's markup split into a skeleton and parts (an
//! editor's dock areas). Unchanged parts keep their whole subtree, virtual rows and
//! live widget state included, while the rest reconciles as usual.
use ara_ui::{
    Result, Ui,
    input::{InputEvent, InputState},
    source::stable_id,
    theme::EDITOR_CSS,
    virtualize::{VirtualList, VirtualView},
};
use ara_ui_text::{Clipboard, EditEvent, TextSystem};

#[derive(Default)]
struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> Result<()> {
        self.0 = text.into();
        Ok(())
    }
}

const SKELETON: &str =
    "<ui id='app' style='flex-direction:row'><panel id='left' /><panel id='right' /></ui>";

fn left(title: &str) -> String {
    format!(
        "<panel id='left' style='width:200px'><text id='left-title' text='{title}' /><scroll id='rows' style='height:110px' /></panel>"
    )
}
fn right(label: &str) -> String {
    format!(
        "<panel id='right' style='flex-grow:1'><text id='label' text='{label}' /><field id='name' text='Outpost' action='Dispatch' /></panel>"
    )
}
fn rows() -> VirtualList {
    VirtualList {
        items: (0..50).map(|i| (i, format!("Row {i}"))).collect(),
    }
}
fn reconcile(ui: &mut Ui, left_title: &str, right_label: &str) -> Result<ara_ui::Reconciled> {
    let (l, r) = (left(left_title), right(right_label));
    ui.reconcile_parts(SKELETON, &[("left", &l), ("right", &r)])
}
/// A Ui with both parts mounted and 50 virtual rows scrolled to row 10.
fn mounted(text: &mut TextSystem) -> Result<(Ui, VirtualView)> {
    let mut ui = Ui::editor("<ui id='app' />", EDITOR_CSS, [640., 400.])?;
    reconcile(&mut ui, "Hierarchy", "A")?;
    ui.frame(text)?;
    let parent = ui.find("rows").ok_or("rows")?;
    let mut view = VirtualView::mount(&mut ui, parent, 22., 110., vec![180.])?;
    view.scroll(&mut ui, &rows(), 220., 110.)?;
    ui.frame(text)?;
    Ok((ui, view))
}
fn row_texts(ui: &Ui, view: &VirtualView) -> Vec<String> {
    view.visible_nodes()
        .iter()
        .map(|i| {
            let cell = ui.nodes[*i].element.children[0];
            ui.nodes[cell]
                .text
                .as_ref()
                .map(|t| t.value.clone())
                .unwrap_or_default()
        })
        .collect()
}

#[test]
fn unchanged_parts_keep_their_subtree_and_virtual_rows() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let (mut ui, mut view) = mounted(&mut text)?;
    let before = row_texts(&ui, &view);
    assert_eq!(before.first().map(String::as_str), Some("Row 10"));
    let pool = ui.nodes.len();

    // Only the right part changes, keeping its shape: its nodes update in place and
    // the left part (with its rows) is not touched.
    let report = reconcile(&mut ui, "Hierarchy", "B")?;
    assert_eq!(
        (report.added, report.removed, report.moved),
        (0, 0, 0),
        "{report:?}"
    );
    assert!(report.changed >= 1, "{report:?}");
    assert_eq!(ui.nodes.len(), pool, "the virtual rows survive");
    assert!(view.remap(&ui), "every row handle follows its node");
    assert_eq!(row_texts(&ui, &view), before);
    let label = ui.find("label").ok_or("label")?;
    assert_eq!(
        ui.nodes[label].text.as_ref().map(|t| t.value.as_str()),
        Some("B")
    );
    // The kept view still scrolls, and the next frame lays out normally.
    view.scroll(&mut ui, &rows(), 440., 110.)?;
    ui.frame(&mut text)?;
    assert_eq!(
        row_texts(&ui, &view).first().map(String::as_str),
        Some("Row 20")
    );
    let rows = ui.find("rows").ok_or("rows")?;
    assert!(ui.nodes[rows].rect.h > 100.);
    Ok(())
}

#[test]
fn a_reshaped_part_reconciles_and_the_rest_moves_wholesale() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let (mut ui, mut view) = mounted(&mut text)?;
    let before = row_texts(&ui, &view);
    // The right part gains a node: the rows appended after it move.
    let reshaped = "<panel id='right' style='flex-grow:1'><text id='label' text='A' /><text id='extra' text='New' /><field id='name' text='Outpost' action='Dispatch' /></panel>";
    let report = ui.reconcile_parts(
        SKELETON,
        &[("left", &left("Hierarchy")), ("right", reshaped)],
    )?;
    assert_eq!(report.added, 1, "{report:?}");
    assert!(report.moved > 0, "{report:?}");
    assert!(view.remap(&ui));
    assert_eq!(row_texts(&ui, &view), before);
    ui.frame(&mut text)?;
    assert!(ui.find("extra").is_some());
    Ok(())
}

#[test]
fn changed_parts_reconcile_like_markup() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let (mut ui, mut view) = mounted(&mut text)?;
    let report = reconcile(&mut ui, "Scene", "A")?;
    // The left part is matched node by node: its rows were runtime-appended, so they go.
    assert!(report.changed >= 1 && report.removed > 0, "{report:?}");
    assert!(!view.remap(&ui), "a recompiled part drops its rows");
    let title = ui.find("left-title").ok_or("title")?;
    assert_eq!(
        ui.nodes[title].text.as_ref().map(|t| t.value.as_str()),
        Some("Scene")
    );
    ui.frame(&mut text)?;
    Ok(())
}

#[test]
fn an_unchanged_call_touches_nothing() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let (mut ui, view) = mounted(&mut text)?;
    let nodes = ui.nodes.len();
    let report = reconcile(&mut ui, "Hierarchy", "A")?;
    assert_eq!(report.moved, 0, "{report:?}");
    assert!(report.unchanged());
    assert_eq!(ui.nodes.len(), nodes);
    for index in view.visible_nodes() {
        assert_eq!(ui.moved_index(*index), Some(*index));
    }
    let work = ui.frame(&mut text)?;
    assert_eq!((work.restyled, work.layout_passes), (0, 0), "{work:?}");
    Ok(())
}

#[test]
fn rejected_parts_leave_the_tree_intact() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let (mut ui, _) = mounted(&mut text)?;
    let nodes = ui.nodes.len();
    let wrong = "<panel id='elsewhere' />";
    assert!(
        ui.reconcile_parts(SKELETON, &[("left", &left("Hierarchy")), ("right", wrong)])
            .is_err()
    );
    let missing = right("A");
    assert!(
        ui.reconcile_parts(
            SKELETON,
            &[("left", &left("Hierarchy")), ("nowhere", &missing)]
        )
        .is_err()
    );
    assert_eq!(ui.nodes.len(), nodes);
    assert!(ui.find("label").is_some() && ui.find("rows").is_some());
    // The next call starts over cleanly.
    reconcile(&mut ui, "Hierarchy", "C")?;
    ui.frame(&mut text)?;
    let label = ui.find("label").ok_or("label")?;
    assert_eq!(
        ui.nodes[label].text.as_ref().map(|t| t.value.as_str()),
        Some("C")
    );
    Ok(())
}

#[test]
fn a_part_hidden_by_its_skeleton_stays_mounted() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let (mut ui, mut view) = mounted(&mut text)?;
    let before = row_texts(&ui, &view);
    // The left part sits in a slot the skeleton shows or hides (an inactive tab).
    let mut slot = |ui: &mut Ui, display: &str| {
        let skeleton = format!(
            "<ui id='app' style='flex-direction:row'><panel id='slot' style='display:{display}'><panel id='left' /></panel><panel id='right' /></ui>"
        );
        let report = ui.reconcile_parts(
            &skeleton,
            &[("left", &left("Hierarchy")), ("right", &right("A"))],
        );
        ui.frame(&mut text)?;
        report
    };
    slot(&mut ui, "flex")?;
    // Handles follow every reconcile (the slot shifted the part's indices).
    assert!(view.remap(&ui));
    let pool = ui.nodes.len();

    let report = slot(&mut ui, "none")?;
    assert_eq!((report.added, report.removed), (0, 0), "{report:?}");
    assert_eq!(ui.nodes.len(), pool, "the hidden part keeps its nodes");
    assert!(view.remap(&ui));
    let (rows, label) = (
        ui.find("rows").ok_or("rows")?,
        ui.find("label").ok_or("label")?,
    );
    assert!(!ui.shown(rows) && ui.shown(label));
    assert!(
        !ui.paint_order.contains(&rows),
        "hidden nodes are not painted or hit"
    );
    let right_x = ui.find("right").map(|i| ui.nodes[i].rect.x);
    assert!(
        right_x.is_some_and(|x| x < 20.),
        "the hidden slot takes no space: {right_x:?}"
    );

    // Showing it again is a style change: the rows are as they were.
    let report = slot(&mut ui, "flex")?;
    assert_eq!((report.added, report.removed), (0, 0), "{report:?}");
    assert!(view.remap(&ui));
    assert_eq!(row_texts(&ui, &view), before);
    let rows = ui.find("rows").ok_or("rows")?;
    assert!(ui.shown(rows) && ui.nodes[rows].rect.h > 100.);
    Ok(())
}

#[test]
fn a_new_skeleton_moves_kept_parts() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let (mut ui, mut view) = mounted(&mut text)?;
    let left_x = |ui: &Ui| ui.find("left").map(|i| ui.nodes[i].rect.x);
    assert!(left_x(&ui).is_some_and(|x| x < 100.), "{:?}", left_x(&ui));
    let swapped =
        "<ui id='app' style='flex-direction:row'><panel id='right' /><panel id='left' /></ui>";
    let report = ui.reconcile_parts(
        swapped,
        &[("left", &left("Hierarchy")), ("right", &right("A"))],
    )?;
    assert_eq!(report.added + report.removed, 0, "{report:?}");
    assert!(view.remap(&ui));
    ui.frame(&mut text)?;
    assert!(left_x(&ui).is_some_and(|x| x > 400.), "{:?}", left_x(&ui));
    assert_eq!(
        row_texts(&ui, &view).first().map(String::as_str),
        Some("Row 10")
    );
    Ok(())
}

#[test]
fn live_widget_state_survives_in_kept_parts() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let (mut ui, _) = mounted(&mut text)?;
    let mut input = InputState::default();
    let mut clip = Clip::default();
    let field = ui.find("name").ok_or("field")?;
    let r = ui.nodes[field].rect;
    let at = [r.x + r.w * 0.5, r.y + r.h * 0.5];
    input.event(&mut ui, InputEvent::PointerDown(at), &mut clip)?;
    input.event(&mut ui, InputEvent::PointerUp(at), &mut clip)?;
    input.event(
        &mut ui,
        InputEvent::Edit(EditEvent::Insert("X".into())),
        &mut clip,
    )?;
    let typed = |ui: &Ui| match ui.widgets.get(&stable_id("name")) {
        Some(ara_ui::widgets::Widget::Input(input)) => input.value().to_owned(),
        _ => String::new(),
    };
    let value = typed(&ui);
    assert!(value.contains('X'), "{value}");
    // The left part changes; the right part (with the edited field) is kept.
    reconcile(&mut ui, "Scene", "A")?;
    assert_eq!(typed(&ui), value);
    assert_eq!(input.focused(), Some(stable_id("name")));
    ui.frame(&mut text)?;
    // The right part's label changes in place: the field keeps its edit and focus.
    reconcile(&mut ui, "Scene", "B")?;
    assert_eq!(typed(&ui), value);
    assert_eq!(input.focused(), Some(stable_id("name")));
    ui.frame(&mut text)?;

    // Hidden (an inactive tab), the field keeps its edit but not the focus: typing
    // no longer reaches it.
    let hidden = "<ui id='app' style='flex-direction:row'><panel id='left' /><panel id='slot' style='display:none'><panel id='right' /></panel></ui>";
    ui.reconcile_parts(hidden, &[("left", &left("Scene")), ("right", &right("B"))])?;
    ui.frame(&mut text)?;
    input.event(
        &mut ui,
        InputEvent::Edit(EditEvent::Insert("Y".into())),
        &mut clip,
    )?;
    assert_eq!(input.focused(), None);
    assert_eq!(typed(&ui), value);
    assert!(input.focus(&mut ui, stable_id("name")).is_err());
    Ok(())
}

#[test]
fn kept_views_fit_a_taller_or_narrower_host() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let (mut ui, mut view) = mounted(&mut text)?;
    reconcile(&mut ui, "Hierarchy", "B")?;
    assert!(view.remap(&ui));
    // The host grew to 220 px and narrowed to 150 px: more rows, narrower cells.
    view.fit(&mut ui, 220., vec![150.])?;
    view.scroll(&mut ui, &rows(), 220., 220.)?;
    ui.frame(&mut text)?;
    let texts = row_texts(&ui, &view);
    assert_eq!(texts.len(), 12, "{texts:?}");
    assert_eq!(texts.first().map(String::as_str), Some("Row 10"));
    let cell = ui.nodes[view.visible_nodes()[0]].element.children[0];
    assert!(
        (ui.nodes[cell].rect.w - 150.).abs() < 0.5,
        "{:?}",
        ui.nodes[cell].rect
    );
    assert!(
        view.fit(&mut ui, 220., vec![150., 40.]).is_err(),
        "columns stay fixed"
    );
    Ok(())
}
