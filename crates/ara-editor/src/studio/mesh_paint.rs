//! Mesh Paint (Unreal's): per-instance vertex colours painted on the selected meshes in
//! the Scene view; the shared model never changes. This panel holds the brush: the tool
//! (brush, erase, fill, flood, pick), the paint and erase colours, the channels written,
//! and the brush's radius, strength and falloff. Strokes run in the host
//! (`crate::painter`); each commits one undo step.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{Align, El, col, el, row, text};

use super::Studio;
use crate::catalog::Target;
use crate::paint::{PaintSettings, PaintTool};

/// The tools in order: command, title, icon.
const TOOLS: [(PaintTool, &str, &str, stillkit::icons::Icon); 5] = [
    (PaintTool::Brush, "paint.tool.brush", "Paint", icons::BRUSH),
    (PaintTool::Erase, "paint.tool.erase", "Erase", icons::PENCIL),
    (PaintTool::Fill, "paint.tool.fill", "Fill", icons::BLEND),
    (
        PaintTool::Flood,
        "paint.tool.flood",
        "Flood",
        icons::DROPLETS,
    ),
    (PaintTool::Pick, "paint.tool.pick", "Pick", icons::PIPETTE),
];

/// Changes the brush (turning Mesh Paint on first if it is off).
pub(super) fn edit(s: &mut Studio, change: impl FnOnce(&mut PaintSettings)) {
    let mut settings = s.editor.state.paint.unwrap_or_default();
    change(&mut settings);
    s.editor.state.paint = Some(settings);
}

/// A colour as an opaque swatch.
fn swatch(l: &ara_kit::look::Look, color: [f32; 4], size: f32) -> El<Studio> {
    el().size(size)
        .radius(4.0)
        .fill(stillkit::Rgba::new(color[0], color[1], color[2], 1.0))
        .border(1.0, l.c.border.0)
}

/// A labelled number the brush has, typed into (Enter sets it, within `range`).
fn number(
    s: &Studio,
    key: &'static str,
    label: &str,
    value: f32,
    range: (f32, f32),
    set: fn(&mut PaintSettings, f32),
) -> El<Studio> {
    let l = &s.look;
    // A radius spans millimetres to metres: its track runs by magnitude.
    let log = range.1 / range.0.max(f32::EPSILON) > 100.0;
    row()
        .h(super::style::ROW)
        .gap(8.0)
        .align(Align::Center)
        .child(
            text(label.to_owned())
                .text_size(12.0)
                .color(l.c.muted_foreground.0)
                .w(90.0),
        )
        .child(
            super::style::slider(
                l,
                format!("{key}/track"),
                value,
                range,
                log,
                move |s: &mut Studio, v, _| {
                    edit(s, |p| set(p, v));
                },
            )
            .w(0.0)
            .grow(1.0),
        )
        .child(
            field(
                l,
                Size::Sm,
                key,
                None,
                "",
                &format!("{value:.2}"),
                move |f| {
                    f.select_all_on_focus().on_submit(move |s: &mut Studio, t| {
                        if let Ok(v) = t.trim().trim_end_matches('m').trim().parse::<f32>() {
                            edit(s, |p| set(p, v.clamp(range.0, range.1)));
                        }
                    })
                },
            )
            .w(64.0),
        )
}

