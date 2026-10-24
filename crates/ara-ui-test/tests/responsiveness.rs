//! Responsiveness regressions through the real runtime: a host that rebuilds its
//! markup on every model change (`Ui::reconcile`), clicks delivered through
//! `InputState` with rebuilds and batched (slow-frame) input between press and
//! release, tabs, a menu, notifications dismissed through the UI, and the cost of
//! idle frames, unchanged rebuilds and a theme switch.
use ara_ui::{
    Result, Ui,
    input::{InputEvent, InputState},
    source::stable_id,
    theme::{EDITOR_CSS, EditorTheme},
};
use ara_ui_text::{Clipboard, TextSystem};

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

/// A small app in the shape of the Hub and the editor: its markup is a function of
/// its model, rebuilt after every model change.
struct Host {
    ui: Ui,
    input: InputState,
    text: TextSystem,
    clip: Clip,
    /// Changes every frame, like a status line or a job progress readout.
    ticks: u32,
    /// A banner above everything, toggled to move the controls below it.
    banner: bool,
    tab: usize,
    menu: bool,
    notices: Vec<u32>,
    commands: Vec<String>,
}

impl Host {
    fn new() -> Result<Self> {
        let mut host = Self {
            ui: Ui::editor("<ui id='app' />", EDITOR_CSS, [640., 480.])?,
            input: InputState::default(),
            text: TextSystem::bundled()?,
            clip: Clip::default(),
            ticks: 0,
            banner: false,
            tab: 0,
            menu: false,
            notices: vec![1, 2, 3],
            commands: Vec::new(),
        };
        host.frame()?;
        Ok(host)
    }

    fn markup(&self) -> String {
        let mut m = format!(
            "<ui id='app'><text id='status' text='Working… {}' />",
            self.ticks
        );
        if self.banner {
            m.push_str("<panel id='banner' style='height:48px'><text id='banner-text' text='Update ready' /></panel>");
        }
        m.push_str("<panel id='tabs' style='flex-direction:row'>");
        for (i, name) in ["Projects", "Installs", "Learn"].iter().enumerate() {
            let class = if i == self.tab { "selected" } else { "" };
            m.push_str(&format!(
                "<button id='tab-{i}' class='{class}' text='{name}' action='Dispatch' />"
            ));
        }
        m.push_str("</panel><button id='menu-title' text='File' action='Dispatch' />");
        if self.menu {
            m.push_str("<panel id='menu'><button id='menu-new' text='New Project' action='Dispatch' /><button id='menu-open' text='Open…' action='Dispatch' /></panel>");
        }
        m.push_str("<panel id='notices'>");
        for n in &self.notices {
            m.push_str(&format!("<panel id='notice-{n}' style='flex-direction:row'><text id='notice-{n}-text' text='Notice {n}' /><button id='notice-{n}-close' text='Dismiss' action='Dispatch' /></panel>"));
        }
        m.push_str("</panel></ui>");
        m
    }

    /// One host frame: apply actions to the model, rebuild the markup, lay out, paint.
    fn frame(&mut self) -> Result<ara_ui::Work> {
        let actions: Vec<_> = self.ui.drain_actions().collect();
        for action in actions {
            let id = self
                .ui
                .authored_id(action.node)
                .ok_or("an action from an unknown node")?
                .to_owned();
            if let Some(tab) = id.strip_prefix("tab-") {
                self.tab = tab.parse()?;
            } else if id == "menu-title" {
                self.menu = !self.menu;
            } else if let Some(command) = id.strip_prefix("menu-") {
                self.commands.push(command.into());
                self.menu = false;
            } else if let Some(n) = id
                .strip_prefix("notice-")
                .and_then(|s| s.strip_suffix("-close"))
            {
                let n: u32 = n.parse()?;
                self.notices.retain(|m| *m != n);
            }
        }
        self.ticks += 1;
        self.ui.reconcile(&self.markup())?;
        self.ui.frame(&mut self.text)
    }

    fn centre(&self, id: &str) -> Result<[f32; 2]> {
        let r = self.ui.nodes[self.ui.find(id).ok_or("no such control")?].rect;
        Ok([r.x + r.w * 0.5, r.y + r.h * 0.5])
    }

    fn event(&mut self, event: InputEvent) -> Result<()> {
        self.input.event(&mut self.ui, event, &mut self.clip)
    }

    /// Press, run `between` host frames, release (0: one batch, as after a slow
    /// frame), then one more frame for the action.
    fn click(&mut self, id: &str, between: usize) -> Result<()> {
        let at = self.centre(id)?;
        self.event(InputEvent::PointerMove(at))?;
        self.event(InputEvent::PointerDown(at))?;
        for _ in 0..between {
            self.frame()?;
        }
        self.event(InputEvent::PointerUp(at))?;
        self.frame()?;
        Ok(())
    }
}

