use super::*;
use ara_ui::docking::{Axis, DockNode};
pub(super) fn escape(s: &str) -> String {
    ara_ui::source::escape(s, b'\'')
}
pub(super) fn button(id: &str, text: &str, class: &str) -> String {
    format!(
        "<button id='{}' class='{}' text='{}' action='Dispatch' />",
        escape(id),
        class,
        escape(text)
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
                out.push_str(&format!("<panel id='split-{path}' style='flex-grow:1; flex-direction:{}; gap:1px'><panel id='dock-first-{path}' style='flex-grow:{}; flex-basis:0px; min-width:0px; min-height:0px'>",if row{"row"}else{"column"},ratio));
                self.dock(first, &format!("{path}-a"), editor, out)?;
                out.push_str(&format!("</panel><splitter id='splitter-{path}' style='width:{}px; height:{}px; background:var(--border)' action='ResizeDock' /><panel id='dock-second-{path}' style='flex-grow:{}; flex-basis:0px; min-width:0px; min-height:0px'>",if row{3}else{0},if row{0}else{3},1.-ratio));
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
        out.push_str("<panel id='scene-tools' class='toolbar'>");
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
                    "small selected"
                } else {
                    "small quiet"
                },
            ));
        }
        out.push_str(&self.bind_button(
            "tool-space",
            if editor.state.local { "Local" } else { "World" },
            Binding::Protocol(
                "editor.tool",
                json!({"tool":editor.state.tool,"local":!editor.state.local}),
            ),
            "small quiet",
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
            "small quiet",
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
            "small quiet",
        ));
        out.push_str("<panel id='scene-fill' style='flex-grow:1' />");
        out.push_str(&self.bind_button(
            "frame",
            "Frame  F",
            Binding::Protocol("editor.frame", json!({})),
            "small quiet",
        ));
        out.push_str("</panel><image id='viewport-image' src='editor-viewport' style='flex-grow:1; min-width:0px; min-height:0px' label='Scene viewport' />");
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
