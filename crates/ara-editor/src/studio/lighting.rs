//! Lighting: everything that lights the scene in one list (lights, sky and environment,
//! height fog, post-process volumes with their live weight at the Scene camera, reflection
//! probes), as Unreal's Lighting views gather them. A row selects its entity (the Inspector
//! edits it); Add creates a volume, fog or probe at the camera's pivot; Presets append
//! override groups to the selected volumes. The rows and commands are `crate::lighting`'s
//! (`model`, `authoring`).
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{Align, Axis, El, Justify, Side, col, el, icon, menu, row, text};
use stillkit::icons::Icon;

use super::Studio;
use crate::lighting::authoring::{self, Create, Preset};

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
    let rows = crate::lighting::model::rows(&s.resolved, &s.editor, query.trim());
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
    // Reflection probes recapture on demand (the scene is unchanged); a running capture
    // offers Cancel instead.
    let capture = if capturing(s) {
        button(l, Tone::Ghost, Size::Sm, Some(icons::X), "Cancel capture")
            .key("lighting/probe/cancel")
            .on_press(|s: &mut Studio| {
                super::commands::run(s, "lighting.probe.cancel", &crate::catalog::Target::None)
            })
    } else {
        enabled(
            button(
                l,
                Tone::Ghost,
                Size::Sm,
                Some(icons::REFRESH_CW),
                "Recapture",
            )
            .key("lighting/probe/recapture")
            .tooltip("Capture the selected reflection probes again")
            .on_press(|s: &mut Studio| {
                super::commands::run(s, "lighting.probe.recapture", &crate::catalog::Target::None)
            }),
            !probes(s, None).is_empty(),
        )
    };
    let toolbar = row()
        .h(super::style::TOOLBAR + 6.0)
        .pad_x(6.0)
        .gap(4.0)
        .align(Align::Center)
        .child(search)
        .child(add)
        .child(presets)
        .child(capture)
        .child(
            icon_button(
                l,
                Tone::Ghost,
                Size::Sm,
                icons::ACTIVITY,
                "Refresh probe status",
            )
            .key("lighting/probe/refresh")
            .on_press(|s: &mut Studio| {
                super::commands::run(s, "lighting.probe.refresh", &crate::catalog::Target::None)
            }),
        );
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
                    .children(mixer(s, &r.id))
                    .children(probe_state(s, &r.id).map(|state| {
                        text(state)
                            .text_size(11.0)
                            .color(c.muted_foreground.0)
                            .tooltip("The renderer's capture of this probe")
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

/// Colour temperatures a light can take from the Lighting list, warmest first.
const TEMPERATURES: &[(&str, f32)] = &[
    ("Candle", 1900.0),
    ("Tungsten", 2700.0),
    ("Halogen", 3200.0),
    ("Fluorescent", 4100.0),
    ("Daylight", 5600.0),
    ("Overcast", 6500.0),
    ("Clear sky", 10000.0),
];

/// A black body's colour at `kelvin` as linear RGB, brightest channel 1 (Tanner
/// Helland's fit of the Planckian locus, then the sRGB transfer undone).
pub(super) fn kelvin(kelvin: f32) -> [f32; 3] {
    let t = (kelvin / 100.0).clamp(10.0, 400.0);
    let red = match t <= 66.0 {
        true => 255.0,
        false => 329.698_73 * (t - 60.0).powf(-0.133_204_76),
    };
    let green = match t <= 66.0 {
        true => 99.470_8 * t.ln() - 161.119_57,
        false => 288.122_16 * (t - 60.0).powf(-0.075_514_85),
    };
    let blue = match t {
        t if t >= 66.0 => 255.0,
        t if t <= 19.0 => 0.0,
        t => 138.517_73 * (t - 10.0).ln() - 305.044_8,
    };
    let linear = [red, green, blue].map(|c| {
        let c = (c / 255.0).clamp(0.0, 1.0);
        match c <= 0.04045 {
            true => c / 12.92,
            false => ((c + 0.055) / 1.055).powf(2.4),
        }
    });
    let top = linear.iter().copied().fold(f32::MIN, f32::max).max(1e-6);
    linear.map(|c| c / top)
}

/// A light's row in the Lighting list as Unreal's Light Mixer has it: shown in the Scene
/// (the viewport's eye), its intensity in its units (typed, Enter applies), and its
/// colour, with colour temperatures to pick. Other rows have none.
fn mixer(s: &Studio, id: &str) -> Option<El<Studio>> {
    use ara_runtime::components::{Light, LightKind, LightUnits};
    let record = s.resolved.get(id)?.record.get("Light")?;
    let light: Light = serde_json::from_value(record.clone()).unwrap_or_default();
    let l = &s.look;
    let c = &l.c;
    let hidden = s.editor.state.hidden.contains(id);
    let units = match (light.kind, light.units) {
        (LightKind::Directional, _) => "lux",
        (_, LightUnits::Lumens) => "lm",
        (_, LightUnits::Candela) => "cd",
        (_, LightUnits::Nits) => "nits",
    };
    let eye = {
        let id = id.to_owned();
        icon_button(
            l,
            Tone::Ghost,
            Size::Sm,
            if hidden { icons::EYE_OFF } else { icons::EYE },
            if hidden {
                "Show in the Scene"
            } else {
                "Hide in the Scene"
            },
        )
        .key(format!("lighting/{id}/eye"))
        .on_press(move |s: &mut Studio| {
            s.act(
                "editor.visibility",
                serde_json::json!({"entity": id, "enabled": !hidden}),
            )
        })
    };
    let intensity = {
        let id = id.to_owned();
        field(
            l,
            Size::Sm,
            format!("lighting/{id}/intensity"),
            None,
            "",
            &format!("{}", (light.intensity * 100.0).round() / 100.0),
            move |f| {
                f.on_submit(move |s: &mut Studio, t| match t.trim().parse::<f32>() {
                    Ok(v) if v.is_finite() && v >= 0.0 => s.apply(
                        vec![serde_json::json!({"cmd": "property.set", "args": {"entity": id, "path": "Light.intensity", "value": v}})],
                        "Set light intensity",
                    ),
                    _ => super::note(s, "Intensity is a number, zero or more"),
                })
            },
        )
        .w(64.0)
        .tooltip("Intensity")
    };
    let [r, g, b, _] = light.color.0;
    let swatch = {
        let id = id.to_owned();
        let open = s.local.lighting_color.as_deref() == Some(id.as_str());
        let mut swatch = el()
            .key(format!("lighting/{id}/color"))
            .size(18.0)
            .radius(4.0)
            .fill(stillkit::Rgba::new(r, g, b, 1.0))
            // A hairline in the ink's tone: a white or black swatch keeps its edge in
            // either theme.
            .border(1.0, c.foreground.0.with_alpha(0.2))
            .label("Colour temperature")
            .tooltip("Colour temperature")
            .on_press({
                let id = id.clone();
                move |s: &mut Studio| {
                    s.local.lighting_color =
                        match s.local.lighting_color.as_deref() == Some(id.as_str()) {
                            true => None,
                            false => Some(id.clone()),
                        };
                }
            });
        if open {
            let items = TEMPERATURES
                .iter()
                .map(|(name, k)| {
                    let (id, k) = (id.to_owned(), *k);
                    let [r, g, b] = kelvin(k);
                    menu_item(
                        l,
                        &format!("lighting/{id}/kelvin/{k}"),
                        None,
                        &format!("{name} · {k:.0} K"),
                        None,
                        false,
                    )
                    .child(el().size(12.0).radius(3.0).fill(stillkit::Rgba::new(r, g, b, 1.0)))
                    .on_press(move |s: &mut Studio| {
                        s.local.lighting_color = None;
                        s.apply(
                            vec![serde_json::json!({"cmd": "property.set", "args": {"entity": id, "path": "Light.color", "value": [r, g, b, 1.0]}})],
                            "Set light colour temperature",
                        );
                    })
                })
                .collect();
            swatch = swatch.overlay(
                menu(menu_panel(l, items))
                    .side(Side::Bottom)
                    .align(Align::End)
                    .on_dismiss(|s: &mut Studio| s.local.lighting_color = None),
            );
        }
        swatch
    };
    Some(
        row()
            .gap(6.0)
            .align(Align::Center)
            .child(eye)
            .child(intensity)
            .child(
                text(units)
                    .text_size(11.0)
                    .w(28.0)
                    .color(c.muted_foreground.0),
            )
            .child(swatch),
    )
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

/// What a probe capture job is followed as (`Local::asset_jobs`).
const RECAPTURE: &str = "Recapture reflection probes";

/// The selected Reflection Probes (or `entity`, when given).
fn probes(s: &Studio, entity: Option<&str>) -> Vec<String> {
    match entity {
        Some(entity) => vec![entity.to_owned()],
        None => s
            .editor
            .state
            .selection
            .iter()
            .filter(|id| {
                s.resolved
                    .get(*id)
                    .is_some_and(|e| e.record.contains_key("ReflectionProbe"))
            })
            .cloned()
            .collect(),
    }
}

/// Queues a fresh capture of the selected Reflection Probes (derived data: the scene is
/// unchanged); the host loop follows the job, a failure says why.
pub(super) fn recapture(s: &mut Studio, entity: Option<&str>) -> ara_ui::Result<()> {
    let entities = probes(s, entity);
    if entities.is_empty() {
        return Err("Select one or more Reflection Probes to recapture".into());
    }
    for id in &entities {
        let value = s
            .resolved
            .get(id)
            .and_then(|e| e.record.get("ReflectionProbe"))
            .ok_or("That entity has no Reflection Probe")?;
        let probe: ara_components::render::ReflectionProbe =
            serde::Deserialize::deserialize(value)?;
        if let Some(reason) = probe.invalid() {
            return Err(reason.into());
        }
    }
    let reply = s.editor.action(
        "render.probe.recapture",
        serde_json::json!({"doc": s.editor.state.doc, "entities": entities, "why": RECAPTURE}),
    )?;
    let job = reply["job"]
        .as_str()
        .ok_or("The recapture returned no job")?
        .to_owned();
    s.local.asset_jobs.push((job, RECAPTURE.to_owned()));
    super::note(
        s,
        match entities.len() {
            1 => "Recapturing 1 reflection probe".to_owned(),
            n => format!("Recapturing {n} reflection probes"),
        },
    );
    Ok(())
}

/// Asks the renderer where each of the scene's reflection probes stands (captured, stale,
/// capturing, failed) for the Lighting list.
pub(super) fn refresh_probes(s: &mut Studio) -> ara_ui::Result<()> {
    let doc = s.editor.state.doc.clone();
    let answer = s.editor.action(
        "render.probes.query",
        serde_json::json!({"doc": doc, "since": null}),
    )?;
    let snapshot: ara_runtime_render::probes::ProbeSnapshot = serde_json::from_value(answer)
        .map_err(|e| format!("The renderer's probe report could not be read: {e}"))?;
    if snapshot.probes.iter().any(|row| row.doc != doc) {
        return Err("The probe report was for another scene".into());
    }
    s.local.probe_status = Some(snapshot);
    Ok(())
}

/// What the renderer last said of `entity`'s probe capture, in a few words.
fn probe_state(s: &Studio, entity: &str) -> Option<String> {
    use ara_runtime_render::probes::ProbeState;
    let row = s
        .local
        .probe_status
        .as_ref()?
        .probes
        .iter()
        .find(|r| r.entity == entity)?;
    let state = match row.state {
        ProbeState::Idle if row.generation == 0 => "Not captured".to_owned(),
        ProbeState::Idle | ProbeState::Done => "Captured".to_owned(),
        ProbeState::Queued => "Queued".to_owned(),
        ProbeState::Capturing | ProbeState::Progress => match row.progress {
            Some(p) => format!("Capturing {:.0}%", p * 100.0),
            None => "Capturing".to_owned(),
        },
        ProbeState::Cancelled => "Cancelled".to_owned(),
        ProbeState::Failed => "Failed".to_owned(),
    };
    Some(if row.stale {
        format!("{state} · stale")
    } else {
        state
    })
}

/// The renderer's report on one probe as JSON, for the clipboard (bug reports, agents).
pub(super) fn copy_probe(s: &mut Studio, entity: &str) -> ara_ui::Result<()> {
    let snapshot = s
        .local
        .probe_status
        .as_ref()
        .ok_or("Refresh the probe status first")?;
    let row = snapshot
        .probes
        .iter()
        .find(|r| r.entity == entity)
        .ok_or("The renderer has not reported that reflection probe yet")?;
    let report = serde_json::json!({
        "doc": s.editor.state.doc,
        "entity": entity,
        "version": snapshot.version,
        "capabilities": snapshot.capabilities,
        "probe": row,
    });
    s.local.copy = Some(ara_json::to_canonical_string(&report));
    Ok(())
}

/// Asks this window's running probe captures to stop.
pub(super) fn cancel(s: &mut Studio) -> ara_ui::Result<()> {
    let jobs: Vec<String> = s
        .local
        .asset_jobs
        .iter()
        .filter(|(_, why)| why == RECAPTURE)
        .map(|(job, _)| job.clone())
        .collect();
    if jobs.is_empty() {
        return Err("No probe capture is running".into());
    }
    for job in jobs {
        s.editor
            .action("render.probe.cancel", serde_json::json!({"job": job}))?;
    }
    super::note(s, "Probe capture cancelled");
    Ok(())
}

/// Whether a probe capture this window started is still running.
pub(super) fn capturing(s: &Studio) -> bool {
    s.local.asset_jobs.iter().any(|(_, why)| why == RECAPTURE)
}

/// Appends `preset`'s override groups to the selected volumes (one undo step).
pub(super) fn apply_preset(s: &mut Studio, preset: Preset) {
    s.local.lighting_menu = None;
    match authoring::preset_commands(&s.editor, &s.resolved, preset) {
        Ok((commands, why)) => s.apply(commands, why),
        Err(error) => s.editor.report_failure(&*error),
    }
}
