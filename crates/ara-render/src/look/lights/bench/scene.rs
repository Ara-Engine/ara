//! G1's scenes, rebuilt from T1's render-v3 stress recipes (`ara-dev` lab
//! `cases.v1.json`, `render_v3/fixture.rs` on `t3/renderer-gate-tooling` at
//! `ed1361b45`, not yet merged): a floor under a grid of 200 sphere lights (0.1 m
//! source radius) at authored 1/4/8 m ranges, power scaled by range squared, 16
//! occluder boxes (half glossy metal), seen from above. `extent` sets the light
//! spacing so lights per pixel meets the case's target.
//!
//! [`Motion::E3`] adds R5-1/E3-style dynamics to the static recipe (every light
//! flickers, every fourth orbits, four boxes move), so pops, flicker and moving
//! shadows are scored; [`Motion::MovingLights`] is T1's `moving_lights` recipe.
use crate::scene::{CameraSpec, Light};
use glam::{Affine3A, Quat, Vec3};
use std::f32::consts::{PI, SQRT_2};

/// How a case animates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Motion {
    /// T1's `lights` recipe: nothing moves.
    Static,
    /// T1's `moving_lights`: lights sway along x from the event frame.
    MovingLights,
    /// E3 dynamics: flicker, orbits and moving blockers from frame zero.
    E3,
}

/// One G1 case.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Case {
    /// Case id (T1's, with `-e3` for the E3-dynamics variant).
    pub id: String,
    pub motion: Motion,
    pub lights: u32,
    /// Authored range, metres.
    pub range: f32,
    /// Lights per pixel the layout targets.
    pub target_lpp: f32,
    /// T1's event frame (moving lights start there).
    pub event_frame: u32,
}

/// G1's cases: the LPP sweep at <= 3 and ~27 LPP at 1/4/8 m windows and the moving
/// lights, each static recipe also with E3 dynamics.
pub(crate) fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for (lpp, name) in [(2.4, "low"), (27., "dense")] {
        for range in [1., 4., 8.] {
            for motion in [Motion::Static, Motion::E3] {
                let suffix = if motion == Motion::E3 { "-e3" } else { "" };
                out.push(Case {
                    id: format!("lpp-{name}-{range}m{suffix}"),
                    motion,
                    lights: 200,
                    range,
                    target_lpp: lpp,
                    event_frame: 8,
                });
            }
        }
    }
    out.push(Case {
        id: "moving-200-lights".into(),
        motion: Motion::MovingLights,
        lights: 200,
        range: 4.,
        target_lpp: 2.4,
        event_frame: 8,
    });
    out
}

/// Material slots (`Bench` material table rows; the primary pass's caller words).
pub(crate) const FLOOR: u32 = 0;
pub(crate) const RED: u32 = 1;
pub(crate) const GLOSSY: u32 = 3;

/// A bench material: linear albedo, perceptual roughness, metalness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Material {
    pub albedo: [f32; 3],
    pub roughness: f32,
    pub metal: f32,
}

/// T1's materials: grey floor, red, white, glossy metal.
pub(crate) fn materials() -> Vec<Material> {
    let m = |albedo: [f32; 3], roughness: f32, metal: f32| Material {
        albedo,
        roughness,
        metal,
    };
    vec![
        m([0.7; 3], 0.7, 0.),
        m([0.7, 0.12, 0.05], 0.7, 0.),
        m([0.94; 3], 0.7, 0.),
        m([0.7; 3], 0.08, 1.),
    ]
}

/// Instance masks: the floor, and the boxes (blockers; the no-blocker runs drop them
/// from visibility).
pub(crate) const MASK_FLOOR: u8 = 1;
pub(crate) const MASK_BLOCKERS: u8 = 2;

/// One placed unit cube.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placed {
    /// Stable caller id.
    pub id: u64,
    pub world: Affine3A,
    pub material: u32,
    pub mask: u8,
}

/// The scene at one frame.
#[derive(Clone)]
pub(crate) struct Frame {
    pub lights: Vec<Light>,
    pub boxes: Vec<Placed>,
}

