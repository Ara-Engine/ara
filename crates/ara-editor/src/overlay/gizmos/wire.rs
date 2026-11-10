//! The gizmo pen: world-space wires clipped to the view frustum and projected into
//! the viewport's logical pixels (the GPU antialiases at native resolution). Lines
//! buffer per entity; [`Pen::flush`] writes every halo of the batch before its lines,
//! so polyline joints never darken. Curved shapes pick their segment count from their
//! projected size, and the far side of spheres, capsules and rings draws faint, the
//! depth cue Unity's handles give hidden parts.
use super::*;
use glam::{Quat, Vec4};

/// Alpha kept by the far side of a closed shape.
const HIDDEN: f32 = 0.3;

struct Stroke {
    from: [f32; 2],
    to: [f32; 2],
    width: f32,
    color: [f32; 4],
}

pub(super) struct Pen {
    matrix: Mat4,
    rect: Box2,
    camera: Camera,
    eye: Vec3,
    /// Toward the viewer, for orthographic cameras.
    toward: Option<Vec3>,
    strokes: Vec<Stroke>,
    /// Dark halo under every stroke (off for subtle hints).
    pub halo: bool,
}

impl Pen {
    pub fn new(camera: &Camera, rect: Box2) -> Option<Self> {
        if ![rect.x, rect.y, rect.w, rect.h]
            .iter()
            .all(|v| v.is_finite())
            || rect.w <= 0.
            || rect.h <= 0.
        {
            return None;
        }
        let resolved = camera
            .spec()
            .resolve([rect.w.max(1.) as u32, rect.h.max(1.) as u32])
            .ok()?;
        let toward =
            (camera.projection == crate::state::CameraProjection::Orthographic).then(|| {
                resolved
                    .view
                    .inverse()
                    .z_axis
                    .truncate()
                    .normalize_or_zero()
            });
        Some(Self {
            matrix: resolved.projection * resolved.view,
            rect,
            camera: *camera,
            eye: camera.position(),
            toward,
            strokes: Vec::new(),
            halo: true,
        })
    }
    /// World metres per logical pixel at `at` (constant-size parts).
    pub fn per_pixel(&self, at: Vec3) -> f32 {
        self.camera
            .world_per_pixel(at, self.rect.h)
            .filter(|p| p.is_finite() && *p > 0.)
            .unwrap_or(0.01)
    }
    /// Unit vector from `at` toward the viewer.
    pub fn toward(&self, at: Vec3) -> Vec3 {
        self.toward
            .unwrap_or_else(|| (self.eye - at).normalize_or_zero())
    }
    /// Distance from the eye along the view (perspective) or zero (orthographic).
    pub fn distance(&self, at: Vec3) -> f32 {
        if self.toward.is_some() {
            0.
        } else {
            self.eye.distance(at)
        }
    }
    /// A world point in logical pixels, if in front of the camera.
    pub fn project(&self, at: Vec3) -> Option<[f32; 2]> {
        let clip = self.matrix * at.extend(1.);
        (clip.w > 1e-4 && clip.is_finite()).then(|| self.screen(clip))?
    }
    fn screen(&self, clip: Vec4) -> Option<[f32; 2]> {
        let ndc = clip.truncate() / clip.w;
        let point = [
            self.rect.x + (ndc.x + 1.) * 0.5 * self.rect.w,
            self.rect.y + (1. - ndc.y) * 0.5 * self.rect.h,
        ];
        point.iter().all(|v| v.is_finite()).then_some(point)
    }
    /// The viewport in logical pixels.
    pub fn rect(&self) -> Box2 {
        self.rect
    }

