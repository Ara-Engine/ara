//! Mesh Paint panel (Unreal's vertex paint mode): turns the mode on, picks the tool
//! (brush, erase, fill, flood, pick), the written channels, brush radius, strength
//! and falloff, the paint and erase colours, and which channel the Scene shows.
//! Every control runs a `paint.*` command (menus, palette, shortcuts and agents share
//! them); `editor.paint` sets any of it at once.
use super::*;
use commands::Target;

/// Tools in panel order, with their commands and help.
const TOOLS: [(&str, &str, &str, &str); 5] = [
    (
        "brush",
        "paint.tool.brush",
        "Brush",
        "Blend the paint colour in under the brush.",
    ),
    (
        "erase",
        "paint.tool.erase",
        "Erase",
        "Blend the erase colour in under the brush.",
    ),
    (
        "fill",
        "paint.tool.fill",
        "Fill",
        "Set the whole clicked mesh to the paint colour.",
    ),
    (
        "flood",
        "paint.tool.flood",
        "Flood",
        "Set the clicked connected island to the paint colour.",
    ),
    (
        "pick",
        "paint.tool.pick",
        "Pick",
        "Take the colour under the cursor as the paint colour.",
    ),
];
/// Written channels: command suffix, label and index.
const CHANNELS: [(&str, &str, &str, usize); 4] = [
    ("red", "paint.channel.red", "R", 0),
    ("green", "paint.channel.green", "G", 1),
    ("blue", "paint.channel.blue", "B", 2),
    ("alpha", "paint.channel.alpha", "A", 3),
];

fn hex(c: [f32; 4]) -> String {
    let byte = |v: f32| (v.clamp(0., 1.) * 255. + 0.5) as u8;
    format!(
        "#{:02X}{:02X}{:02X} a{:.2}",
        byte(c[0]),
        byte(c[1]),
        byte(c[2]),
        c[3]
    )
}

