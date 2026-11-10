//! The Scene view: a native layer the renderer presents into (`SCENE_LAYER`), with the UI
//! floating over it: a tool and view toolbar, the renderer's status while it starts, and the
//! frame rate. Navigation follows Unity and Unreal: right-drag looks around, Alt+left-drag
//! orbits, middle-drag pans, the wheel dollies; a left click picks (the host reads the
//! renderer's entity channel), Shift adds.
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{Align, El, Handled, Justify, PointerPhase, col, el, icon, native, row, text};
use stillkit::{Button, Cursor, Rgba};

use super::{SCENE_LAYER, Studio, commands, gizmo};
use crate::shell::commands::Target;

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
    /// A right click: open the Entity (or Viewport) menu here, window design px.
    pub context: Option<(f32, f32)>,
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
            if s.local.flying {
                // While flying the wheel sets the fly speed (Unity, Unreal), a step at a time.
                let now = s
                    .editor
                    .state
                    .settings
                    .get("viewport.cameraSpeed")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(4.0);
                let next = (now * if w.delta > 0.0 { 1.25 } else { 0.8 }).clamp(0.5, 50.0);
                let next = (next * 2.0).round() / 2.0;
                s.act(
                    "editor.settings.set",
                    serde_json::json!({"key": "viewport.cameraSpeed", "value": next}),
                );
            } else {
                s.editor.state.camera.zoom(w.delta);
            }
            true
        })
        .on_key(|s: &mut Studio, key, tx| {
            // Shift+A adds at the pointer: the browser opens there and places on the ground
            // under it.
            if key.0 == 0x41 && tx.modifiers().shift && !tx.modifiers().ctrl {
                let (anchor, place) = match s.local.scene_pointer {
                    Some(super::state::ScenePointer { at, size, window }) => {
                        let rect = ara_ui::paint::Box2 {
                            x: 0.0,
                            y: 0.0,
                            w: size.0,
                            h: size.1,
                        };
                        let ground =
                            crate::shell::overlay::ground_point(&s.editor.state.camera, rect, at);
                        (
                            super::create::Anchor::Point(window.0, window.1),
                            ground.map(|p| [p.x, p.y, p.z]),
                        )
                    }
                    None => (super::create::Anchor::Key("scene/view"), None),
                };
                super::create::toggle(s, anchor, place);
                return Handled::Yes;
            }
            // ` opens the Overlays menu (Unity).
            if key.0 == 0xC0 && !tx.modifiers().ctrl && !tx.modifiers().meta {
                s.local.overlays_menu = !s.local.overlays_menu;
                return Handled::Yes;
            }
            // While flying, the fly keys steer instead of switching tools.
            if s.local.flying && super::FLY_KEYS.contains(&key.0) {
                return Handled::Yes;
            }
            if key == stillkit::el::KeyCode::ESCAPE && s.local.gizmo.is_some() {
                gizmo::end(s, true);
                return Handled::Yes;
            }
            match commands::for_key(key.0, tx.modifiers()) {
                Some(id) => {
                    commands::run(s, id, &Target::None);
                    Handled::Yes
                }
                None => Handled::No,
            }
        })
        .children(super::scene_overlays::layer(s))
        .children(s.local.marquee.map(|(a, b, _)| {
            let c = &l.c;
            el().absolute(a[0].min(b[0]), a[1].min(b[1]))
                .w((a[0] - b[0]).abs())
                .h((a[1] - b[1]).abs())
                .fill(c.primary.0.with_alpha(0.1))
                .border(1.0, c.primary.0.with_alpha(0.7))
        }));
    // Flying: the speed, which the wheel changes.
    if s.local.flying {
        let speed = s
            .editor
            .state
            .settings
            .get("viewport.cameraSpeed")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(4.0);
        view = view.child(
            stillkit::el::col()
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .justify(Justify::End)
                .align(Align::Center)
                .child(
                    glass_group(l)
                        .pad_x(10.0)
                        .gap(6.0)
                        .child(icon(icons::GAUGE).size(13.0).color(l.c.muted_foreground.0))
                        .child(
                            text(format!("{speed:.1} m/s · wheel to change"))
                                .text_size(11.5)
                                .color(l.c.foreground.0),
                        ),
                )
                .child(el().h(12.0)),
        );
    }
    // An asset over the view: it lands where the pointer lets go.
    if let (Some((_, _, at)), Some(rect)) = (&s.local.asset_drag, s.local.rects.get("scene/view"))
        && at.0 >= rect.left
        && at.0 <= rect.right
        && at.1 >= rect.top
        && at.1 <= rect.bottom
    {
        view = view.child(
            el().absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .border(2.0, l.c.primary.0),
        );
    }
    if !s.local.scene.ready {
        view = view.child(
            el().absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .align(Align::Center)
                .justify(Justify::Center)
                .child(
                    glass_group(l)
                        .h(36.0)
                        .pad_x(14.0)
                        .gap(10.0)
                        .child(spinner(l, 16.0))
                        .child(
                            text(s.local.scene.message.clone())
                                .text_size(12.5)
                                .color(l.c.foreground.0),
                        ),
                ),
        );
    }
    view
}

