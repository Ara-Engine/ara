//! Compact controls over the Scene image; every action uses the command registry.
use super::commands::{Target, menus::Context};
use super::*;

impl Shell {
    pub(super) fn scene_panel(&mut self, editor: &Editor, out: &mut String) {
        out.push_str("<panel id='scene-surface' class='scene-surface'><image id='viewport-image' src='editor-viewport' style='width:100%;height:100%' label='Scene viewport' />");
        if !editor.state.hide_overlays {
            let width = self
                .ui
                .find("scene-surface")
                .map(|i| self.ui.nodes[i].rect.w)
                .unwrap_or(self.ui.size[0] * 0.55);
            let compact = width < 780.;
            let narrow = width < 1000.;
            out.push_str("<panel id='scene-tools' class='viewport-tools'><panel id='scene-edit-tools' class='viewport-cluster'>");
            let tool = editor.state.tool;
            let gizmos = !editor.state.hide_gizmos;
            self.bind_command("tool-select", "view.tool.select", Target::None);
            let select_keys = self
                .keymap
                .chords("view.tool.select")
                .first()
                .map(|chord| chord.text())
                .unwrap_or_default();
            out.push_str(&format!("<button id='tool-select' class='viewport-button{}' label='Select' description='Select · {select_keys}' action='Dispatch'>{}</button><panel id='tool-select-separator' class='viewport-divider' /><panel id='transform-segment' class='viewport-segment' name='mode'>",if gizmos {""}else{" viewport-button-on"},icons::icon_in("tool-select","mouse-pointer","viewport-icon")));
            for (id, command, icon, label, on) in [
                (
                    "tool-Translate",
                    "view.tool.move",
                    "move-3d",
                    "Move",
                    gizmos && tool == crate::state::Tool::Translate,
                ),
                (
                    "tool-Rotate",
                    "view.tool.rotate",
                    "rotate-3d",
                    "Rotate",
                    gizmos && tool == crate::state::Tool::Rotate,
                ),
                (
                    "tool-Scale",
                    "view.tool.scale",
                    "scaling",
                    "Scale",
                    gizmos && tool == crate::state::Tool::Scale,
                ),
                (
                    "tool-Universal",
                    "view.tool.universal",
                    "transform",
                    "Universal",
                    gizmos && tool == crate::state::Tool::Universal,
                ),
            ] {
                self.bind_command(id, command, Target::None);
                let keys = self
                    .keymap
                    .chords(command)
                    .first()
                    .map(|c| c.text())
                    .unwrap_or_default();
                out.push_str(&format!("<button id='{id}' class='viewport-button{}' label='{label}' description='{label} · {keys}' action='Dispatch'>{}</button>",if on{" viewport-button-on"}else{""},icons::icon_in(id,icon,"viewport-icon")));
            }
            out.push_str("</panel>");
            // Space and pivot: the icon is the state; the tooltip says what a click does.
            let local = editor.state.local;
            let centre = editor.state.centre;
            for (id, command, icon, state, other) in [
                (
                    "tool-space",
                    "view.space",
                    if local { "box" } else { "globe" },
                    if local { "Local axes" } else { "World axes" },
                    if local { "world axes" } else { "local axes" },
                ),
                (
                    "tool-pivot",
                    "view.pivot",
                    if centre { "circle-dot" } else { "crosshair" },
                    if centre {
                        "Around the selection centre"
                    } else {
                        "Around each pivot"
                    },
                    if centre {
                        "each pivot"
                    } else {
                        "the selection centre"
                    },
                ),
            ] {
                self.bind_command(id, command, Target::None);
                out.push_str(&format!("<button id='{id}' class='viewport-dropdown' label='{state}' description='{state} · click for {other}' action='Dispatch'>{}<text id='{id}-label' text='{}' /></button>",icons::icon_in(id,icon,"viewport-icon"),if id == "tool-space" { if local {"Local"} else {"World"} } else if centre {"Center"} else {"Pivot"}));
            }
            out.push_str("<panel id='snap-sep' class='viewport-divider' />");
            // Snapping: one magnet, then the three increments as value menus (muted off).
            let snap = editor.state.snapping;
            self.bind_command("snap-toggle", "snap.toggle", Target::None);
            out.push_str(&format!("<button id='snap-toggle' class='viewport-button{}' label='Snapping' description='Snapping is {} · click to turn it {}' action='Dispatch'>{}</button>",if snap{" viewport-button-on"}else{""},if snap{"on"}else{"off"},if snap{"off"}else{"on"},icons::icon_in("snap-toggle","magnet","viewport-icon")));
            if compact {
                self.viewport_menu(
                    out,
                    "snap-options",
                    Context::Snapping,
                    "sliders-horizontal",
                    "",
                    "Move, rotate and scale snap increments",
                );
            } else {
                for (id, command, icon, what, value) in [
                    (
                        "snap-move",
                        "snap.grid",
                        "grid-2x2",
                        "Move increment",
                        format!("{} m", editor.state.move_snap),
                    ),
                    (
                        "snap-rotate",
                        "snap.rotation",
                        "rotate-3d",
                        "Rotate increment",
                        format!("{}°", editor.state.rotate_snap),
                    ),
                    (
                        "snap-scale",
                        "snap.scale",
                        "scaling",
                        "Scale increment",
                        format!("{}", editor.state.scale_snap),
                    ),
                ] {
                    self.bind_command(id, command, Target::None);
                    let tone = if snap { "fg" } else { "muted" };
                    out.push_str(&format!("<button id='{id}' class='viewport-dropdown' label='{what}' description='{what}: {} · click to choose' action='Dispatch'>{}<text id='{id}-text' class='{tone}' text='{}' />{}</button>",escape(&value),icons::icon_in(id,icon,tone),escape(&value),icons::icon_in(&format!("{id}-more"),"chevron-down","icon-xs")));
                }
            }
            out.push_str("</panel><panel id='scene-fill' class='fill' /><panel id='scene-view-tools' class='viewport-cluster'>");
            self.viewport_menu(
                out,
                "view-camera",
                Context::Camera,
                "video",
                if narrow {
                    ""
                } else if editor.state.camera.projection
                    == crate::state::CameraProjection::Orthographic
                {
                    "Orthographic"
                } else {
                    "Perspective"
                },
                "Camera views, projection, lens, bookmarks and speed",
            );
            let orthographic =
                editor.state.camera.projection == crate::state::CameraProjection::Orthographic;
            self.bind_command("view-projection", "view.camera.perspective", Target::None);
            out.push_str(&format!("<button id='view-projection' class='viewport-button{}' label='Toggle orthographic projection' description='{} projection; click to switch · Num5' action='Dispatch'>{}</button>",if orthographic {" viewport-button-on"} else {""},if orthographic {"Orthographic"} else {"Perspective"},icons::icon_in("view-projection","box","viewport-icon")));
            self.bind_command(
                "view-bookmark-save",
                "view.camera.bookmark.save",
                Target::None,
            );
            self.bind_command("view-bookmarks", "view.camera.bookmarks", Target::None);
            for (id, icon, label) in [
                ("view-bookmark-save", "save", "Save camera bookmark"),
                ("view-bookmarks", "camera", "Recall camera bookmark"),
            ] {
                out.push_str(&format!("<button id='{id}' class='viewport-button' label='{label}' description='{label}' action='Dispatch'>{}</button>",icons::icon_in(id,icon,"viewport-icon")));
            }
            self.bindings
                .insert(ara_ui::source::stable_id("view-fov"), Binding::CameraFov);
            if orthographic {
                out.push_str(&format!("<field id='view-fov' class='number viewport-fov' min='0.01' max='1000000' step='0.1' text='{}' label='Orthographic height' description='Visible vertical span in metres; drag the field grip to zoom' action='Dispatch' />",editor.state.camera.orthographic_height));
            } else {
                out.push_str(&format!("<field id='view-fov' class='number viewport-fov' min='10' max='120' step='1' text='{}' label='Field of view' description='Vertical field of view in degrees (10–120)' action='Dispatch' />",editor.state.camera.fov_y_degrees));
            }
            self.viewport_menu(
                out,
                "view-speed",
                Context::CameraSpeed,
                "gauge",
                &format!("{}", editor.state.number("viewport.cameraSpeed", 4.)),
                "Camera fly speed (metres per second)",
            );
            let state = &editor.state;
            let mode = match state.view_mode.as_str() {
                "baseColor" => "Unlit".into(),
                "normals" => "Normals".into(),
                "depth" => "Depth".into(),
                "roughnessMetal" => "Rough / metal".into(),
                "entityIds" => "Entity IDs".into(),
                "shadowCascades" => "Cascades".into(),
                "wireframe" => "Wireframe".into(),
                "litWireframe" => "Lit wire".into(),
                // Lit shows the preset the view renders with (`Lit · Auto` until detected).
                _ if narrow => "Lit".into(),
                _ => format!(
                    "Lit · {}",
                    state
                        .resolve_quality(state.render_choice())
                        .map_or("Auto", crate::state::preset_title)
                ),
            };
            self.viewport_menu(
                out,
                "view-mode",
                Context::ViewModes,
                "sun-medium",
                &mode,
                "View mode and rendering quality",
            );
            self.viewport_menu(
                out,
                "view-show",
                Context::Show,
                "eye",
                "",
                "Show: grid, outlines, gizmos and overlays",
            );
            out.push_str("</panel></panel>");
        }
        if !editor.state.hide_overlays && !editor.state.hide_orientation && !editor.state.game_view
        {
            self.axis_markup(editor, out);
        }
        self.notices_markup(editor, out);
        self.loading_markup(editor, out);
        if !editor.state.hide_overlays
            && !editor.state.game_view
            && let Some(target) = self.pilot_target()
        {
            out.push_str(&format!("<panel id='scene-pilot' style='position:absolute;left:12px;top:46px;max-width:92%;padding:6px;gap:4px;background:var(--popover);border-radius:4px'><text id='scene-pilot-target' class='caption' style='white-space:normal' text='Piloting {}' /><button id='scene-unpilot' class='viewport-button' label='Stop piloting' text='Stop piloting' description='Keep the last completed Camera move and return to Scene navigation' action='Dispatch' /></panel>",escape(target)));
            self.bind_command("scene-unpilot", "view.camera.pilot", Target::None);
        }
        self.viewport_diagnostics.stats_markup(&editor.state, out);
        out.push_str("<panel id='gizmo-readout' class='readout' style='left:0px;top:0px'><text id='gizmo-readout-text' class='mono' text='' /></panel></panel>");
    }

    /// Registry-built dropdown; the common menu host supplies anchored popovers.
    fn viewport_menu(
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
        out.push_str(&format!("<button id='{id}' class='viewport-dropdown' label='{}' description='{}' action='Dispatch'>{}{words}{}</button>",escape(description),escape(description),icons::icon_in(id,icon,"viewport-icon"),icons::icon_in(&format!("{id}-more"),"chevron-down","icon-xs")));
    }
}