#[test]
fn clicks_land_first_time_across_rebuilds_and_batched_input() -> Result<()> {
    let mut host = Host::new()?;
    for between in [0, 1, 3] {
        for tab in [1, 2, 0, 2] {
            host.click(&format!("tab-{tab}"), between)?;
            assert_eq!(host.tab, tab, "tab {tab} with {between} frames between");
        }
    }
    // The layout moves between press and release (a banner appears above): the
    // press still activates the control it was on.
    let at = host.centre("tab-1")?;
    host.event(InputEvent::PointerDown(at))?;
    host.banner = true;
    host.frame()?;
    assert_ne!(host.centre("tab-1")?, at, "the tab moved under the pointer");
    host.event(InputEvent::PointerUp(at))?;
    host.frame()?;
    assert_eq!(host.tab, 1);
    Ok(())
}

#[test]
fn menus_open_close_and_run_rows_on_the_first_click() -> Result<()> {
    let mut host = Host::new()?;
    host.click("menu-title", 1)?;
    assert!(host.menu, "the title opens its menu");
    host.click("menu-title", 0)?;
    assert!(!host.menu, "the title closes its open menu");
    host.click("menu-title", 0)?;
    host.click("menu-open", 2)?;
    assert_eq!(host.commands, ["open"]);
    assert!(!host.menu, "running a row closes the menu");
    Ok(())
}

#[test]
fn notifications_dismiss_through_the_ui_one_click_each() -> Result<()> {
    let mut host = Host::new()?;
    // Dismissing the first moves the others up; each still takes one click.
    host.click("notice-1-close", 1)?;
    assert_eq!(host.notices, [2, 3]);
    host.click("notice-2-close", 0)?;
    assert_eq!(host.notices, [3]);
    // Hover survives the rebuilds that follow each dismissal.
    let at = host.centre("notice-3-close")?;
    host.event(InputEvent::PointerMove(at))?;
    host.frame()?;
    let close = host.ui.find("notice-3-close").ok_or("close button")?;
    assert!(host.ui.nodes[close].state.hover);
    host.click("notice-3-close", 3)?;
    assert!(host.notices.is_empty());
    assert!(host.ui.find("notice-3").is_none());
    Ok(())
}

#[test]
fn unchanged_rebuilds_and_idle_frames_do_no_work() -> Result<()> {
    let mut host = Host::new()?;
    let markup = host.markup();
    let report = host.ui.reconcile(&markup)?;
    assert!(report.unchanged(), "{report:?}");
    let idle = host.ui.frame(&mut host.text)?;
    assert_eq!(idle, ara_ui::Work::default(), "nothing to redraw");
    // The status line changes: one paragraph reshapes, nothing restyles.
    let work = host.frame()?;
    assert_eq!(work.shaped, 1, "{work:?}");
    assert_eq!(work.restyled, 0, "{work:?}");
    assert_eq!(work.layout_passes, 1, "{work:?}");
    Ok(())
}

#[test]
fn theme_switch_is_one_restyle_without_layout_or_shaping() -> Result<()> {
    let mut host = Host::new()?;
    host.notices = (0..200).collect();
    host.frame()?;
    let nodes = host.ui.nodes.len();
    host.ui.switch_theme(EditorTheme::Light)?;
    let started = std::time::Instant::now();
    let work = host.ui.frame(&mut host.text)?;
    let elapsed = started.elapsed();
    assert_eq!(work.restyled, nodes, "every node once");
    assert_eq!(work.shaped, 0, "colors never reshape");
    assert_eq!(work.layout_passes, 0, "colors never relayout");
    assert!(work.paint_nodes > 0);
    // Generous for the unoptimized test profile on a loaded machine; the release
    // editor measures a theme switch in `--verify-responsiveness`.
    assert!(
        elapsed < std::time::Duration::from_millis(250),
        "{elapsed:?} for {nodes} nodes"
    );
    let background = |ui: &Ui| {
        let i = ui.find("app").expect("root");
        ui.nodes[i].computed.background
    };
    let light = background(&host.ui);
    // The same theme again is free.
    host.ui.switch_theme(EditorTheme::Light)?;
    assert_eq!(host.ui.frame(&mut host.text)?, ara_ui::Work::default());
    host.ui.switch_theme(EditorTheme::Dark)?;
    host.ui.frame(&mut host.text)?;
    assert_ne!(background(&host.ui), light);
    assert_eq!(
        host.ui.by_id(stable_id("notice-199-close")),
        host.ui.find("notice-199-close")
    );
    Ok(())
}

