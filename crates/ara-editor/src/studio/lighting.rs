//! Lighting: everything that lights the scene in one list (lights, sky and environment,
//! height fog, post-process volumes with their live weight at the Scene camera, reflection
//! probes), as Unreal's Lighting views gather them. A row selects its entity (the Inspector
//! edits it); Add creates a volume, fog or probe at the camera's pivot; Presets append
//! override groups to the selected volumes. The rows and commands are the legacy panel's
//! (`shell::lighting::{model, authoring}`), so both windows read and write the same way.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{Align, Axis, El, Justify, Side, col, el, icon, menu, row, text};
use stillkit::icons::Icon;

use super::Studio;
use crate::shell::lighting::authoring::{self, Create, Preset};

/// What Add offers, in order.
const ADD: [(Create, &str, Icon); 6] = [
    (Create::Global, "Global post-process volume", icons::GLOBE),
    (Create::Box, "Local post-process box", icons::BOX),
    (Create::Sphere, "Local post-process sphere", icons::CIRCLE),
    (Create::HeightFog, "Height fog", icons::CLOUD),
    (Create::FogVolume, "Local fog volume", icons::CLOUD),
    (Create::Probe, "Reflection probe", icons::APERTURE),
];

/// The panel.
pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let query = s.local.lighting_query.to_lowercase();
    let rows = crate::shell::lighting::model::rows(&s.resolved, &s.editor, query.trim());
    let volumes = !s.editor.state.selection.is_empty()
        && s.editor.state.selection.iter().all(|id| {
            s.resolved
                .get(id)
                .is_some_and(|e| e.record.contains_key("PostProcessVolume"))
        });
    let search = field(
        l,
        Size::Sm,
        "lighting/search",
        Some(icons::SEARCH),
        "Filter lights, fog, volumes…",
        &s.local.lighting_query,
        |f| f.on_input(|s: &mut Studio, t| s.local.lighting_query = t.to_owned()),
    )
    .grow(1.0);
    let mut add = button(l, Tone::Ghost, Size::Sm, Some(icons::PLUS), "Add")
        .key("lighting/add")
        .on_press(|s: &mut Studio| toggle(s, "add"));
    if s.local.lighting_menu == Some("add") {
        let items = ADD
            .iter()
            .map(|(kind, title, glyph)| {
                let kind = *kind;
                menu_item(
                    l,
                    &format!("lighting/add/{title}"),
                    Some(*glyph),
                    title,
                    None,
                    false,
                )
                .on_press(move |s: &mut Studio| create(s, kind))
            })
            .collect();
        add = add.overlay(
            menu(menu_panel(l, items))
                .side(Side::Bottom)
                .align(Align::End)
                .on_dismiss(|s: &mut Studio| s.local.lighting_menu = None),
        );
    }
    let mut presets = enabled(
        button(
            l,
            Tone::Ghost,
            Size::Sm,
            Some(icons::WAND_SPARKLES),
            "Presets",
        )
        .key("lighting/presets")
        .tooltip(if volumes {
            "Append override groups to the selected volumes"
        } else {
            "Select post-process volumes to apply a preset"
        })
        .on_press(|s: &mut Studio| toggle(s, "presets")),
        volumes,
    );
    if s.local.lighting_menu == Some("presets") {
        let items = [
            (Preset::ManualExposure, "Manual exposure (EV100 7)"),
            (
                Preset::Interior,
                "Interior look: exposure, white balance, bloom",
            ),
        ]
        .into_iter()
        .map(|(preset, title)| {
            menu_item(
                l,
                &format!("lighting/preset/{title}"),
                None,
                title,
                None,
                false,
            )
            .on_press(move |s: &mut Studio| apply_preset(s, preset))
        })
        .collect();
        presets = presets.overlay(
            menu(menu_panel(l, items))
                .side(Side::Bottom)
                .align(Align::End)
                .on_dismiss(|s: &mut Studio| s.local.lighting_menu = None),
        );
    }
    let toolbar = row()
        .h(super::style::TOOLBAR + 6.0)
        .pad_x(6.0)
        .gap(4.0)
        .align(Align::Center)
        .child(search)
        .child(add)
        .child(presets);
    let body = if rows.is_empty() {
        col().grow(1.0).justify(Justify::Center).child(empty(
            l,
            icons::SUN,
            if query.trim().is_empty() {
                "Nothing lights this scene yet"
            } else {
                "Nothing matches"
            },
            "Add a light from Create, or a volume, fog or probe from Add.",
            None,
        ))
    } else {
        let mut list = col()
            .key("lighting/list")
            .h(0.0)
            .grow(1.0)
            .pad_x(4.0)
            .gap(1.0)
            .scroll(Axis::Vertical);
        for r in &rows {
            let on = s.editor.state.selection.contains(&r.id);
            let id = r.id.clone();
            let hover = c.hover_surface.0;
            list = list.child(
                super::style::chosen(l, row(), on)
                    .key(format!("lighting/row/{}", r.id))
                    .h(38.0)
                    .pad_x(8.0)
                    .gap(10.0)
                    .align(Align::Center)
                    .radius(super::style::RADIUS)
                    .label(r.name.clone())
                    .description(r.description.clone())
                    .selected(on)
                    .hovered(move |e| if on { e } else { e.fill(hover) })
                    .on_press(move |s: &mut Studio| s.select(vec![id.clone()], false))
                    .child(icon(glyph(r.icon)).size(15.0).color(c.warning_foreground.0))
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(1.0)
                            .child(
                                text(r.name.clone())
                                    .text_size(12.5)
                                    .weight(500)
                                    .color(c.foreground.0),
                            )
                            .child(
                                text(r.description.clone())
                                    .text_size(11.0)
                                    .color(c.muted_foreground.0)
                                    .wrap(1),
                            ),
                    )
                    .children(r.priority.map(|p| {
                        text(format!("{p:.0}"))
                            .text_size(11.0)
                            .family(mono())
                            .color(c.muted_foreground.0)
                            .tooltip("Priority")
                    })),
            );
        }
        list
    };
    col()
        .grow(1.0)
        .child(toolbar)
        .child(body)
        .child(el().h(4.0))
}

/// The legacy rows' icon names as icons.
fn glyph(name: &str) -> Icon {
    match name {
        "sun" => icons::SUN,
        "lightbulb" => icons::LIGHTBULB,
        "box" => icons::APERTURE,
        "sliders-horizontal" => icons::SLIDERS_HORIZONTAL,
        "cloud" => icons::CLOUD,
        _ => icons::LAMP,
    }
}

fn toggle(s: &mut Studio, which: &'static str) {
    s.local.lighting_menu = if s.local.lighting_menu == Some(which) {
        None
    } else {
        Some(which)
    };
}

/// Creates `kind` at the Scene camera's pivot (one undo step) and selects it.
fn create(s: &mut Studio, kind: Create) {
    s.local.lighting_menu = None;
    let (name, commands) = authoring::create_commands(kind, s.editor.state.camera.target);
    match s.editor.apply(commands, &format!("Create {name}"), None) {
        Ok(result) => {
            s.resolve();
            if let Some(id) = result["outputs"][0]["id"].as_str() {
                s.select(vec![id.to_owned()], false);
            }
        }
        Err(error) => s.editor.report_failure(&error),
    }
}

/// Appends `preset`'s override groups to the selected volumes (one undo step).
fn apply_preset(s: &mut Studio, preset: Preset) {
    s.local.lighting_menu = None;
    match authoring::preset_commands(&s.editor, &s.resolved, preset) {
        Ok((commands, why)) => s.apply(commands, why),
        Err(error) => s.editor.report_failure(&*error),
    }
}
