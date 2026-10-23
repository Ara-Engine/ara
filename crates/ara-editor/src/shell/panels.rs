use super::*;
use ara_ui::docking::{Axis, DockNode};
pub(super) fn escape(s: &str) -> String {
    ara_ui::source::escape(s, b'\'')
}
pub(super) fn button(id: &str, text: &str, class: &str) -> String {
    let (name, only) = match id {
        "tool-Translate" => ("move-3d", true),
        "tool-Rotate" => ("rotate-3d", true),
        "tool-Scale" => ("scaling", true),
        "tool-space" => ("globe", true),
        "tool-pivot" => ("crosshair", true),
        "tool-snap" => ("magnet", true),
        "play" => ("play", true),
        "pause" => ("pause", true),
        "step" => ("step-forward", true),
        "entity-create" => ("plus", true),
        "hierarchy-menu" => ("ellipsis", true),
        "hierarchy-collapse" => ("chevron-down", true),
        "hierarchy-lock" => ("lock", true),
        "hierarchy-hide" => ("eye", true),
        "frame" => ("focus", true),
        "theme" => (if text == "Dark" { "moon" } else { "sun" }, true),
        "palette-open" => ("command", false),
        "add-component" => ("plus", false),
        _ if id.starts_with("remove-") => ("ellipsis", true),
        _ if id.starts_with("collapse-component-") => (
            if text == "Expand" {
                "chevron-right"
            } else {
                "chevron-down"
            },
            true,
        ),
        _ if id.starts_with("pick-") => ("folder", false),
        _ if id.starts_with("accept-") => ("check", false),
        _ if id.starts_with("revert-") => ("undo-2", false),
        _ if id.starts_with("tab-") => (
            match &id[4..] {
                "scene" => "box",
                "game" => "play",
                "hierarchy" => "layers",
                "inspector" => "settings-2",
                "agents" => "bot",
                "console" => "activity",
                _ => "folder",
            },
            false,
        ),
        _ => ("", false),
    };
    if name.is_empty() {
        return format!(
            "<button id='{}' class='{}' text='{}' action='Dispatch' />",
            escape(id),
            class,
            escape(text)
        );
    }
    format!(
        "<button id='{}' class='{} {}' label='{}' description='{}' action='Dispatch'>{}{}</button>",
        escape(id),
        class,
        if only { "icon-button" } else { "" },
        escape(text),
        escape(text),
        super::icons::icon(id, name),
        if only {
            String::new()
        } else {
            format!(
                "<text id='{id}-label' text='{}' style='font-size:12px' />",
                escape(text.trim_start_matches("+ "))
            )
        }
    )
}
impl Shell {
    pub(super) fn dock(
        &mut self,
        node: &DockNode,
        path: &str,
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
                out.push_str(&format!("<panel id='split-{path}' style='flex-grow:1; flex-basis:0px; flex-direction:{}; gap:1px'><panel id='dock-first-{path}' style='flex-grow:{}; flex-basis:0px; min-width:0px; min-height:0px'>",if row{"row"}else{"column"},ratio));
                self.dock(first, &format!("{path}-a"), editor, out)?;
                out.push_str(&format!("</panel><splitter id='splitter-{path}' class='splitter' style='width:{}px; height:{}px' action='ResizeDock' /><panel id='dock-second-{path}' style='flex-grow:{}; flex-basis:0px; min-width:0px; min-height:0px'>",if row{3}else{0},if row{0}else{3},1.-ratio));
                self.dock(second, &format!("{path}-b"), editor, out)?;
                out.push_str("</panel></panel>");
            }
            DockNode::Area { id, tabs, active } => {
                out.push_str(&format!(
                    "<panel id='area-{}' class='dock'><panel id='tabs-{}' class='tabbar'>",
                    escape(id),
                    escape(id)
                ));
                for (index, tab) in tabs.iter().enumerate() {
                    out.push_str(&self.bind_button(
                        &format!("tab-{tab}"),
                        title(tab),
                        Binding::Protocol("editor.layout", json!({"panel":tab})),
                        if index == *active {
                            "tab tab-selected"
                        } else {
                            "tab muted"
                        },
                    ));
                }
                out.push_str("</panel>");
                if let Some(panel) = tabs.get(*active) {
                    match panel.as_str(){"hierarchy"=>self.hierarchy_panel(editor,out)?,"scene"=>self.scene_panel(editor,out),"game"=>out.push_str("<panel id='game-empty' class='body'><text id='game-title' text='Game' /><text id='game-message' class='muted' text='Start a play session to view the game.' /><text id='game-reason' class='muted' text='Play becomes available with the simulation host.' /></panel>"),"inspector"=>self.inspector_panel(editor,out)?,"assets"=>self.assets_panel(editor,out),"console"=>self.console_panel(editor,out),"agents"=>self.agents_panel(editor,out),_=>{}}
                }
                out.push_str("</panel>");
            }
        }
        Ok(())
    }
    pub(super) fn scene_panel(&mut self, editor: &Editor, out: &mut String) {
        out.push_str("<panel id='scene-tools' class='toolbar'><panel id='transform-segment' class='segment'>");
        for (tool, label) in [
            (crate::state::Tool::Translate, "Move  W"),
            (crate::state::Tool::Rotate, "Rotate  E"),
            (crate::state::Tool::Scale, "Scale  R"),
        ] {
            out.push_str(&self.bind_button(
                &format!("tool-{tool:?}"),
                label,
                Binding::Protocol("editor.tool", json!({"tool":tool})),
                if editor.state.tool == tool {
                    "segment-button selected"
                } else {
                    "segment-button quiet"
                },
            ));
        }
        out.push_str("</panel><panel id='space-segment' class='segment'>");
        out.push_str(&self.bind_button(
            "tool-space",
            if editor.state.local { "Local" } else { "World" },
            Binding::Protocol(
                "editor.tool",
                json!({"tool":editor.state.tool,"local":!editor.state.local}),
            ),
            "segment-button quiet",
        ));
        out.push_str(&self.bind_button(
            "tool-pivot",
            if editor.state.centre {
                "Centre"
            } else {
                "Pivot"
            },
            Binding::Protocol(
                "editor.tool",
                json!({"tool":editor.state.tool,"centre":!editor.state.centre}),
            ),
            "segment-button quiet",
        ));
        out.push_str(&self.bind_button(
            "tool-snap",
            if editor.state.snapping {
                "Snap on"
            } else {
                "Snap off"
            },
            Binding::Protocol(
                "editor.tool",
                json!({"tool":editor.state.tool,"snapping":!editor.state.snapping}),
            ),
            "segment-button quiet",
        ));
        out.push_str("</panel><panel id='scene-fill' style='flex-grow:1' />");
        out.push_str(&self.bind_button(
            "frame",
            "Frame  F",
            Binding::Protocol("editor.frame", json!({})),
            "segment-button quiet",
        ));
        out.push_str(&self.bind_button(
            "viewport-quality",
            if editor.state.quality {
                "Fidelity"
            } else {
                "Raster"
            },
            Binding::Protocol(
                "editor.preferences",
                json!({"quality":!editor.state.quality}),
            ),
            "small quiet",
        ));
        out.push_str("</panel><panel id='scene-surface' style='flex-grow:1;flex-basis:0px;min-height:0px'><image id='viewport-image' src='editor-viewport' style='width:100%;height:100%' label='Scene viewport' /><panel id='view-cube' style='position:absolute;right:12px;top:12px;width:84px;height:84px'>");
        for (id, label, yaw, pitch, left, top) in [
            ("view-x", "X", std::f32::consts::FRAC_PI_2, 0., 56, 28),
            ("view-y", "Y", 0., 1.54, 28, 0),
            ("view-z", "Z", 0., 0., 0, 28),
            ("view-iso", "3D", 0.6, 0.4, 28, 28),
        ] {
            self.bindings.insert(ara_ui::source::stable_id(id),Binding::Protocol("editor.camera",json!({"target":editor.state.camera.target.to_array(),"yaw":yaw,"pitch":pitch,"distance":editor.state.camera.distance})));
            if id == "view-iso" {
                out.push_str(&format!("<button id='{id}' class='small quiet view-axis' style='position:absolute;left:{left}px;top:{top}px;width:28px;height:24px' label='Perspective view' description='Return to isometric perspective' action='Dispatch'>{}</button>",icons::icon(id,"box")));
                continue;
            }
            out.push_str(&format!("<button id='{id}' class='small quiet view-axis' style='position:absolute;left:{left}px;top:{top}px;width:28px;height:24px' text='{label}' description='Snap Scene camera to {label} view' action='Dispatch' />"));
        }
        out.push_str("</panel></panel>");
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
fn title(panel: &str) -> &str {
    match panel {
        "scene" => "Scene",
        "game" => "Game",
        "hierarchy" => "Hierarchy",
        "inspector" => "Inspector",
        "assets" => "Assets",
        "console" => "Console / Problems",
        "agents" => "Agents",
        _ => panel,
    }
}