/// The panel.
pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let on = s.editor.state.paint.is_some();
    let p = s.editor.state.paint.unwrap_or_default();
    let run = |id: &'static str| move |s: &mut Studio| super::commands::run(s, id, &Target::None);
    let meshes = s
        .editor
        .state
        .selection
        .iter()
        .filter(|id| {
            s.resolved
                .get(*id)
                .is_some_and(|e| e.record.contains_key("MeshRenderer"))
        })
        .count();
    let head = row()
        .gap(8.0)
        .align(Align::Center)
        .child(
            button(
                l,
                if on { Tone::Secondary } else { Tone::Primary },
                Size::Sm,
                Some(icons::BRUSH),
                if on { "Stop painting" } else { "Paint" },
            )
            .key("paint/mode")
            .on_press(run("paint.mode")),
        )
        .child(
            text(match (on, meshes) {
                (_, 0) => "Select meshes to paint".to_owned(),
                (false, 1) => "1 mesh selected".to_owned(),
                (false, n) => format!("{n} meshes selected"),
                (true, _) => "Paint in the Scene; Alt still orbits".to_owned(),
            })
            .text_size(11.5)
            .color(c.muted_foreground.0),
        );
    // The tools by name as well as icon (each reads at a glance, as Unreal's do).
    let keys: Vec<String> = TOOLS
        .iter()
        .map(|(t, ..)| format!("paint/tool/{}", t.name()))
        .collect();
    let options: Vec<(&str, Option<stillkit::icons::Icon>, &str)> = TOOLS
        .iter()
        .zip(&keys)
        .map(|((_, _, title, _), key)| (key.as_str(), None, *title))
        .collect();
    // The chosen tool shows whether or not painting is on: it is what the next stroke uses.
    let chosen = TOOLS
        .iter()
        .position(|(t, ..)| *t == p.tool)
        .unwrap_or(usize::MAX);
    let tools = segmented(l, &options, chosen, |i| {
        let id = TOOLS[i].1;
        move |s: &mut Studio| super::commands::run(s, id, &Target::None)
    });
    let mut quick = row().gap(4.0);
    for (id, color) in [
        ("paint.color.white", [1.0, 1.0, 1.0, 1.0]),
        ("paint.color.black", [0.0, 0.0, 0.0, 1.0]),
        ("paint.color.red", [1.0, 0.0, 0.0, 1.0]),
        ("paint.color.green", [0.0, 1.0, 0.0, 1.0]),
        ("paint.color.blue", [0.0, 0.0, 1.0, 1.0]),
    ] {
        quick = quick.child(
            swatch(l, color, 18.0)
                .key(format!("paint/{id}"))
                .label(id.trim_start_matches("paint.color."))
                .on_press(run(id)),
        );
    }
    let colors = row()
        .gap(10.0)
        .align(Align::Center)
        .child(
            col()
                .gap(2.0)
                .child(swatch(l, p.color, 26.0).key("paint/color"))
                .child(text("Paint").text_size(10.5).color(c.muted_foreground.0)),
        )
        .child(
            icon_button(
                l,
                Tone::Ghost,
                Size::Sm,
                icons::REPEAT,
                "Swap paint and erase",
            )
            .key("paint/swap")
            .on_press(run("paint.swap")),
        )
        .child(
            col()
                .gap(2.0)
                .child(swatch(l, p.erase, 26.0).key("paint/erase"))
                .child(text("Erase").text_size(10.5).color(c.muted_foreground.0)),
        )
        .child(el().w(8.0))
        .child(quick);
    let mut channels = row().gap(2.0);
    for (i, (id, name)) in [
        ("paint.channel.red", "R"),
        ("paint.channel.green", "G"),
        ("paint.channel.blue", "B"),
        ("paint.channel.alpha", "A"),
    ]
    .into_iter()
    .enumerate()
    {
        let lit = p.channels[i];
        channels = channels.child(
            super::style::chosen(l, row(), lit)
                .key(format!("paint/{id}"))
                .h(22.0)
                .w(28.0)
                .justify(stillkit::el::Justify::Center)
                .align(Align::Center)
                .radius(super::style::RADIUS)
                .label(format!("Write {name}"))
                .checked(lit)
                .on_press(run(id))
                .child(text(name).text_size(11.5).weight(600).color(if lit {
                    c.foreground.0
                } else {
                    c.muted_foreground.0
                })),
        );
    }
    let band = |title: &str| {
        text(title.to_owned())
            .text_size(11.5)
            .weight(600)
            .color(c.muted_foreground.0)
    };
    col()
        .grow(1.0)
        .pad(10.0)
        .gap(10.0)
        .child(head)
        .child(band("Tool"))
        .child(tools)
        .child(band("Colours"))
        .child(colors)
        .child(band("Channels"))
        .child(channels)
        .child(band("Brush"))
        .child(number(
            s,
            "paint/radius",
            "Radius (m)",
            p.radius,
            (0.005, 100.0),
            |p, v| p.radius = v,
        ))
        .child(number(
            s,
            "paint/strength",
            "Strength",
            p.strength,
            (0.0, 1.0),
            |p, v| p.strength = v,
        ))
        .child(number(
            s,
            "paint/falloff",
            "Falloff",
            p.falloff,
            (0.0, 1.0),
            |p, v| p.falloff = v,
        ))
}
