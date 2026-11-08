//! The Scene view: a native layer the renderer presents into (`SCENE_LAYER`), with the UI
//! floating over it: a tool and view toolbar, the renderer's status while it starts, and the
//! frame rate. Navigation follows Unity and Unreal: right-drag looks around, Alt+left-drag
//! orbits, middle-drag pans, the wheel dollies; a left click picks (the host reads the
//! renderer's entity channel), Shift adds.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{Align, El, Handled, Justify, PointerPhase, el, icon, native, row, text};
use stillkit::{Button, Cursor, Rgba};

use super::{SCENE_LAYER, Studio, commands};
use crate::shell::commands::Target;
use crate::state::Tool;

/// A pick the host resolves against the renderer after the frame: where (0..1 across the
/// view), and whether it adds to the selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    /// Horizontal place across the view, 0..1.
    pub u: f32,
    /// Vertical place across the view, 0..1.
    pub v: f32,
    /// Shift held: toggle in the selection.
    pub additive: bool,
}

pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let mut view = native(SCENE_LAYER)
        .key("scene/view")
        .grow(1.0)
        .w_full()
        .focusable()
        .label("Scene view")
        .cursor(Cursor::Crosshair)
        .on_pointer(navigate)
        .on_wheel(|s: &mut Studio, w, _| {
            s.editor.state.camera.zoom(w.delta);
            true
        })
        .on_key(
            |s: &mut Studio, key, tx| match commands::for_key(key.0, tx.modifiers()) {
                Some(id) => {
                    commands::run(s, id, &Target::None);
                    Handled::Yes
                }
                None => Handled::No,
            },
        )
        .child(toolbar(s))
        .child(orientation(s));
    if !s.local.scene.ready {
        view = view.child(
            el().absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .align(Align::Center)
                .justify(Justify::Center)
                .child(
                    row()
                        .pad_x(16.0)
                        .pad_y(12.0)
                        .gap(10.0)
                        .align(Align::Center)
                        .radius(l.radius.md)
                        .fill(Rgba::new(0.0, 0.0, 0.0, 0.55))
                        .border(1.0, Rgba::new(1.0, 1.0, 1.0, 0.1))
                        .child(spinner(l, 16.0))
                        .child(
                            text(s.local.scene.message.clone())
                                .text_size(12.5)
                                .color(Rgba::new(1.0, 1.0, 1.0, 0.92)),
                        ),
                ),
        );
    }
    view
}

/// Camera navigation and picking from the pointer's story on the view.
fn navigate(s: &mut Studio, e: &stillkit::el::PointerEvent, _: &mut stillkit::el::Tx) -> bool {
    let camera = &mut s.editor.state.camera;
    match (e.phase, e.button) {
        (PointerPhase::Move, Button::Right) => camera.look([e.delta.x * 0.006, e.delta.y * 0.006]),
        (PointerPhase::Move, Button::Middle) => camera.pan([e.delta.x, e.delta.y]),
        (PointerPhase::Move, Button::Left) if e.mods.alt => {
            camera.orbit([e.delta.x * 0.008, e.delta.y * 0.008])
        }
        (PointerPhase::Up, Button::Left)
            if !e.mods.alt && e.travel().x.abs() + e.travel().y.abs() < 4.0 && e.size.0 > 0.0 =>
        {
            s.local.pick = Some(Pick {
                u: e.local.x / e.size.0,
                v: e.local.y / e.size.1,
                additive: e.mods.shift,
            });
            return false;
        }
        _ => return false,
    }
    true
}