/// Camera navigation and picking from the pointer's story on the view.
fn navigate(s: &mut Studio, e: &stillkit::el::PointerEvent, _: &mut stillkit::el::Tx) -> bool {
    let at = [e.local.x, e.local.y];
    // The gizmo takes the left button before picking and orbiting.
    match (e.phase, e.button) {
        (PointerPhase::Hover, _) => {
            s.local.scene_pointer = Some(super::state::ScenePointer {
                at,
                size: e.size,
                window: (e.at.x, e.at.y),
            });
            return gizmo::hover(s, at, e.size);
        }
        (PointerPhase::Down, Button::Left) if !e.mods.alt && gizmo::begin(s, at, e.size) => {
            return true;
        }
        (PointerPhase::Move, Button::Left) if s.local.gizmo.is_some() => {
            gizmo::drive(s, at);
            return true;
        }
        (PointerPhase::Up, Button::Left) if s.local.gizmo.is_some() => {
            gizmo::end(s, false);
            return true;
        }
        (PointerPhase::Cancel, _) if s.local.gizmo.is_some() => {
            gizmo::end(s, true);
            return true;
        }
        _ => {}
    }
    let camera = &mut s.editor.state.camera;
    match (e.phase, e.button) {
        (PointerPhase::Down, Button::Right) => s.local.flying = true,
        (PointerPhase::Up, Button::Right) | (PointerPhase::Cancel, Button::Right) => {
            s.local.flying = false;
            s.local.held.clear();
            let still = e.travel().x.abs() + e.travel().y.abs() < 4.0;
            if e.phase == PointerPhase::Up && still && e.size.0 > 0.0 {
                s.local.pick = Some(Pick {
                    u: e.local.x / e.size.0,
                    v: e.local.y / e.size.1,
                    additive: false,
                    context: Some((e.at.x, e.at.y)),
                });
            }
        }
        (PointerPhase::Move, Button::Right) => camera.look([e.delta.x * 0.006, e.delta.y * 0.006]),
        (PointerPhase::Move, Button::Middle) => camera.pan([e.delta.x, e.delta.y]),
        (PointerPhase::Move, Button::Left) if e.mods.alt => {
            camera.orbit([e.delta.x * 0.008, e.delta.y * 0.008])
        }
        // A drag from empty space selects what its box covers (Shift adds).
        (PointerPhase::Move, Button::Left)
            if !e.mods.alt && e.travel().x.abs() + e.travel().y.abs() >= 4.0 =>
        {
            let start = [at[0] - e.travel().x, at[1] - e.travel().y];
            s.local.marquee = Some((start, at, e.mods.shift));
        }
        (PointerPhase::Up, Button::Left) if s.local.marquee.is_some() => {
            if let Some((from, to, additive)) = s.local.marquee.take() {
                let ids = marquee_hits(s, from, to, e.size);
                s.select(ids, additive);
            }
        }
        (PointerPhase::Cancel, _) if s.local.marquee.is_some() => s.local.marquee = None,
        (PointerPhase::Up, Button::Left)
            if !e.mods.alt && e.travel().x.abs() + e.travel().y.abs() < 4.0 && e.size.0 > 0.0 =>
        {
            s.local.pick = Some(Pick {
                u: e.local.x / e.size.0,
                v: e.local.y / e.size.1,
                additive: e.mods.shift,
                context: None,
            });
            return false;
        }
        _ => return false,
    }
    true
}