/// A case's fixed layout.
#[derive(Clone)]
pub(crate) struct Layout {
    pub case: Case,
    /// Light-field side scale, metres.
    pub extent: f32,
    pub camera: CameraSpec,
    /// T1's exposure: EV100 5, raised by log2(LPP / 2.4) for dense cases.
    pub ev100: f32,
    /// A fixed scene instead of the recipe (focused tests): its frame and floor.
    custom: Option<(Frame, Placed)>,
}

impl Layout {
    /// A still scene of its own (focused tests).
    pub(crate) fn custom(frame: Frame, floor: Placed, camera: CameraSpec) -> Self {
        let case = Case {
            id: "custom".into(),
            motion: Motion::Static,
            lights: frame.lights.len() as u32,
            range: 1.,
            target_lpp: 1.,
            event_frame: 0,
        };
        Self {
            case,
            extent: 1.,
            camera,
            ev100: 5.,
            custom: Some((frame, floor)),
        }
    }

    pub(crate) fn new(case: &Case) -> Self {
        let extent =
            (f64::from(case.lights) * std::f64::consts::PI * (f64::from(case.range) * 0.95).powi(2)
                / f64::from(case.target_lpp))
            .sqrt() as f32;
        Self {
            case: case.clone(),
            extent,
            camera: CameraSpec::LookAt {
                position: Vec3::new(0., extent + 10., 0.001),
                target: Vec3::ZERO,
                vertical_fov: 2. * (extent / (4. * SQRT_2 * (extent + 10.))).atan(),
            },
            ev100: 5. + (case.target_lpp / 2.4).log2(),
            custom: None,
        }
    }

    /// Light `i`'s grid position before motion.
    fn light_base(&self, i: u32) -> Vec3 {
        let n = self.case.lights;
        let cols = if n == 200 {
            20
        } else {
            (n as f32).sqrt().ceil() as u32
        };
        let x = ((i % cols) as f32 + 0.5) / cols as f32 - 0.5;
        let z = ((i / cols) as f32 + 0.5) / n.div_ceil(cols) as f32 - 0.5;
        Vec3::new(
            x * self.extent * SQRT_2,
            self.case.range * 0.3,
            z * self.extent / SQRT_2,
        )
    }

    /// The floor (T1: 0.2 m thick, at least 21.2 m wide).
    pub(crate) fn floor(&self) -> Placed {
        if let Some((_, floor)) = &self.custom {
            return *floor;
        }
        let side = self.extent.max(20. + 0.3 * 4.);
        Placed {
            id: 1,
            world: Affine3A::from_scale_rotation_translation(
                Vec3::new(side, 0.2, side),
                Quat::IDENTITY,
                Vec3::new(0., -0.1, 0.),
            ),
            material: FLOOR,
            mask: MASK_FLOOR,
        }
    }

