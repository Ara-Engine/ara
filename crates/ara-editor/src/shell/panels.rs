//! Dock areas and the shared panel template: a 28 px header (one panel) or tab
//! strip (several), optional toolbar, body and footer, with a ⋯ menu built from
//! the registry and a real empty state when there is nothing to show.
use super::commands::{Target, menus::Context};
use super::*;
use ara_ui::docking::{Axis, DockNode};
pub(super) fn escape(s: &str) -> String {
    ara_ui::source::escape(s, b'\'')
}
/// A text button; ids with a known icon render icon-only or icon + label.
pub(super) fn button(id: &str, text: &str, class: &str) -> String {
    let (name, only) = match id {
        "entity-create" => ("plus", true),
        "theme" => (if text == "Dark" { "moon" } else { "sun" }, true),
        _ if id.starts_with("pick-") => ("folder", false),
        _ if id.starts_with("accept-") => ("check", false),
        _ if id.starts_with("revert-") => ("undo-2", false),
        _ => ("", false),
    };
    if name.is_empty() {
        return format!(
            "<button id='{}' class='{}' label='{}' action='Dispatch'><text id='{}-label' text='{}' /></button>",
            escape(id),
            class,
            escape(text),
            escape(id),
            escape(text)
        );
    }
    format!(
        "<button id='{}' class='{} {}' label='{}' description='{}' action='Dispatch'>{}{}</button>",
        escape(id),
        class,
        if only { "icon-button ghost" } else { "" },
        escape(text),
        escape(text),
        super::icons::icon(id, name),
        if only {
            String::new()
        } else {
            format!(
                "<text id='{id}-label' text='{}' />",
                escape(text.trim_start_matches("+ "))
            )
        }
    )
}
/// Human panel title.
pub(super) fn title(panel: &str) -> &str {
    match panel {
        "material" => "Material",
        "vfx" => "VFX",
        "animation" => "Animation",
        "profiler" => "Profiler",
        "scene" => "Scene",
        "game" => "Game",
        "hierarchy" => "Hierarchy",
        "inspector" => "Inspector",
        "assets" => "Assets",
        "console" => "Console",
        "problems" => "Problems",
        "agents" => "Agents",
        "settings" => "Settings",
        "history" => "History",
        _ => panel,
    }
}
/// Panels whose body stays mounted, hidden, while another tab of its area is active:
/// list and text panels, so switching back keeps their rows, scroll and shaped text.
/// Scene, Game and the preview, canvas and timeline panels hold GPU surfaces or live
/// geometry and render only while active.
pub(super) fn keep_alive(panel: &str) -> bool {
    matches!(
        panel,
        "hierarchy"
            | "inspector"
            | "assets"
            | "console"
            | "problems"
            | "agents"
            | "settings"
            | "history"
    )
}
/// One icon per concept (Appendix B).
pub(super) fn panel_icon(panel: &str) -> &'static str {
    match panel {
        "scene" => "clapperboard",
        "game" => "gamepad-2",
        "material" => "contrast",
        "vfx" => "sparkles",
        "animation" => "person-standing",
        "profiler" => "gauge",
        "hierarchy" => "list-tree",
        "inspector" => "sliders-horizontal",
        "assets" => "folder",
        "console" => "terminal",
        "problems" => "triangle-alert",
        "agents" => "bot",
        "settings" => "settings-2",
        "history" => "clock",
        _ => "square",
    }
}
impl Shell {
    /// Markup for a dock tree. `edge` is true while the subtree touches the main
    /// window's bottom edge: its areas can collapse to their tab strip (a chevron in
    /// their header), like the default layout's bottom bar.
    pub(super) fn dock(
        &mut self,
        node: &DockNode,
        path: &str,
        edge: bool,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        match node {
            DockNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let row = *axis == Axis::Horizontal;
                let folded = |n: &DockNode| matches!(n, DockNode::Area { id, .. } if editor.state.collapsed_areas.contains(id));
                // A collapsed area is just as tall as its tab strip; its sibling takes
                // the rest, and there is nothing to resize between them.
                let (fold_first, fold_second) = (folded(first), folded(second));
                let pane = |folded: bool, other: bool, grow: f32| {
                    if folded {
                        "flex-grow:0; flex-shrink:0; flex-basis:auto; min-width:0px".to_owned()
                    } else {
                        let grow = if other { 1. } else { grow };
                        format!("flex-grow:{grow}; flex-basis:0px; min-width:0px; min-height:0px")
                    }
                };
                out.push_str(&format!("<panel id='split-{path}' style='flex-grow:1; flex-basis:0px; min-width:0px; min-height:0px; flex-direction:{}; gap:0px'><panel id='dock-first-{path}' style='{}'>",if row{"row"}else{"column"},pane(fold_first, fold_second, *ratio)));
                self.dock(first, &format!("{path}-a"), edge && row, editor, out)?;
                if fold_first || fold_second {
                    out.push_str(&format!(
                        "</panel><panel id='gap-{path}' style='{}:4px; flex-shrink:0' />",
                        if row { "width" } else { "height" }
                    ));
                } else {
                    // Only the main-axis size is set: the cross axis stretches, so the
                    // splitter spans its split (an explicit 0 there left nothing to grab).
                    out.push_str(&format!("</panel><splitter id='splitter-{path}' class='splitter' style='{}:4px; flex-shrink:0' action='ResizeDock' />",if row{"width"}else{"height"}));
                }
                out.push_str(&format!(
                    "<panel id='dock-second-{path}' style='{}'>",
                    pane(fold_second, fold_first, 1. - ratio)
                ));
                self.dock(second, &format!("{path}-b"), edge, editor, out)?;
                out.push_str("</panel></panel>");
            }
            DockNode::Area { id, tabs, active } => {
                // The area's header and each mounted tab body are parts
                // (`Ui::reconcile_parts`): an unchanged part keeps its retained subtree,
                // virtual rows included. Inactive `keep_alive` tabs stay mounted, hidden,
                // so switching to one is a visibility change; other panels render only
                // while active.
                let area = escape(id);
                let collapsed = editor.state.collapsed_areas.contains(id);
                let mut head = String::new();
                let head_id =
                    self.area_head(&area, tabs, *active, edge.then_some(collapsed), &mut head);
                out.push_str(&format!(
                    "<panel id='area-{area}' class='dock'><panel id='{head_id}' />"
                ));
                self.parts.push((head_id, head));
                if collapsed {
                    // Just the tab strip: a tab expands the area again.
                    out.push_str("</panel>");
                    return Ok(());
                }
                match tabs.get(*active) {
                    None => self.empty(
                        out,
                        &format!("area-empty-{area}"),
                        "layout-grid",
                        "Empty area",
                        "Drag a tab here, or open a panel from the Window menu.",
                        &[],
                    ),
                    Some(current) => {
                        for tab in tabs {
                            let host = format!("body-{tab}");
                            let body = if tab == current {
                                let mut body = format!("<panel id='{host}' class='tab-body'>");
                                self.panel_body(tab, editor, &mut body)?;
                                body.push_str("</panel>");
                                body
                            } else if let Some(body) = self.bodies.remove(tab) {
                                self.hidden_bodies.push(host.clone());
                                body
                            } else {
                                continue;
                            };
                            out.push_str(&format!("<panel id='{host}' />"));
                            self.parts.push((host, body));
                        }
                    }
                }
                out.push_str("</panel>");
            }
        }
        Ok(())
    }
    /// One dock area's header part: its tab bar (or a lone panel's title), a
    /// collapse/expand chevron when `fold` is `Some(collapsed)` (bottom-edge areas),
    /// and the active panel's options button. Returns the part's host ID.
    fn area_head(
        &mut self,
        area: &str,
        tabs: &[String],
        active: usize,
        fold: Option<bool>,
        out: &mut String,
    ) -> String {
        let current = tabs.get(active).cloned();
        let host = if tabs.len() <= 1 {
            let panel = current.clone().unwrap_or_default();
            // `header-{panel}` names a panel (drag, tab menu): an empty area's header
            // has IDs of its own.
            let (host, title_id, fill_id) = match &current {
                Some(_) => (
                    format!("header-{panel}"),
                    format!("header-title-{panel}"),
                    format!("header-fill-{panel}"),
                ),
                None => (
                    format!("empty-header-{area}"),
                    format!("empty-header-title-{area}"),
                    format!("empty-header-fill-{area}"),
                ),
            };
            out.push_str(&format!("<panel id='{host}' class='panel-header' cursor='grab'>{}<text id='{title_id}' class='panel-title' text='{}' /><panel id='{fill_id}' class='fill' />",icons::icon(&host,panel_icon(&panel)),escape(title(&panel))));
            host
        } else {
            out.push_str(&format!("<panel id='tabs-{area}' class='tabbar'>"));
            // Narrow areas show inactive tabs as icons; the tooltip names them.
            let width = self
                .ui
                .find(&format!("area-{area}"))
                .map(|i| self.ui.nodes[i].rect.w)
                .unwrap_or(f32::MAX);
            let compact = width < tabs.len() as f32 * 96. + 40.;
            for (index, tab) in tabs.iter().enumerate() {
                let tab_id = format!("tab-{tab}");
                self.bind_command(&tab_id, "window.show_panel", Target::Panel(tab.clone()));
                let active = index == active;
                // Every tab keeps its label (hidden on a narrow area's inactive tabs),
                // so a tab switch only changes classes and reconciles in place.
                let label = format!(
                    "<text id='{tab_id}-label'{} text='{}' />",
                    if active || !compact {
                        ""
                    } else {
                        " class='tab-label-hidden'"
                    },
                    escape(title(tab))
                );
                out.push_str(&format!("<button id='{tab_id}' cursor='grab' class='tab{}' label='{}' description='{}: drag to move, right-click for options' action='Dispatch'>{}{label}</button>",if active{" tab-selected"}else{""},escape(title(tab)),escape(title(tab)),if active{icons::icon_in(&tab_id,panel_icon(tab),"fg")}else{icons::icon(&tab_id,panel_icon(tab))}));
            }
            out.push_str(&format!("<panel id='tabs-fill-{area}' class='fill' />"));
            format!("tabs-{area}")
        };
        if let Some(collapsed) = fold {
            let id = format!("fold-{area}");
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::Protocol(
                    "editor.layout.collapse",
                    json!({"area": area, "collapsed": !collapsed}),
                ),
            );
            let (icon, label) = if collapsed {
                ("chevron-up", "Expand")
            } else {
                ("chevron-down", "Collapse")
            };
            out.push_str(&format!("<button id='{id}' class='ghost icon-button' label='{label}' description='{label} this area' action='Dispatch'>{}</button>",icons::icon(&id,icon)));
        }
        if let Some(panel) = &current {
            // One options button per area (its binding names the panel), so switching
            // tabs keeps the header's shape.
            let menu = format!("panel-menu-{area}");
            self.bindings.insert(
                ara_ui::source::stable_id(&menu),
                Binding::ContextMenu(Context::Tab, Target::Panel(panel.clone())),
            );
            out.push_str(&format!("<button id='{menu}' class='ghost icon-button' label='{} options' description='Move, float, maximize or close' action='Dispatch'>{}</button>",escape(title(panel)),icons::icon(&menu,"ellipsis")));
        }
        out.push_str("</panel>");
        host
    }
    fn panel_body(
        &mut self,
        panel: &str,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        self.with_panel_boundary(panel, editor, out, |shell, editor, out| {
            shell.panel_body_inner(panel, editor, out)
        });
        Ok(())
    }
    fn panel_body_inner(
        &mut self,
        panel: &str,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        match panel {
            "hierarchy" => self.hierarchy_panel(editor, out)?,
            "scene" => self.scene_panel(editor, out),
            "game" => self.game_panel(editor, out),
            "material" => self.material_panel(editor, out)?,
            "vfx" => self.vfx_panel(editor, out)?,
            "profiler" => self.profiler_panel(editor, out)?,
            "animation" => self.animation_panel(editor, out)?,
            "inspector" => self.inspector_panel(editor, out)?,
            "assets" => {
                let editor: &Editor = editor;
                let key = self.assets_key(editor);
                self.memoized("assets", key, out, |shell, out| {
                    shell.assets_panel(editor, out);
                });
            }
            "console" => self.console_panel(editor, out),
            "problems" => self.problems_panel(editor, out),
            "agents" => self.agents_panel(editor, out),
            "settings" => self.settings_panel(editor, out),
            "history" => self.history_panel(editor, out),
            other => self.empty(
                out,
                &format!("unknown-{other}"),
                "square",
                "Unknown panel",
                "This layout names a panel this editor does not have.",
                &[("unknown-reset", "Reset layout", "layout.reset")],
            ),
        }
        Ok(())
    }
    /// A real empty state: icon, one line, a hint and up to three command buttons.
    pub(super) fn empty(
        &mut self,
        out: &mut String,
        id: &str,
        icon: &str,
        headline: &str,
        hint: &str,
        actions: &[(&str, &str, &'static str)],
    ) {
        out.push_str(&format!("<panel id='{id}' class='empty'>{}<text id='{id}-title' class='empty-title' text='{}' /><text id='{id}-hint' class='empty-hint' text='{}' />",icons::icon_in(id,icon,"icon-lg"),escape(headline),escape(hint)));
        if !actions.is_empty() {
            out.push_str(&format!("<panel id='{id}-actions' class='empty-actions'>"));
            for (n, (button, label, command)) in actions.iter().enumerate() {
                self.bind_command(button, command, Target::None);
                let keys = self
                    .keymap
                    .chords(command)
                    .first()
                    .map(|c| format!(" · {}", c.text()))
                    .unwrap_or_default();
                out.push_str(&format!("<button id='{button}' class='{}' label='{}' description='{}{}' action='Dispatch'><text id='{button}-label' text='{}' /></button>",if n==0{"primary"}else{""},escape(label),escape(label),escape(&keys),escape(label)));
            }
            out.push_str("</panel>");
        }
        out.push_str("</panel>");
    }
    /// An icon-only toolbar button bound to a registry command, with a tooltip.
    pub(super) fn tool_icon(
        &mut self,
        out: &mut String,
        id: &str,
        command: &'static str,
        icon: &str,
        editor: &Editor,
        on: bool,
    ) {
        self.bind_command(id, command, Target::None);
        let spec = commands::get(command);
        let keys = self
            .keymap
            .chords(command)
            .first()
            .map(|c| format!(" · {}", c.text()))
            .unwrap_or_default();
        let title = spec
            .map(|s| s.title(&self.ctx(editor, &Target::None)))
            .unwrap_or_default();
        out.push_str(&format!("<button id='{id}' class='{} icon-button' label='{}' description='{}{}' action='Dispatch'>{}</button>",if on{"selected"}else{"ghost"},escape(&title),escape(&title),escape(&keys),if on{icons::icon_in(id,icon,"on-selection")}else{icons::icon(id,icon)}));
    }
    /// A labelled toolbar button (icon + text) bound to a registry command.
    pub(super) fn tool_text(
        &mut self,
        out: &mut String,
        id: &str,
        command: &'static str,
        icon: &str,
        text: &str,
        class: &str,
    ) {
        self.bind_command(id, command, Target::None);
        let summary = commands::get(command).map(|c| c.summary).unwrap_or("");
        out.push_str(&format!("<button id='{id}' class='{class}' label='{}' description='{}' action='Dispatch'>{}<text id='{id}-label' text='{}' /></button>",escape(text),escape(summary),icons::icon(id,icon),escape(text)));
    }
    /// A dropdown button that opens a registry-built menu. An empty `text` makes an
    /// icon-only button labelled by its description.
    pub(super) fn tool_menu(
        &mut self,
        out: &mut String,
        id: &str,
        context: Context,
        icon: &str,
        text: &str,
        description: &str,
    ) {
        self.bindings.insert(
            ara_ui::source::stable_id(id),
            Binding::ContextMenu(context, Target::None),
        );
        let words = if text.is_empty() {
            String::new()
        } else {
            format!("<text id='{id}-label' text='{}' />", escape(text))
        };
        out.push_str(&format!("<button id='{id}' class='ghost' style='padding-left:6px;padding-right:4px' label='{}' description='{}' action='Dispatch'>{}{words}{}</button>",escape(if text.is_empty(){description}else{text}),escape(description),icons::icon(id,icon),icons::icon_in(&format!("{id}-more"),"chevron-down","icon-sm")));
    }
}
pub(super) fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(n) => {
            let value = n.as_f64().unwrap_or(0.);
            if value.fract().abs() < 0.00001 {
                format!("{value:.0}")
            } else {
                format!("{value:.3}").trim_end_matches('0').to_owned()
            }
        }
        _ => v.to_string(),
    }
}