/// The entities whose origins project inside the box from `a` to `b` (view px) of a view
/// `size`, front to back; hidden and locked ones are left out, as clicks leave them.
fn marquee_hits(s: &Studio, a: [f32; 2], b: [f32; 2], size: (f32, f32)) -> Vec<String> {
    let rect = ara_ui::paint::Box2 {
        x: 0.0,
        y: 0.0,
        w: size.0,
        h: size.1,
    };
    let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
    let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
    let state = &s.editor.state;
    let camera = state.camera;
    let eye = camera.position();
    let mut hits: Vec<(f32, String)> = crate::world::world_transforms(&s.resolved)
        .into_iter()
        .filter(|(id, _)| !state.hidden.contains(id) && !state.locked.contains(id))
        .filter_map(|(id, m)| {
            let origin = m.w_axis.truncate();
            let [x, y] = crate::shell::overlay::project(&camera, rect, origin)?;
            ((x0..=x1).contains(&x) && (y0..=y1).contains(&y))
                .then(|| ((origin - eye).length(), id))
        })
        .collect();
    hits.sort_by(|p, q| p.0.total_cmp(&q.0));
    hits.into_iter().map(|(_, id)| id).collect()
}

/// The Orientation overlay: the axis gizmo, and the frame rate under it while frames run.
pub(super) fn orientation_widget(s: &Studio) -> El<Studio> {
    let mut corner = col().gap(6.0).align(Align::End).child(axes(s));
    if let Some((fps, ms)) = s.local.scene.fps.filter(|_| s.local.scene.ready) {
        corner = corner.child(
            row().h(18.0).pad_x(6.0).child(
                text(format!("{fps:.0} fps  {ms:.1} ms"))
                    .text_size(10.5)
                    .family(mono())
                    .color(s.look.c.muted_foreground.0),
            ),
        );
    }
    corner
}

/// The axis gizmo's size and its arms' length, design px.
const AXES: f32 = 76.0;
const ARM: f32 = 26.0;