/// The floating toolbar: transform tools, space and pivot, snapping, then view controls.
fn toolbar(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let st = &s.editor.state;
    let tool = |id: &'static str, glyph, label: &str, on: bool| {
        glass_toggle(l, glyph, label, on)
            .key(format!("scene/{id}"))
            .on_press(move |s: &mut Studio| commands::run(s, id, &Target::None))
    };
    let tools = glass_group(l)
        .child(tool(
            "view.tool.move",
            icons::MOVE_3D,
            "Move (W)",
            st.tool == Tool::Translate,
        ))
        .child(tool(
            "view.tool.rotate",
            icons::ROTATE_3D,
            "Rotate (E)",
            st.tool == Tool::Rotate,
        ))
        .child(tool(
            "view.tool.scale",
            icons::SCALE_3D,
            "Scale (R)",
            st.tool == Tool::Scale,
        ))
        .child(tool(
            "view.tool.universal",
            icons::MOVE,
            "Universal (T)",
            st.tool == Tool::Universal,
        ));
    let space = glass_group(l)
        .child(
            glass_label(
                l,
                if st.local { icons::BOX } else { icons::GLOBE },
                if st.local { "Local" } else { "World" },
            )
            .key("scene/space")
            .on_press(|s: &mut Studio| commands::run(s, "view.space", &Target::None)),
        )
        .child(
            glass_label(
                l,
                icons::CROSSHAIR,
                if st.centre { "Centre" } else { "Pivot" },
            )
            .key("scene/pivot")
            .on_press(|s: &mut Studio| commands::run(s, "view.pivot", &Target::None)),
        )
        .child(
            glass_toggle(l, icons::MAGNET, "Snapping", st.snapping)
                .key("scene/snap")
                .on_press(|s: &mut Studio| commands::run(s, "snap.toggle", &Target::None)),
        );
    let projection = if st.camera.projection == crate::state::CameraProjection::Perspective {
        "Perspective"
    } else {
        "Orthographic"
    };
    let views = glass_group(l)
        .child(
            glass_label(l, icons::VIDEO, projection)
                .key("scene/projection")
                .on_press(|s: &mut Studio| {
                    commands::run(s, "view.camera.perspective", &Target::None)
                }),
        )
        .child(
            glass_label(l, icons::FOCUS, "Frame")
                .key("scene/frame")
                .on_press(|s: &mut Studio| commands::run(s, "view.frame", &Target::None)),
        );
    row()
        .absolute(10.0, 10.0)
        .gap(8.0)
        .child(tools)
        .child(space)
        .child(el().w(8.0))
        .child(views)
}

/// The frame rate, top right, while frames run.
fn orientation(s: &Studio) -> El<Studio> {
    match s.local.scene.fps {
        Some((fps, ms)) if s.local.scene.ready => row()
            .absolute(0.0, 10.0)
            .w_full()
            .justify(Justify::End)
            .child(
                glass_group(&s.look).pad_x(8.0).child(
                    text(format!("{fps:.0} fps  {ms:.1} ms"))
                        .text_size(11.0)
                        .family(mono())
                        .color(Rgba::new(1.0, 1.0, 1.0, 0.8)),
                ),
            )
            .child(el().w(10.0)),
        _ => el(),
    }
}

/// A translucent group over the viewport, legible over any picture.
fn glass_group(l: &ara_kit::look::Look) -> El<Studio> {
    row()
        .h(30.0)
        .pad_x(3.0)
        .gap(2.0)
        .align(Align::Center)
        .radius(l.radius.md)
        .fill(Rgba::new(0.06, 0.06, 0.07, 0.72))
        .border(1.0, Rgba::new(1.0, 1.0, 1.0, 0.09))
}

/// An icon toggle in a viewport group: the accent while on.
fn glass_toggle(
    l: &ara_kit::look::Look,
    glyph: stillkit::icons::Icon,
    label: &str,
    on: bool,
) -> El<Studio> {
    let ink = if on {
        Rgba::new(1.0, 1.0, 1.0, 1.0)
    } else {
        Rgba::new(1.0, 1.0, 1.0, 0.72)
    };
    row()
        .size(24.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .radius(l.radius.sm)
        .fill(if on { l.c.primary.0 } else { Rgba::TRANSPARENT })
        .color(ink)
        .cascade_ink()
        .label(label)
        .tooltip(label)
        .checked(on)
        .hovered(|e| e.color(Rgba::new(1.0, 1.0, 1.0, 1.0)))
        .child(icon(glyph).size(15.0))
}

/// An icon and label button in a viewport group.
fn glass_label(l: &ara_kit::look::Look, glyph: stillkit::icons::Icon, label: &str) -> El<Studio> {
    row()
        .h(24.0)
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .radius(l.radius.sm)
        .color(Rgba::new(1.0, 1.0, 1.0, 0.78))
        .cascade_ink()
        .label(label)
        .hovered(|e| e.color(Rgba::new(1.0, 1.0, 1.0, 1.0)))
        .child(icon(glyph).size(14.0))
        .child(text(label).text_size(12.0))
}