    /// The scene at frame `frame` (60 Hz).
    pub(crate) fn frame(&self, frame: u32) -> Frame {
        if let Some((custom, _)) = &self.custom {
            return custom.clone();
        }
        let case = &self.case;
        let r = case.range;
        let t = frame as f32 / 60.;
        let event = frame.saturating_sub(case.event_frame) as f32 / 60.;
        let power = 180. * r * r;
        let lights = (0..case.lights)
            .map(|i| {
                let mut p = self.light_base(i);
                let mut lumens = power;
                let phase = i as f32 * 2.399963;
                match case.motion {
                    Motion::Static => {}
                    Motion::MovingLights => {
                        if frame >= case.event_frame {
                            p.x += ((event * 1.7 + i as f32).sin() - (i as f32).sin()) * r * 0.2;
                        }
                    }
                    Motion::E3 => {
                        lumens *= 0.8
                            + 0.12 * (t * 19. + phase).sin()
                            + 0.08 * (t * 31. + phase * 0.7).sin();
                        if i % 4 == 0 {
                            let a = t * 2. + phase;
                            p += Vec3::new(a.sin(), 0., a.cos()) * (0.18 * r);
                        }
                    }
                }
                Light {
                    position_range: [p.x, p.y, p.z, r],
                    color_lumens: [1., 0.85, 0.65, lumens],
                    direction_outer: [0., -1., 0., -1.],
                    parameters: [0., 1., 0.1, 1.],
                    ..Light::ZERO
                }
            })
            .collect();
        let boxes = (0..16u32)
            .map(|i| {
                let mut p = Vec3::new(
                    ((i % 4) as f32 - 1.5) * r * 0.4,
                    r * 0.125,
                    ((i / 4) as f32 - 1.5) * r * 0.4,
                );
                // The four central boxes are the E3 movers.
                if case.motion == Motion::E3 && matches!(i, 5 | 6 | 9 | 10) {
                    p.x += (t * 2. + i as f32).sin() * r * 0.15;
                    p.z += (t * 1.3 + i as f32).cos() * r * 0.1;
                }
                Placed {
                    id: 2000 + u64::from(i),
                    world: Affine3A::from_scale_rotation_translation(
                        Vec3::new(r * 0.12, r * 0.25, r * 0.12),
                        Quat::IDENTITY,
                        p,
                    ),
                    material: if i % 2 == 0 { GLOSSY } else { RED },
                    mask: MASK_BLOCKERS,
                }
            })
            .collect();
        Frame { lights, boxes }
    }
}

/// A unit cube's triangles (T1's 12-triangle `cube(12)`), counter-clockwise outward.
pub(crate) fn cube() -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for face in 0..6 {
        let (normal, u, v) = match face {
            0 => (Vec3::X, -Vec3::Z, Vec3::Y),
            1 => (-Vec3::X, Vec3::Z, Vec3::Y),
            2 => (Vec3::Y, Vec3::X, -Vec3::Z),
            3 => (-Vec3::Y, Vec3::X, Vec3::Z),
            4 => (Vec3::Z, Vec3::X, Vec3::Y),
            _ => (-Vec3::Z, -Vec3::X, Vec3::Y),
        };
        let start = positions.len() as u32;
        for (x, y) in [(0., 0.), (1., 0.), (0., 1.), (1., 1.)] {
            positions.push((normal * 0.5 + u * (x - 0.5) + v * (y - 0.5)).into());
        }
        indices.extend([start, start + 1, start + 3, start, start + 3, start + 2]);
    }
    (positions, indices)
}

/// T1's `cube(108)` (render_v3 geometry.rs): each face an `n` x `n` grid of quads (n = 3:
/// 108 triangles), the same winding and surface as [`cube`]. The occluder boxes use it.
pub(crate) fn cube_grid(n: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for face in 0..6 {
        let (normal, u, v) = match face {
            0 => (Vec3::X, -Vec3::Z, Vec3::Y),
            1 => (-Vec3::X, Vec3::Z, Vec3::Y),
            2 => (Vec3::Y, Vec3::X, -Vec3::Z),
            3 => (-Vec3::Y, Vec3::X, Vec3::Z),
            4 => (Vec3::Z, Vec3::X, Vec3::Y),
            _ => (-Vec3::Z, -Vec3::X, Vec3::Y),
        };
        let start = positions.len() as u32;
        for y in 0..=n {
            for x in 0..=n {
                let (s, t) = (x as f32 / n as f32, y as f32 / n as f32);
                positions.push((normal * 0.5 + u * (s - 0.5) + v * (t - 0.5)).into());
            }
        }
        for y in 0..n {
            for x in 0..n {
                let a = start + y * (n + 1) + x;
                let (b, c) = (a + 1, a + n + 1);
                indices.extend([a, b, c + 1, a, c + 1, c]);
            }
        }
    }
    (positions, indices)
}

/// The pixel footprint per metre of view distance for a vertical fov over `height`.
pub(crate) fn footprint(camera: &CameraSpec, height: u32) -> f32 {
    match *camera {
        CameraSpec::LookAt { vertical_fov, .. } => 2. * (vertical_fov * 0.5).tan() / height as f32,
        _ => 2. * (PI / 6.).tan() / height as f32,
    }
}