/// The world axes as the camera sees them, from the gizmo's centre: screen offset, depth
/// (toward the viewer is negative), and which axis and sign.
fn axis_points(camera: &crate::state::Camera) -> Vec<([f32; 2], f32, usize, bool)> {
    let Some((right, up, forward)) = camera.basis() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (axis, dir) in [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z]
        .into_iter()
        .enumerate()
    {
        for positive in [true, false] {
            let d = if positive { dir } else { -dir };
            out.push((
                [d.dot(right) * ARM, -d.dot(up) * ARM],
                d.dot(forward),
                axis,
                positive,
            ));
        }
    }
    // Far first, so near ones draw on top.
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// Unity's scene gizmo: X, Y and Z turning with the camera, the positive ends labelled.
/// Clicking an end looks at the scene from that side; the centre switches projection.
fn axes(s: &Studio) -> El<Studio> {
    let camera = s.editor.state.camera;
    let points = axis_points(&camera);
    let tints = [
        Rgba::rgb8([240, 82, 82], 1.0),
        Rgba::rgb8([110, 205, 110], 1.0),
        Rgba::rgb8([80, 140, 255], 1.0),
    ];
    let drawn = points.clone();
    let mut gizmo = el()
        .key("scene/axes")
        .size(AXES)
        .radius(AXES / 2.0)
        .fill(Rgba::new(0.06, 0.06, 0.07, 0.0))
        .label("View orientation")
        .tooltip("Click an axis to look from that side; the centre switches projection")
        .hovered(|e| e.fill(Rgba::new(0.06, 0.06, 0.07, 0.45)))
        .child(
            stillkit::el::paint(move |f, rect, _| {
                let c = stillkit::Point::new(
                    rect.left + rect.width() / 2.0,
                    rect.top + rect.height() / 2.0,
                );
                let k = f.px(1.0);
                for (offset, depth, axis, positive) in &drawn {
                    let end = stillkit::Point::new(c.x + offset[0] * k, c.y + offset[1] * k);
                    let tint = tints[*axis];
                    // Ends pointing away fade.
                    let alpha = if *depth > 0.3 { 0.55 } else { 1.0 };
                    if *positive {
                        f.push(stillkit::Op::Line {
                            from: c,
                            to: end,
                            width: 2.0 * k,
                            color: tint.with_alpha(alpha),
                        });
                        let r = 7.0 * k;
                        f.fill(
                            stillkit::Rect::new(end.x - r, end.y - r, 2.0 * r, 2.0 * r),
                            r,
                            tint.with_alpha(alpha),
                        );
                    } else {
                        let r = 4.5 * k;
                        f.fill(
                            stillkit::Rect::new(end.x - r, end.y - r, 2.0 * r, 2.0 * r),
                            r,
                            tint.with_alpha(0.35 * alpha + 0.15),
                        );
                    }
                }
                let r = 3.0 * k;
                f.fill(
                    stillkit::Rect::new(c.x - r, c.y - r, 2.0 * r, 2.0 * r),
                    r,
                    Rgba::new(1.0, 1.0, 1.0, 0.85),
                );
            })
            .w_full()
            .h_full(),
        )
        .on_pointer(move |s: &mut Studio, e, _| {
            if e.phase != PointerPhase::Up || e.button != Button::Left {
                return e.phase == PointerPhase::Down;
            }
            let at = [e.local.x - AXES / 2.0, e.local.y - AXES / 2.0];
            let hit = axis_points(&s.editor.state.camera)
                .into_iter()
                .rev()
                .find(|(o, ..)| (o[0] - at[0]).hypot(o[1] - at[1]) < 9.0);
            match hit {
                Some((_, _, axis, positive)) => look_from(s, axis, positive),
                None if at[0].hypot(at[1]) < 8.0 => {
                    commands::run(s, "view.camera.perspective", &Target::None)
                }
                None => {}
            }
            true
        });
    // The positive ends' letters, over their discs.
    for (offset, depth, axis, positive) in points {
        if positive && depth <= 0.3 {
            gizmo = gizmo.child(
                text(["X", "Y", "Z"][axis])
                    .text_size(9.5)
                    .weight(800)
                    .color(Rgba::new(0.05, 0.05, 0.06, 0.95))
                    .text_align(stillkit::el::TextAlign::Center)
                    .w(14.0)
                    .absolute(AXES / 2.0 + offset[0] - 7.0, AXES / 2.0 + offset[1] - 6.5),
            );
        }
    }
    gizmo
}

/// Points the Scene camera from the side `axis` (`positive` or not) toward its target.
fn look_from(s: &mut Studio, axis: usize, positive: bool) {
    let sign = if positive { 1.0 } else { -1.0 };
    let (yaw, pitch) = match axis {
        0 => (sign * std::f32::consts::FRAC_PI_2, 0.0),
        1 => (s.editor.state.camera.yaw, sign * 1.54),
        _ => (if positive { 0.0 } else { std::f32::consts::PI }, 0.0),
    };
    let camera = &mut s.editor.state.camera;
    camera.yaw = yaw;
    camera.pitch = pitch;
}

/// A group of Scene controls over the image: the panels' strip tone, nearly opaque, no
/// border, so it reads as the same UI as the docks around it.
fn glass_group(l: &ara_kit::look::Look) -> El<Studio> {
    row()
        .h(26.0)
        .pad_x(2.0)
        .gap(1.0)
        .align(Align::Center)
        .radius(super::style::RADIUS + 1.0)
        .fill(super::style::tones(l).strip.with_alpha(0.94))
}