    /// One world segment, clipped to the frustum.
    pub fn line(&mut self, a: Vec3, b: Vec3, width: f32, color: [f32; 4]) {
        if color[3] <= 0.004 {
            return;
        }
        let Some([from, to]) =
            super::super::diagnostics::clip(self.matrix * a.extend(1.), self.matrix * b.extend(1.))
        else {
            return;
        };
        let (Some(from), Some(to)) = (self.screen(from), self.screen(to)) else {
            return;
        };
        self.strokes.push(Stroke {
            from,
            to,
            width,
            color,
        });
    }
    /// A screen-space dot of `radius` logical pixels at a world point.
    pub fn dot(&mut self, at: Vec3, radius: f32, color: [f32; 4]) {
        if let Some(p) = self.project(at) {
            self.strokes.push(Stroke {
                from: p,
                to: p,
                width: radius,
                color,
            });
        }
    }
    /// Segments for a circle of `radius` metres at `centre`: about one per 8 projected px.
    fn segments(&self, centre: Vec3, radius: f32, sweep: f32) -> usize {
        let pixels = radius / self.per_pixel(centre) * sweep;
        ((pixels / 8.).ceil() as usize).clamp(6, 72)
    }
    /// An arc of `radius` around `centre` in the plane of unit vectors `u` and `v`, from
    /// angle `a` to `b`. With `around`, segments whose outward direction from that point
    /// faces away from the viewer draw faint (the far side of a sphere or ring).
    #[allow(clippy::too_many_arguments)]
    pub fn arc(
        &mut self,
        centre: Vec3,
        u: Vec3,
        v: Vec3,
        radius: f32,
        (a, b): (f32, f32),
        width: f32,
        color: [f32; 4],
        around: Option<Vec3>,
    ) {
        if !(radius.is_finite() && radius > 0.) {
            return;
        }
        let n = self.segments(centre, radius, (b - a).abs());
        let point = |t: f32| {
            let angle = a + (b - a) * t;
            centre + (u * angle.cos() + v * angle.sin()) * radius
        };
        for k in 0..n {
            let (p, q) = (point(k as f32 / n as f32), point((k + 1) as f32 / n as f32));
            let mut c = color;
            if let Some(origin) = around {
                let mid = (p + q) * 0.5;
                if (mid - origin).dot(self.toward(mid)) < 0. {
                    c[3] *= HIDDEN;
                }
            }
            self.line(p, q, width, c);
        }
    }
    /// A full circle (see [`arc`](Self::arc)).
    #[allow(clippy::too_many_arguments)]
    pub fn circle(
        &mut self,
        centre: Vec3,
        u: Vec3,
        v: Vec3,
        radius: f32,
        width: f32,
        color: [f32; 4],
        around: Option<Vec3>,
    ) {
        self.arc(
            centre,
            u,
            v,
            radius,
            (0., std::f32::consts::TAU),
            width,
            color,
            around,
        );
    }
    /// The sphere's silhouette as seen from the camera (always fully visible).
    pub fn horizon(&mut self, centre: Vec3, radius: f32, width: f32, color: [f32; 4]) {
        let toward = self.toward(centre);
        let (centre, radius) = if self.toward.is_some() {
            (centre, radius)
        } else {
            let d = self.eye.distance(centre);
            if d <= radius * 1.001 {
                return; // Inside the sphere: no silhouette.
            }
            (
                centre + toward * (radius * radius / d),
                radius * (1. - (radius / d).powi(2)).sqrt(),
            )
        };
        let u = toward.any_orthonormal_vector();
        self.circle(centre, u, toward.cross(u), radius, width, color, None);
    }
    /// A sphere: three great circles in `rotation`'s frame (far halves faint) and the
    /// silhouette.
    pub fn sphere(
        &mut self,
        centre: Vec3,
        rotation: Quat,
        radius: f32,
        width: f32,
        color: [f32; 4],
    ) {
        let [x, y, z] = [Vec3::X, Vec3::Y, Vec3::Z].map(|a| rotation * a);
        for (u, v) in [(x, y), (y, z), (z, x)] {
            self.circle(centre, u, v, radius, width, color, Some(centre));
        }
        self.horizon(centre, radius, width, color);
    }
    /// A box of half extents `half` around local `centre`, transformed by `world`.
    pub fn cuboid(&mut self, world: Mat4, centre: Vec3, half: Vec3, width: f32, color: [f32; 4]) {
        let corner = |n: usize| {
            world.transform_point3(
                centre
                    + half
                        * Vec3::new(
                            if n & 1 == 0 { -1. } else { 1. },
                            if n & 2 == 0 { -1. } else { 1. },
                            if n & 4 == 0 { -1. } else { 1. },
                        ),
            )
        };
        for (a, b) in [
            (0, 1),
            (2, 3),
            (4, 5),
            (6, 7),
            (0, 2),
            (1, 3),
            (4, 6),
            (5, 7),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            self.line(corner(a), corner(b), width, color);
        }
    }
    /// A Y-axis cylinder (`caps`: rounded into a capsule) of `half` height between its
    /// ring centres, in the frame of `rotation` around `centre`.
    #[allow(clippy::too_many_arguments)]
    pub fn tube(
        &mut self,
        centre: Vec3,
        rotation: Quat,
        half: f32,
        radius: f32,
        caps: bool,
        width: f32,
        color: [f32; 4],
    ) {
        let [x, y, z] = [Vec3::X, Vec3::Y, Vec3::Z].map(|a| rotation * a);
        let (top, bottom) = (centre + y * half, centre - y * half);
        for ring in [top, bottom] {
            // Rings dim on the far side of the axis.
            let n = self.segments(ring, radius, std::f32::consts::TAU);
            for k in 0..n {
                let angle = |k: usize| k as f32 / n as f32 * std::f32::consts::TAU;
                let (p, q) = (
                    ring + (x * angle(k).cos() + z * angle(k).sin()) * radius,
                    ring + (x * angle(k + 1).cos() + z * angle(k + 1).sin()) * radius,
                );
                let mid = (p + q) * 0.5;
                let mut c = color;
                if (mid - ring).dot(self.toward(mid)) < 0. {
                    c[3] *= HIDDEN;
                }
                self.line(p, q, width, c);
            }
        }
        // Side lines on the silhouette, plus the front/back pair faint.
        let toward = self.toward(centre);
        let side = y.cross(toward).normalize_or(x);
        let front = side.cross(y).normalize_or(z);
        for (direction, strength) in [(side, 1.), (-side, 1.), (front, 1.), (-front, HIDDEN)] {
            let mut c = color;
            c[3] *= strength;
            self.line(
                top + direction * radius,
                bottom + direction * radius,
                width,
                c,
            );
        }
        if caps {
            for (cap, up) in [(top, y), (bottom, -y)] {
                for across in [side, front] {
                    self.arc(
                        cap,
                        across,
                        up,
                        radius,
                        (0., std::f32::consts::PI),
                        width,
                        color,
                        Some(cap),
                    );
                }
            }
        }
    }

    /// Append the buffered strokes: every halo first, then every line.
    /// Translucent strokes (faint parts, halos) are trimmed by their cap radius so
    /// neighbouring round caps meet instead of overlapping into darker beads.
    pub fn flush(&mut self, overlays: &mut Vec<Overlay>) {
        let line = |overlays: &mut Vec<Overlay>, s: &Stroke, width: f32, color: [f32; 4]| {
            let (a, b) = (Vec2::from(s.from), Vec2::from(s.to));
            let length = a.distance(b);
            let (from, to) = if color[3] < 0.99 && length > width * 2. {
                let trim = (b - a) / length * width;
                ((a + trim).into(), (b - trim).into())
            } else {
                (s.from, s.to)
            };
            overlays.push(Overlay::Line {
                from,
                to,
                width,
                color,
            });
        };
        if self.halo {
            for s in &self.strokes {
                line(overlays, s, s.width + 0.5, [0., 0., 0., 0.28 * s.color[3]]);
            }
        }
        for s in self.strokes.drain(..) {
            line(overlays, &s, s.width, s.color);
        }
    }
}