impl Shell {
    pub(super) fn mesh_paint_panel(&mut self, editor: &Editor, out: &mut String) {
        out.push_str("<scroll id='mesh-paint' class='lighting-scroll'>");
        let on = editor.state.paint.is_some();
        self.bind_command("mesh-paint-mode", "paint.mode", Target::None);
        out.push_str(&format!(
            "<panel id='mesh-paint-mode-row' class='lighting-card render-debug-row'><toggle id='mesh-paint-mode' checked='{on}' label='Mesh Paint' description='Paint per-instance vertex colours on the selected meshes' action='Dispatch' /><panel id='mesh-paint-mode-text' class='agent-text'><text id='mesh-paint-mode-title' class='lighting-title' text='Mesh Paint' /><text id='mesh-paint-mode-help' class='lighting-help' text='{}' /></panel></panel>",
            escape(if on {
                "Click and drag on a selected mesh to paint; Alt keeps orbiting. Release commits one undoable stroke; Escape cancels it."
            } else {
                "Select meshes, then turn Mesh Paint on. Paint is stored per entity (VertexPaint) and never changes the shared model."
            })
        ));
        let settings = editor.state.paint.unwrap_or_default();
        out.push_str("<panel id='mesh-paint-tools' class='lighting-card'><text id='mesh-paint-tools-title' class='lighting-title' text='Tool' /><panel id='mesh-paint-tool-row' class='paint-row'>");
        for (name, command, title, help) in TOOLS {
            let id = format!("mesh-paint-tool-{name}");
            self.bind_command(&id, command, Target::None);
            let active = settings.tool.name() == name;
            out.push_str(&format!(
                "<button id='{id}' class='lighting-button paint-chip{}' text='{title}' description='{}' action='Dispatch' />",
                if active { " selected" } else { "" },
                escape(help)
            ));
        }
        out.push_str("</panel></panel>");
        out.push_str("<panel id='mesh-paint-channels' class='lighting-card'><text id='mesh-paint-channels-title' class='lighting-title' text='Channels' /><panel id='mesh-paint-channel-row' class='paint-row'>");
        for (name, command, label, index) in CHANNELS {
            let id = format!("mesh-paint-channel-{name}");
            self.bind_command(&id, command, Target::None);
            out.push_str(&format!(
                "<button id='{id}' class='lighting-button paint-chip{}' text='{label}' description='Write the {name} channel' action='Dispatch' />",
                if settings.channels[index] { " selected" } else { "" }
            ));
        }
        out.push_str("</panel></panel>");
        out.push_str("<panel id='mesh-paint-brush' class='lighting-card'><text id='mesh-paint-brush-title' class='lighting-title' text='Brush' />");
        for (name, title, value, unit, commands) in [
            (
                "radius",
                "Radius",
                settings.radius,
                " m",
                ["paint.radius.down", "paint.radius.up"],
            ),
            (
                "strength",
                "Strength",
                settings.strength,
                "",
                ["paint.strength.down", "paint.strength.up"],
            ),
            (
                "falloff",
                "Falloff",
                settings.falloff,
                "",
                ["paint.falloff.down", "paint.falloff.up"],
            ),
        ] {
            let (down, up) = (
                format!("mesh-paint-{name}-down"),
                format!("mesh-paint-{name}-up"),
            );
            self.bind_command(&down, commands[0], Target::None);
            self.bind_command(&up, commands[1], Target::None);
            out.push_str(&format!(
                "<panel id='mesh-paint-{name}-row' class='paint-row'><text id='mesh-paint-{name}-label' class='lighting-help paint-label' text='{title}: {value:.2}{unit}' /><button id='{down}' class='lighting-button paint-step' text='-' description='Decrease {name}' action='Dispatch' /><button id='{up}' class='lighting-button paint-step' text='+' description='Increase {name}' action='Dispatch' /></panel>"
            ));
        }
        out.push_str("</panel>");
        self.bind_command("mesh-paint-swap", "paint.swap", Target::None);
        out.push_str(&format!(
            "<panel id='mesh-paint-colors' class='lighting-card'><text id='mesh-paint-colors-title' class='lighting-title' text='Colours' /><text id='mesh-paint-color' class='lighting-help' text='Paint {}' /><text id='mesh-paint-erase' class='lighting-help' text='Erase {}' /><button id='mesh-paint-swap' class='lighting-button' text='Swap paint and erase' description='Exchange the paint and erase colours' action='Dispatch' />",
            escape(&hex(settings.color)),
            escape(&hex(settings.erase))
        ));
        out.push_str("<panel id='mesh-paint-color-row' class='paint-row'>");
        for (name, command, title) in [
            ("white", "paint.color.white", "White"),
            ("black", "paint.color.black", "Black"),
            ("red", "paint.color.red", "Red"),
            ("green", "paint.color.green", "Green"),
            ("blue", "paint.color.blue", "Blue"),
        ] {
            let id = format!("mesh-paint-color-{name}");
            self.bind_command(&id, command, Target::None);
            out.push_str(&format!(
                "<button id='{id}' class='lighting-button paint-chip' text='{title}' description='Paint {name}' action='Dispatch' />"
            ));
        }
        out.push_str("</panel></panel>");
        out.push_str("<panel id='mesh-paint-view' class='lighting-card'><text id='mesh-paint-view-title' class='lighting-title' text='Show' />");
        let shown = editor.state.view_mode.as_str();
        for (row, buttons) in [
            (
                "main",
                &[
                    ("view.mode.lit", "Lit", "lit"),
                    ("view.mode.vertex_color", "Vertex colour", "vertexColor"),
                ][..],
            ),
            (
                "channels",
                &[
                    ("view.mode.vertex_color_red", "R", "vertexColorRed"),
                    ("view.mode.vertex_color_green", "G", "vertexColorGreen"),
                    ("view.mode.vertex_color_blue", "B", "vertexColorBlue"),
                    ("view.mode.vertex_color_alpha", "A", "vertexColorAlpha"),
                ][..],
            ),
        ] {
            out.push_str(&format!(
                "<panel id='mesh-paint-view-{row}' class='paint-row'>"
            ));
            for &(command, title, mode) in buttons {
                let id = format!("mesh-paint-{}", command.replace('.', "-"));
                self.bind_command(&id, command, Target::None);
                out.push_str(&format!(
                "<button id='{id}' class='lighting-button paint-chip{}' text='{title}' description='Show {mode} in the Scene' action='Dispatch' />",
                if shown == mode { " selected" } else { "" }
            ));
            }
            out.push_str("</panel>");
        }
        out.push_str("</panel></scroll>");
    }
}
