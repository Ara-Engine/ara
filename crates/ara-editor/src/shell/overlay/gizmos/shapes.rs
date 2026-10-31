//! Per-component gizmo shapes, after Unity's: lights (point range sphere, spot cone
//! with inner cone and range cap, directional rays), camera frusta, audio min/max
//! spheres and cones, reverb zones with their fade, acoustic portals, primitive
//! colliders and triggers, character capsules, joint anchors/axes/limits, nav agents,
//! obstacles, links and surface bounds, and perception wedges. Detail selects how
//! much draws: unselected entities get a faint hint (or nothing, where Unity draws
//! nothing), the hovered icon's entity a lighter copy, the selection everything.
use super::parts::{Constraint, Shape, Tint, Volume};
use super::wire::Pen;
use super::*;
use glam::Quat;
use std::f32::consts::{PI, TAU};

/// Theme colours, resolved once per frame.
pub(super) struct Palette {
    pub light: [f32; 4],
    pub camera: [f32; 4],
    pub audio: [f32; 4],
    pub physics: [f32; 4],
    pub joint: [f32; 4],
    pub nav: [f32; 4],
    pub perception: [f32; 4],
    pub probe: [f32; 4],
    pub volume: [f32; 4],
    pub effect: [f32; 4],
    pub accent: [f32; 4],
    pub hover: [f32; 4],
    pub surface: [f32; 4],
}
impl Palette {
    pub fn new(color: &impl Fn(&str, f32) -> ara_ui::Result<[f32; 4]>) -> ara_ui::Result<Self> {
        Ok(Self {
            light: color("chart-3", 1.)?,
            camera: color("foreground", 1.)?,
            audio: color("info-foreground", 1.)?,
            physics: color("chart-6", 1.)?,
            joint: color("warning", 1.)?,
            nav: color("chart-2", 1.)?,
            perception: color("chart-5", 1.)?,
            probe: color("chart-2", 1.)?,
            volume: color("muted-foreground", 1.)?,
            effect: color("chart-4", 1.)?,
            accent: color("accent", 1.)?,
            hover: color("warning-foreground", 1.)?,
            surface: color("popover", 1.)?,
        })
    }
    /// An icon's tint.
    pub fn tint(&self, tint: Tint) -> [f32; 4] {
        match tint {
            Tint::Light([r, g, b]) => [r, g, b, 1.],
            Tint::Camera => self.camera,
            Tint::Audio => self.audio,
            Tint::Probe => self.probe,
            Tint::Volume => self.volume,
            Tint::Effect => self.effect,
        }
    }
}

/// Line half-width (logical px) and opacity for a detail level.
fn style(detail: Detail) -> (f32, f32) {
    match detail {
        Detail::Full => (0.6, 1.),
        Detail::Hover => (0.55, 0.75),
        Detail::Subtle => (0.45, 0.3),
    }
}
fn with_alpha(c: [f32; 4], alpha: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * alpha]
}
fn mix(a: [f32; 4], b: [f32; 3], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3],
    ]
}

/// An open arrowhead at `tip` pointing along `direction`, `size` logical px long.
fn arrowhead(pen: &mut Pen, tip: Vec3, direction: Vec3, size: f32, width: f32, c: [f32; 4]) {
    let length = size * pen.per_pixel(tip);
    let direction = direction.normalize_or_zero();
    let side = direction
        .cross(pen.toward(tip))
        .normalize_or(direction.any_orthonormal_vector());
    let back = tip - direction * length;
    for sign in [-1., 1.] {
        pen.line(tip, back + side * (length * 0.45 * sign), width, c);
    }
}

