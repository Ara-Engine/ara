//! `--verify-ux` drags, after the captures: every main-window dock splitter, then a
//! tab torn off into a floating window, another tab docked into that window, and both
//! dragged back into the main window (the last one by its single-tab header, which
//! closes the window), at UI scale 1 and again at 1.25. All of it is SDL-level input
//! through the real
//! loop; cross-window drags follow the shell's synthetic desktop pointer
//! (`Shell::desktop_pointer`) instead of the OS one. Writes `drags.json`; a drag that
//! changes nothing fails the route.
use super::*;
use ara_ui::docking::DockNode;

/// One drag and what it must change.
#[derive(Clone, Debug)]
enum Act {
    /// Drag a main-window splitter `SPLIT_DRAG` points along its axis.
    Splitter(String),
    /// Drag a main-window tab out of every window: a new floating window.
    TearOff(&'static str),
    /// Drag a main-window tab into the floating window that holds `into`.
    DockInto {
        panel: &'static str,
        into: &'static str,
    },
    /// Drag a tab from its floating window onto the main-window area holding `onto`.
    Reattach {
        panel: &'static str,
        onto: &'static str,
    },
    /// Reset the layout to the default and set `appearance.uiScale` (floating
    /// windows map drags with it): each pass starts from the same layout.
    Scale(f64),
}

/// How far a splitter drag moves, in logical pixels.
const SPLIT_DRAG: f32 = 48.;
/// Frames after a release before the result is read (rebuilds, new OS windows).
const SETTLE: u32 = 12;

pub(super) struct DragCheck {
    output: PathBuf,
    acts: Vec<Act>,
    index: usize,
    age: u32,
    /// Where the current drag starts (logical, in its window) and its window's SDL id.
    start: Option<([f32; 2], u32)>,
    /// The measured value before the current drag.
    before: f32,
    /// What the shell saw during the current tab drag (for a failed drag's report).
    trace: Vec<Value>,
    /// The current act failed this frame: the next one starts fresh next frame.
    failed: bool,
    results: Vec<Value>,
}

/// Every panel in a dock tree.
fn panels(node: &DockNode) -> Vec<String> {
    panels_of(node)
}

/// The rect of a tab's handle (`tab-*`, or `header-*` for a single-tab area) on
/// the current surface.
fn tab_rect(shell: &Shell, panel: &str) -> Option<ara_ui::paint::Box2> {
    [format!("tab-{panel}"), format!("header-{panel}")]
        .iter()
        .find_map(|id| shell.shown_node(id))
        .map(|i| shell.ui.nodes[i].rect)
}

fn centre(r: ara_ui::paint::Box2) -> [f32; 2] {
    [r.x + r.w / 2., r.y + r.h / 2.]
}

impl DragCheck {
    /// The drags for the current main layout: its splitters, then the tab drags.
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
                Act::TearOff("console"),
                Act::DockInto {
                    panel: "problems",
                    into: "console",
                },
                Act::Reattach {
                    panel: "problems",
                    onto: "hierarchy",
                },
                Act::Reattach {
                    panel: "console",
                    onto: "hierarchy",
                },
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
            trace: Vec::new(),
            failed: false,
            results: Vec::new(),
        }
    }

    pub fn finished(&self) -> bool {
        self.index >= self.acts.len()
    }

    /// Drags that changed nothing.
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|r| r["ok"] != true)
            .map(|r| r["drag"].as_str().unwrap_or("?").to_owned())
            .collect()
    }

    /// Add this frame's synthetic input. `scale` maps main-window points to logical
    /// pixels (`native::points_to_logical`). A drag that cannot run is recorded as
    /// failed and the next one starts.
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
            events.retain(|(_, e)| !matches!(e, Event::MouseButton { .. }));
            shell.desktop_pointer = None;
            self.results
                .push(json!({"drag": format!("{act:?}"), "ok": false, "error": error.to_string()}));
            self.index += 1;
            self.age = 0;
            self.start = None;
            self.trace.clear();
            self.failed = true;
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the loop's window, floats and event batch are borrowed separately"
    )]
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
        let desktop = |local: [f32; 2]| {
            [
                main[0] as f32 + local[0] / scale,
                main[1] as f32 + local[1] / scale,
            ]
        };
        let motion = |sdl: u32, p: [f32; 2], s: f32| {
            (
                sdl,
                Event::MouseMotion {
                    position: [p[0] / s, p[1] / s],
                    delta: [0., 0.],
                },
            )
        };
        let button = |sdl: u32, pressed: bool| (sdl, Event::MouseButton { button: 1, pressed });
        match (act, self.age) {
            (Act::Scale(scale), 0) => {
                editor.action("editor.layout.reset", json!({}))?;
                editor.action(
                    "editor.settings.set",
                    json!({"key": "appearance.uiScale", "value": scale}),
                )?;
            }
            // After a scale change the layout settles on the next input frame: move the
            // pointer once (away from every drag handle) before the next drag measures.
            (Act::Scale(_), 2) => events.push(motion(0, [8., 8.], scale)),
            (Act::Splitter(id), 0) => {
                let Some(i) = shell.shown_node(id) else {
                    return Err(format!("drags: no `{id}` on screen").into());
                };
                let r = shell.ui.nodes[i].rect;
                self.before = self.extent(shell, id)?;
                self.start = Some((centre(r), 0));
                events.push(motion(0, centre(r), scale));
            }
            (Act::Splitter(_), 1) => events.push(button(0, true)),
            (Act::Splitter(id), n @ 2..=5) => {
                let (from, _) = self.start.ok_or("drags: splitter drag lost its start")?;
                let step = SPLIT_DRAG * (n - 1) as f32 / 4.;
                let to = if self.across(shell, id) {
                    [from[0] + step, from[1]]
                } else {
                    [from[0], from[1] + step]
                };
                events.push(motion(0, to, scale));
            }
            (Act::Splitter(_), 6) => events.push(button(0, false)),

            (Act::TearOff(panel) | Act::DockInto { panel, .. }, 0) => {
                let r = tab_rect(shell, panel)
                    .ok_or_else(|| format!("drags: no `{panel}` tab in the main window"))?;
                let [x, y] = centre(r);
                let hit = shell
                    .ui
                    .hit_test(x, y)
                    .map(|i| shell.ui.nodes[i].element.id.clone());
                self.trace.push(json!({"at": "press", "tab": [r.x, r.y, r.w, r.h], "hit": hit, "size": shell.ui.size}));
                self.start = Some((centre(r), 0));
                events.push(motion(0, centre(r), scale));
            }
            (Act::Reattach { panel, .. }, 0) => {
                let float = floats
                    .windows
                    .iter()
                    .find(|f| {
                        editor
                            .state
                            .floating
                            .iter()
                            .any(|s| s.id == f.id && panels(&s.root).iter().any(|p| p == panel))
                    })
                    .ok_or_else(|| format!("drags: `{panel}` is not in a floating window"))?;
                let s = crate::native::points_to_logical(
                    float.window.raw(),
                    float.window.display_scale() * windows::ui_scale(editor),
                );
                let entered = shell.enter(&float.id);
                let rect = tab_rect(shell, panel);
                if entered {
                    shell.leave();
                }
                let r = rect.ok_or_else(|| format!("drags: no `{panel}` tab in its window"))?;
                self.start = Some((centre(r), float.sdl));
                events.push(motion(float.sdl, centre(r), s));
            }
            (Act::TearOff(_) | Act::DockInto { .. } | Act::Reattach { .. }, 1) => {
                let (from, sdl) = self.start.ok_or("drags: tab drag lost its start")?;
                // Hovering a tab can relayout its area (follow-up: right after a scale
                // change): press where the tab is now, as a hand would.
                if let (Act::TearOff(panel) | Act::DockInto { panel, .. }, 0) = (act, sdl)
                    && let Some(r) = tab_rect(shell, panel)
                    && centre(r) != from
                {
                    self.start = Some((centre(r), 0));
                    events.push(motion(0, centre(r), scale));
                }
                self.trace(shell, "pressing", scale);
                events.push(button(sdl, true));
            }
            (Act::TearOff(_) | Act::DockInto { .. } | Act::Reattach { .. }, n @ 2..=5) => {
                let (from, sdl) = self.start.ok_or("drags: tab drag lost its start")?;
                // Leave the tab first (a drag shows its preview past 6 px), then go to
                // the drop point on the desktop.
                let point = if n == 2 {
                    match sdl {
                        0 => desktop([from[0] + 24., from[1] + 24.]),
                        _ => {
                            let f = floats
                                .windows
                                .iter()
                                .find(|f| f.sdl == sdl)
                                .ok_or("drags: the dragged tab's window closed")?;
                            [f.rect[0] as f32 + 40., f.rect[1] as f32 + 40.]
                        }
                    }
                } else {
                    self.drop_point(act, editor, shell, floats, main, scale)?
                };
                shell.desktop_pointer = Some(point);
                events.push(motion(sdl, from, 1.));
                if n == 2 {
                    self.trace(shell, "moved", scale);
                }
            }
            (Act::TearOff(_) | Act::DockInto { .. } | Act::Reattach { .. }, 6) => {
                let (_, sdl) = self.start.ok_or("drags: tab drag lost its start")?;
                self.trace(shell, "release", scale);
                events.push(button(sdl, false));
            }
            _ => {}
        }
        Ok(())
    }

    /// Where a tab drag lets go, on the desktop.
    fn drop_point(
        &self,
        act: &Act,
        editor: &Editor,
        shell: &Shell,
        floats: &windows::Floats,
        main: [i32; 4],
        scale: f32,
    ) -> ara_ui::Result<[f32; 2]> {
        Ok(match act {
            // Right of the main window, outside every window.
            Act::TearOff(_) => [
                (main[0] + main[2]) as f32 + 160.,
                main[1] as f32 + main[3] as f32 / 3.,
            ],
            // The middle of the floating window holding `into`: add as a tab.
            Act::DockInto { into, .. } => {
                let float = floats
                    .windows
                    .iter()
                    .find(|f| {
                        editor
                            .state
                            .floating
                            .iter()
                            .any(|s| s.id == f.id && panels(&s.root).iter().any(|p| p == into))
                    })
                    .ok_or_else(|| format!("drags: `{into}` is not in a floating window"))?;
                [
                    (float.rect[0] + float.rect[2] / 2) as f32,
                    (float.rect[1] + float.rect[3] / 2) as f32,
                ]
            }
            // The middle of the main-window area holding `onto`: add as a tab.
            Act::Reattach { onto, .. } => {
                let area = shell
                    .ui
                    .nodes
                    .iter()
                    .filter(|n| n.element.id.starts_with("area-") && n.visible)
                    .find(|n| {
                        tab_rect(shell, onto)
                            .is_some_and(|t| n.rect.contains(t.x + t.w / 2., t.y + t.h / 2.))
                    })
                    .ok_or_else(|| format!("drags: no main-window area holds `{onto}`"))?;
                let c = centre(area.rect);
                [main[0] as f32 + c[0] / scale, main[1] as f32 + c[1] / scale]
            }
            Act::Splitter(_) | Act::Scale(_) => [0., 0.],
        })
    }

    /// Note what the shell sees of the current tab drag.
    fn trace(&mut self, shell: &Shell, at: &str, scale: f32) {
        let kind = match &shell.drag {
            Some(crate::shell::Drag::Tab { .. }) => "tab",
            Some(crate::shell::Drag::Entity { .. }) => "entity",
            Some(crate::shell::Drag::Asset { .. }) => "asset",
            Some(crate::shell::Drag::Column { .. }) => "column",
            None => "none",
        };
        let [x, y] = shell.pointer;
        let hit = shell
            .ui
            .hit_test(x, y)
            .map(|i| shell.ui.nodes[i].element.id.clone());
        let tab = tab_rect(shell, "console").map(|r| [r.x, r.y, r.w, r.h]);
        self.trace.push(json!({
            "at": at,
            "console": tab,
            "size": shell.ui.size,
            "drag": kind,
            "hit": hit,
            "dragged": shell.dragged_tab(),
            "dragging": shell.tab_dragging(),
            "preview": shell.drop_preview.as_ref().map(|p| p.label.clone()),
            "pointer": shell.pointer,
            "desktop": shell.desktop_pointer,
            "scale": scale,
        }));
    }

    /// True when a splitter divides left from right (it is dragged along x).
    fn across(&self, shell: &Shell, id: &str) -> bool {
        shell
            .shown_node(id)
            .is_some_and(|i| shell.ui.nodes[i].rect.w < shell.ui.nodes[i].rect.h)
    }

    /// The first pane's size along a splitter's axis.
    fn extent(&self, shell: &Shell, id: &str) -> ara_ui::Result<f32> {
        let path = id.trim_start_matches("splitter-");
        let i = shell
            .shown_node(&format!("dock-first-{path}"))
            .ok_or_else(|| format!("drags: no first pane for `{id}`"))?;
        let r = shell.ui.nodes[i].rect;
        Ok(if self.across(shell, id) { r.w } else { r.h })
    }

    /// After the frame: read the result once the drag has settled, then move on.
    pub fn end(&mut self, editor: &Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        if std::mem::take(&mut self.failed) {
            return Ok(());
        }
        let Some(act) = self.acts.get(self.index).cloned() else {
            return Ok(());
        };
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
                .find(|f| panels(&f.root).iter().any(|p| p == panel))
                .map(|f| f.id.clone())
        };
        let result = match &act {
            Act::Splitter(id) => {
                let after = self.extent(shell, id)?;
                json!({"drag": id, "before": self.before, "after": after,
                    "ok": (after - self.before - SPLIT_DRAG).abs() < 8.})
            }
            Act::TearOff(panel) => {
                let window = floating(panel);
                json!({"drag": format!("tear off {panel}"), "window": window, "ok": window.is_some()})
            }
            Act::DockInto { panel, into } => {
                let (a, b) = (floating(panel), floating(into));
                json!({"drag": format!("dock {panel} into {into}'s window"), "window": a,
                    "ok": a.is_some() && a == b})
            }
            Act::Reattach { panel, .. } => {
                let main = panels(&editor.state.layout).iter().any(|p| p == panel);
                json!({"drag": format!("reattach {panel}"), "ok": main && floating(panel).is_none(),
                    "windows": editor.state.floating.len()})
            }
            Act::Scale(scale) => {
                let now = windows::ui_scale(editor);
                json!({"drag": format!("UI scale {scale}"), "ok": (f64::from(now) - scale).abs() < 1e-3})
            }
        };
        let mut result = result;
        if result["ok"] != true && !self.trace.is_empty() {
            result["trace"] = json!(self.trace);
        }
        self.trace.clear();
        self.results.push(result);
        self.index += 1;
        self.age = 0;
        self.start = None;
        Ok(())
    }

    /// Write `drags.json`.
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("drags.json"),
            ara_json::to_canonical_string(&json!({"drags": self.results})),
        )?;
        Ok(())
    }
}