/// A host echoing a field's edited value into its markup on every keystroke keeps the
/// live input: a mid-string caret, typing and undo carry on across reconciles.
#[test]
fn echoed_field_values_keep_caret_and_undo() -> Result<()> {
    use ara_ui_text::EditEvent;
    let markup = |value: &str| {
        format!("<ui id='app'><field id='name' text='{value}' action='Dispatch' /></ui>")
    };
    let mut text = TextSystem::bundled()?;
    let mut clip = Clip::default();
    let mut input = InputState::default();
    let mut ui = Ui::editor(&markup("Outpost"), EDITOR_CSS, [400., 200.])?;
    ui.frame(&mut text)?;
    let field = ui.find("name").ok_or("field")?;
    let r = ui.nodes[field].rect;
    let at = [r.x + r.w * 0.5, r.y + r.h * 0.5];
    input.event(&mut ui, InputEvent::PointerDown(at), &mut clip)?;
    input.event(&mut ui, InputEvent::PointerUp(at), &mut clip)?;
    input.event(
        &mut ui,
        InputEvent::Edit(EditEvent::Select {
            anchor: 3,
            caret: 3,
        }),
        &mut clip,
    )?;
    let mut type_and_echo = |ui: &mut Ui, edit: EditEvent| -> Result<String> {
        input.event(ui, InputEvent::Edit(edit), &mut clip)?;
        let value = ui
            .drain_actions()
            .last()
            .and_then(|a| a.value.as_str().map(str::to_owned))
            .ok_or("an edit reports its value")?;
        // The model takes the value and the whole markup is rebuilt from it.
        ui.reconcile(&markup(&value))?;
        ui.frame(&mut text)?;
        Ok(value)
    };
    let caret = |ui: &Ui| match ui.widgets.get(&stable_id("name")) {
        Some(ara_ui::widgets::Widget::Input(input)) => input.caret(),
        _ => usize::MAX,
    };
    assert_eq!(
        type_and_echo(&mut ui, EditEvent::Insert("X".into()))?,
        "OutXpost"
    );
    assert_eq!(caret(&ui), 4, "the caret stays mid-string after the echo");
    assert_eq!(
        type_and_echo(&mut ui, EditEvent::Insert("Y".into()))?,
        "OutXYpost"
    );
    let undone = type_and_echo(&mut ui, EditEvent::Undo)?;
    assert!(undone == "OutXpost" || undone == "Outpost", "{undone}");
    // A model value that differs from the live text still wins.
    ui.reconcile(&markup("Hangar"))?;
    ui.frame(&mut text)?;
    let shown = ui.nodes[ui.find("name").ok_or("field")?]
        .text
        .as_ref()
        .map(|t| t.value.clone());
    assert_eq!(shown.as_deref(), Some("Hangar"));
    Ok(())
}

/// Removing a node moves its later siblings up (Taffy drops removed children from
/// their parents itself, so a removal alone must still lay out again).
#[test]
fn removals_release_their_layout_space() -> Result<()> {
    let markup = |first: bool| {
        format!(
            "<ui id='app' style='flex-direction:column;padding:0px;gap:0px'>{}<panel id='second' style='height:40px' /></ui>",
            if first {
                "<panel id='first' style='height:100px' />"
            } else {
                ""
            }
        )
    };
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(&markup(true), EDITOR_CSS, [400., 300.])?;
    ui.frame(&mut text)?;
    let y = |ui: &Ui| ui.nodes[ui.find("second").expect("second")].rect.y;
    assert_eq!(y(&ui), 100.);
    let report = ui.reconcile(&markup(false))?;
    assert_eq!(report.removed, 1, "{report:?}");
    let work = ui.frame(&mut text)?;
    assert_eq!(work.layout_passes, 1, "{work:?}");
    assert_eq!(y(&ui), 0., "the second panel moves up");
    // And back: an insertion pushes it down again.
    ui.reconcile(&markup(true))?;
    ui.frame(&mut text)?;
    assert_eq!(y(&ui), 100.);
    Ok(())
}

/// A reconcile is all or nothing: an editor raw color (D18), a bad inline style or an
/// invalid widget value in new or restyled nodes is rejected and the tree stays as it
/// was; unchanged nodes are not re-validated.
#[test]
fn rejected_reconciles_leave_the_tree_unchanged() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let base = "<ui id='app'><panel id='a' style='height:10px' /><button id='b' text='Go' /></ui>";
    let mut ui = Ui::editor(base, EDITOR_CSS, [200., 100.])?;
    ui.frame(&mut text)?;
    let before = ui.nodes.len();
    for bad in [
        "<ui id='app'><panel id='a' style='background:#ff0000' /><button id='b' text='Go' /></ui>",
        "<ui id='app'><panel id='a' style='height:10px' /><panel id='c' style='color:rgb(1,2,3)' /></ui>",
        "<ui id='app'><panel id='a' style='height:ten' /><button id='b' text='Go' /></ui>",
        "<ui id='app'><panel id='a' style='height:10px' /><slider id='s' min='2' max='1' /></ui>",
    ] {
        assert!(ui.reconcile(bad).is_err(), "{bad}");
        assert_eq!(ui.nodes.len(), before);
        assert!(ui.find("b").is_some());
    }
    assert!(ui.reconcile(base)?.unchanged());
    assert_eq!(ui.frame(&mut text)?, ara_ui::Work::default());
    Ok(())
}