/// Draw one part of an entity at its world pose.
pub(super) fn draw(pen: &mut Pen, part: &Part, world: Mat4, detail: Detail, palette: &Palette) {
    let (width, alpha) = style(detail);
    pen.halo = detail != Detail::Subtle;
    let full = detail != Detail::Subtle;
    let (_, rotation, position) = world.to_scale_rotation_translation();
    let rotation = if rotation.is_finite() {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    };
    if !position.is_finite() {
        return;
    }
    let (right, up, forward) = (rotation * Vec3::X, rotation * Vec3::Y, rotation * -Vec3::Z);
    let pp = pen.per_pixel(position);
    match part.shape {
        Shape::PointLight { range, color } => {
            let c = with_alpha(mix(palette.light, color, 0.55), alpha);
            if full {
                pen.sphere(position, rotation, range, width, c);
            } else {
                pen.horizon(position, range, width, c);
            }
        }
        Shape::SpotLight {
            range,
            inner,
            outer,
            color,
        } => {
            let c = with_alpha(mix(palette.light, color, 0.55), alpha);
            // The cone meets the range sphere: `range` along its slant.
            let rim = |angle: f32| {
                (
                    position + forward * range * angle.cos(),
                    range * angle.sin(),
                )
            };
            let (end, radius) = rim(outer);
            pen.circle(end, right, up, radius, width, c, None);
            if !full {
                pen.line(position, end, width, c);
                return;
            }
            for direction in [right, -right, up, -up] {
                pen.line(position, end + direction * radius, width, c);
            }
            // The range cap: the sphere's arcs between the cone edges.
            for across in [right, up] {
                pen.arc(
                    position,
                    forward,
                    across,
                    range,
                    (-outer, outer),
                    width,
                    c,
                    None,
                );
            }
            if inner > 0.001 && inner < outer - 0.001 {
                let (end, radius) = rim(inner);
                pen.circle(end, right, up, radius, width, with_alpha(c, 0.5), None);
            }
        }
        Shape::DirectionalLight { color } => {
            let c = with_alpha(mix(palette.light, color, 0.55), alpha);
            if !full {
                let tip = position + forward * 36. * pp;
                pen.line(position, tip, width, c);
                arrowhead(pen, tip, forward, 7., width, c);
                return;
            }
            let (radius, length) = (12. * pp, 64. * pp);
            pen.circle(position, right, up, radius, width, c, None);
            for n in 0..8 {
                let angle = n as f32 / 8. * TAU;
                let from = position + (right * angle.cos() + up * angle.sin()) * radius;
                pen.line(from, from + forward * length, width, c);
            }
            let tip = position + forward * (length + 18. * pp);
            pen.line(position, tip, width, c);
            arrowhead(pen, tip, forward, 10., width, c);
        }
        Shape::Camera { fov } => {
            let c = with_alpha(palette.camera, alpha * 0.9);
            let aspect = 16. / 9.;
            let frame = |depth: f32| {
                let h = depth * fov.tan();
                let w = h * aspect;
                [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)]
                    .map(|(x, y)| position + forward * depth + right * (w * x) + up * (h * y))
            };
            let (near, far) = if full {
                (0.3, 20.)
            } else {
                (8. * pp, 40. * pp)
            };
            let (a, b) = (frame(near), frame(far));
            for n in 0..4 {
                pen.line(a[n], a[(n + 1) % 4], width, c);
                pen.line(b[n], b[(n + 1) % 4], width, c);
                pen.line(if full { a[n] } else { position }, b[n], width, c);
            }
            // An "up" marker over the far frame's top edge.
            let top = (b[2] + b[3]) * 0.5;
            let lift = (b[2] - b[1]).length() * 0.18;
            let apex = top + up * lift;
            pen.line(b[3].lerp(b[2], 0.35), apex, width, c);
            pen.line(apex, b[3].lerp(b[2], 0.65), width, c);
        }
        Shape::Audio { min, max, cone } => {
            if !full {
                return;
            }
            let c = with_alpha(palette.audio, alpha);
            pen.sphere(position, rotation, max, width, c);
            pen.sphere(position, rotation, min, width, with_alpha(c, 0.6));
            if let Some((inner, outer)) = cone {
                for (angle, strength) in [(outer, 1.), (inner, 0.5)] {
                    let end = position + forward * max * angle.cos();
                    let radius = max * angle.sin();
                    let cc = with_alpha(c, strength);
                    pen.circle(end, right, up, radius, width, cc, None);
                    for direction in [right, -right, up, -up] {
                        pen.line(position, end + direction * radius, width, cc);
                    }
                }
            }
        }
        Shape::ReverbBox { half, fade } => {
            let c = with_alpha(palette.audio, alpha);
            let pose = Mat4::from_rotation_translation(rotation, position);
            pen.cuboid(pose, Vec3::ZERO, half, width, c);
            let inner = half - Vec3::splat(fade);
            if full && fade > 0. && inner.min_element() > 0. {
                pen.cuboid(pose, Vec3::ZERO, inner, width, with_alpha(c, 0.45));
            }
        }
        Shape::Decal { half } => {
            // The box at the entity's full transform (scale included), and the
            // projection: an arrow from the top face's centre down local -Z.
            let c = with_alpha(palette.accent, alpha);
            let (scale, ..) = world.to_scale_rotation_translation();
            let half = half * scale.abs();
            let pose = Mat4::from_rotation_translation(rotation, position);
            pen.cuboid(pose, Vec3::ZERO, half, width, c);
            if full {
                let top = position - forward * half.z;
                let bottom = position + forward * half.z;
                pen.line(top, bottom, width, c);
                arrowhead(pen, bottom, forward, 10., width, c);
            }
        }
        Shape::ReverbSphere { radius, fade } => {
            let c = with_alpha(palette.audio, alpha);
            if full {
                pen.sphere(position, rotation, radius, width, c);
                if fade > 0. && fade < radius {
                    pen.horizon(position, radius - fade, width, with_alpha(c, 0.45));
                }
            } else {
                pen.horizon(position, radius, width, c);
            }
        }
        Shape::Portal {
            width: w,
            height: h,
        } => {
            let c = with_alpha(palette.audio, alpha);
            let corners = [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)]
                .map(|(x, y)| position + right * (w * 0.5 * x) + up * (h * 0.5 * y));
            for n in 0..4 {
                pen.line(corners[n], corners[(n + 1) % 4], width, c);
            }
            if full {
                let faint = with_alpha(c, 0.4);
                pen.line(corners[0], corners[2], width, faint);
                pen.line(corners[1], corners[3], width, faint);
                let tip = position + forward * 32. * pp;
                pen.line(position, tip, width, c);
                arrowhead(pen, tip, forward, 8., width, c);
            }
        }
        Shape::Collider {
            volume,
            centre,
            sensor,
        } => {
            if !full {
                return;
            }
            // Physics poses drop visual scale: dimensions are physics metres.
            let mut c = with_alpha(palette.physics, alpha);
            if sensor {
                c = with_alpha(mix(c, [1.; 3], 0.35), 0.75);
            }
            let pose = Mat4::from_rotation_translation(rotation, position);
            let at = pose.transform_point3(centre);
            match volume {
                Volume::Box(half) => pen.cuboid(pose, centre, half, width, c),
                Volume::Sphere(radius) => pen.sphere(at, rotation, radius, width, c),
                Volume::Capsule(half, radius) => {
                    pen.tube(at, rotation, half, radius, true, width, c)
                }
                Volume::Cylinder(half, radius) => {
                    pen.tube(at, rotation, half, radius, false, width, c)
                }
            }
        }
        Shape::Character { radius, height } => {
            if !full {
                return;
            }
            // Characters stand upright on their feet (the entity origin).
            let c = with_alpha(palette.physics, alpha);
            let half = (height * 0.5 - radius).max(0.);
            let centre = position + Vec3::Y * (height * 0.5).max(radius);
            pen.tube(centre, Quat::IDENTITY, half, radius, true, width, c);
        }
        Shape::Joint {
            kind,
            anchor,
            axis,
            normal,
            limits,
        } => {
            let c = with_alpha(palette.joint, alpha);
            let pose = Mat4::from_rotation_translation(rotation, position);
            let at = pose.transform_point3(anchor);
            pen.dot(at, if full { 3. } else { 2. }, c);
            if !full {
                return;
            }
            let (axis, normal) = (rotation * axis, rotation * normal);
            let size = 36. * pen.per_pixel(at);
            match kind {
                Constraint::Hinge => {
                    let tip = at + axis * size;
                    pen.line(at - axis * size * 0.4, tip, width, c);
                    arrowhead(pen, tip, axis, 7., width, c);
                    let binormal = axis.cross(normal);
                    let radius = size * 0.7;
                    let (lo, hi) = limits;
                    if hi > lo {
                        pen.arc(at, normal, binormal, radius, (lo, hi), width, c, None);
                        for angle in [lo, hi] {
                            let end = at + (normal * angle.cos() + binormal * angle.sin()) * radius;
                            pen.line(at, end, width, with_alpha(c, 0.6));
                        }
                    } else {
                        pen.circle(
                            at,
                            normal,
                            binormal,
                            radius,
                            width,
                            with_alpha(c, 0.6),
                            None,
                        );
                    }
                }
                Constraint::Slider | Constraint::Distance => {
                    let (lo, hi) = if limits.1 > limits.0 {
                        limits
                    } else {
                        (-size, size)
                    };
                    let (a, b) = (at + axis * lo, at + axis * hi);
                    pen.line(a, b, width, c);
                    let tick = normal * 6. * pen.per_pixel(at);
                    for end in [a, b] {
                        pen.line(end - tick, end + tick, width, c);
                    }
                }
                Constraint::Fixed | Constraint::Point | Constraint::SixDof => {
                    let binormal = axis.cross(normal);
                    for direction in [axis, normal, binormal] {
                        pen.line(
                            at - direction * size * 0.35,
                            at + direction * size * 0.35,
                            width,
                            c,
                        );
                    }
                    if kind == Constraint::Point {
                        pen.horizon(at, size * 0.25, width, with_alpha(c, 0.7));
                    }
                }
            }
        }
        Shape::NavAgent { radius, height } => {
            let c = with_alpha(palette.nav, alpha);
            if full {
                let centre = position + Vec3::Y * height * 0.5;
                pen.tube(
                    centre,
                    Quat::IDENTITY,
                    height * 0.5,
                    radius,
                    false,
                    width,
                    c,
                );
                let tip = position + forward.with_y(0.).normalize_or(Vec3::NEG_Z) * radius * 1.8;
                pen.line(position, tip, width, c);
                arrowhead(pen, tip, tip - position, 7., width, c);
            } else {
                pen.circle(position, Vec3::X, Vec3::Z, radius, width, c, None);
            }
        }
        Shape::NavObstacle { half } => {
            if full {
                // Navigation obstacles are axis-aligned.
                let c = with_alpha(palette.nav, alpha);
                pen.cuboid(Mat4::from_translation(position), Vec3::ZERO, half, width, c);
            }
        }
        Shape::NavLink {
            start,
            end,
            radius,
            arc,
            both,
        } => {
            let c = with_alpha(palette.nav, alpha);
            let point = |t: f32| start.lerp(end, t) + Vec3::Y * (arc * 4. * t * (1. - t));
            let n = 24;
            for k in 0..n {
                pen.line(
                    point(k as f32 / n as f32),
                    point((k + 1) as f32 / n as f32),
                    width,
                    c,
                );
            }
            if full {
                for (at, toward) in [(end, point(0.95)), (start, point(0.05))]
                    .into_iter()
                    .take(if both { 2 } else { 1 })
                {
                    arrowhead(pen, at, at - toward, 10., width, c);
                }
                for at in [start, end] {
                    pen.circle(
                        at,
                        Vec3::X,
                        Vec3::Z,
                        radius,
                        width,
                        with_alpha(c, 0.6),
                        None,
                    );
                }
            }
        }
        Shape::NavSurface { min, max } => {
            if full {
                let c = with_alpha(palette.nav, alpha * 0.8);
                pen.cuboid(
                    Mat4::IDENTITY,
                    (min + max) * 0.5,
                    (max - min) * 0.5,
                    width,
                    c,
                );
            }
        }
        Shape::Perceiver {
            sight,
            fov,
            peripheral,
            eye,
        } => {
            if !full {
                return;
            }
            let c = with_alpha(palette.perception, alpha);
            let at = position + Vec3::Y * eye;
            let ahead = forward.with_y(0.).normalize_or(Vec3::NEG_Z);
            let side = Vec3::Y.cross(ahead);
            if fov >= PI - 0.001 {
                pen.circle(at, ahead, side, sight, width, c, None);
            } else {
                pen.arc(at, ahead, side, sight, (-fov, fov), width, c, None);
                for angle in [-fov, fov] {
                    pen.line(
                        at,
                        at + (ahead * angle.cos() + side * angle.sin()) * sight,
                        width,
                        c,
                    );
                }
            }
            pen.circle(at, ahead, side, peripheral, width, with_alpha(c, 0.6), None);
        }
    }
}
