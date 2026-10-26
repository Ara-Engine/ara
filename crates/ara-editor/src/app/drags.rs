//! SDL-level dock gestures, shared by `--verify-ux` and the focused `--verify-docking`.
//! Desktop points are synthetic; all input still passes through the native event router.
use super::*;
use ara_ui::{
    docking::{Axis, DockNode},
    paint::Box2,
};

#[derive(Clone, Copy, Debug)]
enum Zone {
    Before,
    After,
    Centre,
    Left,
    Right,
    Top,
    Bottom,
}
#[derive(Clone, Debug)]
enum Act {
    Splitter(String),
    Scale(f64),
    TearOff(&'static str),
    Place {
        panel: &'static str,
        onto: &'static str,
        zone: Zone,
    },
    Cancel(&'static str),
}
const SETTLE: u32 = 12;
const SPLIT_DRAG: f32 = 48.;

pub(super) struct DragCheck {
    output: PathBuf,
    acts: Vec<Act>,
    index: usize,
    age: u32,
    start: Option<([f32; 2], u32)>,
    before: f32,
    layout_before: String,
    trace: Vec<Value>,
    failed: bool,
    results: Vec<Value>,
}
fn tab_rect(shell: &Shell, panel: &str) -> Option<Box2> {
    [format!("tab-{panel}"), format!("header-{panel}")]
        .iter()
        .find_map(|id| shell.shown_node(id))
        .map(|i| shell.ui.nodes[i].rect)
}
fn centre(r: Box2) -> [f32; 2] {
    [r.x + r.w / 2., r.y + r.h / 2.]
}
fn area<'a>(node: &'a DockNode, panel: &str) -> Option<&'a [String]> {
    match node {
        DockNode::Area { tabs, .. } => tabs.iter().any(|p| p == panel).then_some(tabs),
        DockNode::Split { first, second, .. } => area(first, panel).or_else(|| area(second, panel)),
    }
}
fn split(node: &DockNode, panel: &str, onto: &str, zone: Zone) -> bool {
    let DockNode::Split {
        axis,
        first,
        second,
        ..
    } = node
    else {
        return false;
    };
    let before = matches!(zone, Zone::Left | Zone::Top);
    let expected = if matches!(zone, Zone::Top | Zone::Bottom) {
        Axis::Vertical
    } else {
        Axis::Horizontal
    };
    let (added, old) = if before {
        (&**first, &**second)
    } else {
        (&**second, &**first)
    };
    (*axis == expected
        && matches!(added, DockNode::Area { tabs, .. } if tabs == &[panel])
        && area(old, onto).is_some())
        || split(first, panel, onto, zone)
        || split(second, panel, onto, zone)
}
impl DragCheck {
    pub fn new(output: &std::path::Path, shell: &Shell) -> Self {
        let mut acts: Vec<Act> = shell
            .ui
            .nodes
            .iter()
            .filter(|n| n.element.id.starts_with("splitter-root") && n.visible)
            .map(|n| Act::Splitter(n.element.id.clone()))
            .collect();
        for scale in [1., 1.25] {
            acts.extend([
                Act::Scale(scale),
                Act::Place {
                    panel: "console",
                    onto: "game",
                    zone: Zone::After,
                },
                Act::Place {
                    panel: "game",
                    onto: "scene",
                    zone: Zone::Before,
                },
                Act::Place {
                    panel: "console",
                    onto: "game",
                    zone: Zone::Right,
                },
                Act::TearOff("assets"),
                Act::Place {
                    panel: "assets",
                    onto: "game",
                    zone: Zone::Left,
                },
                Act::TearOff("assets"),
                Act::TearOff("console"),
                Act::Place {
                    panel: "console",
                    onto: "assets",
                    zone: Zone::After,
                },
                Act::Place {
                    panel: "console",
                    onto: "game",
                    zone: Zone::Centre,
                },
                Act::Place {
                    panel: "assets",
                    onto: "game",
                    zone: Zone::After,
                },
                Act::Place {
                    panel: "console",
                    onto: "game",
                    zone: Zone::Top,
                },
                Act::Place {
                    panel: "console",
                    onto: "game",
                    zone: Zone::Bottom,
                },
                Act::Cancel("assets"),
            ]);
        }
        acts.push(Act::Scale(1.));
        Self {
            output: output.into(),
            acts,
            index: 0,
            age: 0,
            start: None,
            before: 0.,
            layout_before: String::new(),
            trace: Vec::new(),
            failed: false,
            results: Vec::new(),
        }
    }
    pub fn finished(&self) -> bool {
        self.index >= self.acts.len()
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|r| r["ok"] != true)
            .map(|r| r["drag"].as_str().unwrap_or("?").to_owned())
            .collect()
    }
    #[allow(clippy::too_many_arguments)]
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        floats: &windows::Floats,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let Some(act) = self.acts.get(self.index).cloned() else {
            return Ok(());
        };
        if let Err(error) = self.input(&act, editor, shell, window, floats, scale, events) {
            shell.leave();
            events.retain(|(_, e)| !matches!(e, Event::MouseButton { .. }));
            events.push((
                0,
                Event::Key {
                    scancode: 41,
                    pressed: true,
                    repeat: false,
                },
            ));
            shell.desktop_pointer = None;
            self.results
                .push(json!({"drag": format!("{act:?}"), "capturePrefix":format!("dock-{:02}",self.index), "ok": false, "error": error.to_string()}));
            self.advance();
            self.failed = true;
        }
        Ok(())
    }
    /// Find a panel's client-space point and map it to desktop points.
    #[allow(clippy::too_many_arguments)]
    fn point(
        &self,
        shell: &mut Shell,
        editor: &Editor,
        floats: &windows::Floats,
        main: [i32; 4],
        scale: f32,
        panel: &str,
        zone: Option<Zone>,
    ) -> ara_ui::Result<([f32; 2], u32, [f32; 2], f32)> {
        let float = floats.windows.iter().find(|f| {
            editor
                .state
                .floating
                .iter()
                .any(|s| s.id == f.id && area(&s.root, panel).is_some())
        });
        let (origin, sdl, scale) = if let Some(f) = float {
            if !shell.enter(&f.id) {
                return Err("drag surface missing".into());
            }
            (
                f.rect,
                f.sdl,
                crate::native::points_to_logical(
                    f.window.raw(),
                    f.window.display_scale() * windows::ui_scale(editor),
                ),
            )
        } else {
            shell.leave();
            (main, 0, scale)
        };
        let tab = tab_rect(shell, panel).ok_or_else(|| format!("no `{panel}` tab on surface"))?;
        let point = match zone {
            None => [tab.x + tab.w.min(96.) * 0.5, tab.y + tab.h * 0.5],
            Some(Zone::Before) => [tab.x + 2., tab.y + tab.h * 0.5],
            Some(Zone::After) => [tab.x + tab.w.min(120.) - 2., tab.y + tab.h * 0.5],
            Some(zone) => {
                let node = shell
                    .ui
                    .paint_order
                    .iter()
                    .find_map(|&i| {
                        let n = &shell.ui.nodes[i];
                        (n.element.id.starts_with("area-")
                            && n.element
                                .attrs
                                .get("class")
                                .is_some_and(|c| c.split_whitespace().any(|c| c == "dock"))
                            && n.rect.contains(tab.x + 2., tab.y + 2.))
                        .then_some(n)
                    })
                    .ok_or("drag destination area missing")?;
                let r = node.rect;
                match zone {
                    Zone::Centre => centre(r),
                    Zone::Left => [r.x + 4., r.y + r.h * 0.5],
                    Zone::Right => [r.x + r.w - 4., r.y + r.h * 0.5],
                    Zone::Top => [r.x + r.w * 0.5, r.y + 32.],
                    Zone::Bottom => [r.x + r.w * 0.5, r.y + r.h - 4.],
                    Zone::Before | Zone::After => unreachable!("tab zones handled above"),
                }
            }
        };
        shell.leave();
        Ok((
            point,
            sdl,
            [
                origin[0] as f32 + point[0] / scale,
                origin[1] as f32 + point[1] / scale,
            ],
            scale,
        ))
    }
    #[allow(clippy::too_many_arguments)]
    fn input(
        &mut self,
        act: &Act,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        floats: &windows::Floats,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let main = window.rect()?;
        let motion = |sdl: u32, p: [f32; 2], s: f32| {
            (
                sdl,
                Event::MouseMotion {
                    position: p.map(|v| v / s),
                    delta: [0., 0.],
                },
            )
        };
        let button = |sdl, pressed| (sdl, Event::MouseButton { button: 1, pressed });
        match (act, self.age) {
            (Act::Scale(value), 0) => {
                editor.action("editor.layout.reset", json!({}))?;
                editor.action("editor.layout.show", json!({"panel":"game"}))?;
                editor.action(
                    "editor.layout.collapse",
                    json!({"area":"bottom","collapsed":false}),
                )?;
                editor.action(
                    "editor.settings.set",
                    json!({"key":"appearance.uiScale","value":value}),
                )?;
            }
            (Act::Scale(_), 2) => events.push(motion(0, [8., 8.], scale)),
            (Act::Splitter(id), 0) => {
                let i = shell.shown_node(id).ok_or("splitter missing")?;
                let p = centre(shell.ui.nodes[i].rect);
                self.before = self.extent(shell, id)?;
                self.start = Some((p, 0));
                events.push(motion(0, p, scale));
            }
            (Act::Splitter(_), 1) => events.push(button(0, true)),
            (Act::Splitter(id), n @ 2..=5) => {
                let (mut p, _) = self.start.ok_or("splitter start missing")?;
                p[usize::from(!self.across(shell, id))] += SPLIT_DRAG * (n - 1) as f32 / 4.;
                events.push(motion(0, p, scale));
            }
            (Act::Splitter(_), 6) => events.push(button(0, false)),
            (Act::TearOff(panel) | Act::Place { panel, .. } | Act::Cancel(panel), 0) => {
                self.layout_before = editor.state.dock()?.serialize()?;
                let (point, sdl, desktop, scale) =
                    self.point(shell, editor, floats, main, scale, panel, None)?;
                self.start = Some((point, sdl));
                shell.desktop_pointer = Some(desktop);
                events.push(motion(sdl, point, scale));
            }
            (Act::TearOff(_) | Act::Place { .. } | Act::Cancel(_), 1) => {
                let (_, sdl) = self.start.ok_or("tab start missing")?;
                events.push(button(sdl, true));
            }
            (Act::TearOff(_) | Act::Place { .. } | Act::Cancel(_), n @ 2..=5) => {
                let (from, sdl) = self.start.ok_or("tab start missing")?;
                let desktop = if n == 2 {
                    let p = shell.desktop_pointer.ok_or("desktop start missing")?;
                    [p[0] + 24., p[1] + 24.]
                } else if let Act::Place { onto, zone, .. } = act {
                    self.point(shell, editor, floats, main, scale, onto, Some(*zone))?
                        .2
                } else {
                    [
                        (main[0] + main[2]) as f32 + 160. + floats.windows.len() as f32 * 440.,
                        main[1] as f32 + main[3] as f32 / 3.,
                    ]
                };
                shell.desktop_pointer = Some(desktop);
                events.push(motion(sdl, from, 1.));
            }
            (Act::TearOff(_) | Act::Place { .. } | Act::Cancel(_), 6) => {
                self.trace.push(json!({"preview":shell.drop_preview.as_ref().map(|p|&p.label),"desktop":shell.desktop_pointer,
                    "dragged":shell.dragged_tab(),"dragging":shell.tab_dragging()}));
                let (_, sdl) = self.start.ok_or("tab start missing")?;
                if matches!(act, Act::Cancel(_)) {
                    events.push((
                        sdl,
                        Event::Key {
                            scancode: 41,
                            pressed: true,
                            repeat: false,
                        },
                    ));
                } else {
                    events.push(button(sdl, false));
                }
            }
            (Act::Cancel(_), 7) => {
                let (_, sdl) = self.start.ok_or("tab start missing")?;
                events.push(button(sdl, false));
                events.push((
                    sdl,
                    Event::Key {
                        scancode: 41,
                        pressed: false,
                        repeat: false,
                    },
                ));
            }
            _ => {}
        }
        Ok(())
    }
    fn across(&self, shell: &Shell, id: &str) -> bool {
        shell
            .shown_node(id)
            .is_some_and(|i| shell.ui.nodes[i].rect.w < shell.ui.nodes[i].rect.h)
    }
    fn extent(&self, shell: &Shell, id: &str) -> ara_ui::Result<f32> {
        let path = id.trim_start_matches("splitter-");
        let i = shell
            .shown_node(&format!("dock-first-{path}"))
            .ok_or("splitter first pane missing")?;
        let r = shell.ui.nodes[i].rect;
        Ok(if self.across(shell, id) { r.w } else { r.h })
    }
    /// Capture before, during and after each gesture from the already-rendered native targets.
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        floats: &windows::Floats,
    ) -> ara_ui::Result<()> {
        if std::mem::take(&mut self.failed) {
            return Ok(());
        }
        let Some(act) = self.acts.get(self.index).cloned() else {
            return Ok(());
        };
        if [0, 5, 6 + SETTLE].contains(&self.age) {
            std::fs::create_dir_all(&self.output)?;
            let stage = match self.age {
                0 => "before",
                5 => "preview",
                _ => "after",
            };
            gpu.select_target(0);
            gpu.pixels()?.save(
                self.output
                    .join(format!("dock-{:02}-{stage}.png", self.index)),
            )?;
            for float in &floats.windows {
                gpu.select_target(float.key);
                gpu.pixels()?.save(
                    self.output
                        .join(format!("dock-{:02}-{stage}-{}.png", self.index, float.id)),
                )?;
            }
            gpu.select_target(0);
        }
        self.age += 1;
        if self.age < 7 + SETTLE {
            return Ok(());
        }
        shell.desktop_pointer = None;
        let floating = |panel: &str| {
            editor
                .state
                .floating
                .iter()
                .find(|f| area(&f.root, panel).is_some())
        };
        let ok = match &act {
            Act::Splitter(id) => (self.extent(shell, id)? - self.before - SPLIT_DRAG).abs() < 8.,
            Act::Scale(value) => (f64::from(windows::ui_scale(editor)) - value).abs() < 1e-3,
            Act::TearOff(panel) => floating(panel).is_some(),
            Act::Cancel(_) => {
                editor.state.dock()?.serialize()? == self.layout_before && !shell.tab_dragging()
            }
            Act::Place { panel, onto, zone } => {
                let destination = floating(onto).map_or(&editor.state.layout, |f| &f.root);
                match zone {
                    Zone::Before | Zone::After | Zone::Centre => area(destination, onto)
                        .is_some_and(|tabs| {
                            let p = tabs.iter().position(|p| p == panel);
                            let o = tabs.iter().position(|p| p == onto);
                            match (p, o, zone) {
                                (Some(p), Some(o), Zone::Before) => p + 1 == o,
                                (Some(p), Some(o), Zone::After) => p == o + 1,
                                (Some(_), Some(_), Zone::Centre) => true,
                                _ => false,
                            }
                        }),
                    _ => split(destination, panel, onto, *zone),
                }
            }
        };
        self.results.push(json!({"drag":format!("{act:?}"),"capturePrefix":format!("dock-{:02}",self.index),"ok":ok,"trace":self.trace,
            "layout":editor.state.layout,"floating":editor.state.floating,"scale":windows::ui_scale(editor)}));
        self.advance();
        Ok(())
    }
    fn advance(&mut self) {
        self.index += 1;
        self.age = 0;
        self.start = None;
        self.trace.clear();
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("drags.json"),
            ara_json::to_canonical_string(&json!({"drags":self.results})),
        )?;
        Ok(())
    }
}
