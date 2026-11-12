//! Glint's geometry kits, all built from the course table (`course.rs`): matte pale-stone decks
//! on white painted metal with navy kerbs, a polished navy inlay and lit coral chevrons down the
//! best line, the pylons that carry the course up out of the cloud, the sliding platforms,
//! spinner bars, pickups, rings, checkpoint arches, the cannon, the goal plaza, and the far
//! world (cloud banks, islands, the sun's disc). Plain metallic-roughness materials: stone is
//! matte, polish is a deliberate accent, glass is transmission glass; one strong accent (coral).
use super::course::{Course, Kind, Piece};
use crate::{
    kit::{Glass, Kit, Material},
    lock::Lock,
};
use anyhow::Result;
use glam::{Mat4, Quat, Vec2, Vec3};
use std::{
    f32::consts::{FRAC_PI_2, PI, TAU},
    path::Path,
};

/// The accent colour (linear): a hot coral.
pub const ACCENT: [f32; 3] = [1.0, 0.33, 0.12];
/// How far the pylons reach down (into the cloud), metres.
const DEPTH: f32 = 220.0;

fn stone() -> Material {
    Material::flat("pale stone", [0.68, 0.67, 0.64], 0.75, 0.0)
}
fn paint() -> Material {
    Material::flat("white paint", [0.74, 0.75, 0.77], 0.5, 0.0)
}
fn navy() -> Material {
    Material::flat("navy paint", [0.035, 0.08, 0.22], 0.45, 0.0)
}
/// The deliberate polish: a dark mirror-like stone inlay.
fn polish() -> Material {
    Material::flat("polished navy stone", [0.03, 0.06, 0.16], 0.06, 0.0)
}
fn coral() -> Material {
    Material::flat("coral lacquer", ACCENT, 0.25, 0.0)
}
fn chrome() -> Material {
    Material::flat("chrome", [0.93, 0.93, 0.95], 0.06, 1.0)
}
fn lit(name: &str) -> Material {
    Material::emitter(name, [1.0, 0.46, 0.22])
}
fn cool(name: &str) -> Material {
    Material::emitter(name, [0.55, 0.8, 1.0])
}

/// The surface palette, for the comparison spec.
pub fn palette() -> Vec<Material> {
    vec![stone(), paint(), navy(), polish(), coral(), chrome()]
}

/// A box in a piece's local frame: `at` is an offset from the piece centre.
fn local(kit: &mut Kit, material: usize, piece: &Piece, at: Vec3, half: Vec3, extra: Quat) {
    oriented(
        kit,
        material,
        piece.center + piece.rot * at,
        half,
        piece.rot * extra,
    );
}

fn oriented(kit: &mut Kit, material: usize, center: Vec3, half: Vec3, rot: Quat) {
    kit.cuboid(
        material,
        half * 2.0,
        Mat4::from_rotation_translation(rot, center),
        Vec2::ONE,
    );
}

/// A vertical cylinder standing on `base`.
fn column(kit: &mut Kit, material: usize, base: Vec3, radius: f32, height: f32, sides: u32) {
    kit.cylinder(
        material,
        radius,
        height,
        sides,
        Mat4::from_translation(base),
        Vec2::ONE,
    );
}

/// A ring of box segments in `frame`'s XY plane (its axis is `frame`'s Z), over `sweep` radians
/// from `from`.
#[allow(clippy::too_many_arguments)]
fn ring(
    kit: &mut Kit,
    material: usize,
    frame: Mat4,
    radius: f32,
    tube: Vec2,
    segments: u32,
    from: f32,
    sweep: f32,
) {
    let length = radius * sweep / segments as f32 * 1.08;
    for i in 0..segments {
        let a = from + (i as f32 + 0.5) / segments as f32 * sweep;
        kit.cuboid(
            material,
            Vec3::new(length, tube.x, tube.y),
            frame
                * Mat4::from_translation(Vec3::new(a.cos() * radius, a.sin() * radius, 0.0))
                * Mat4::from_rotation_z(a + FRAC_PI_2),
            Vec2::ONE,
        );
    }
}

/// A flat ring lying on the ground (its axis is Y).
fn floor_ring(
    kit: &mut Kit,
    material: usize,
    center: Vec3,
    radius: f32,
    width: f32,
    segments: u32,
) {
    let frame = Mat4::from_translation(center) * Mat4::from_rotation_x(FRAC_PI_2);
    ring(
        kit,
        material,
        frame,
        radius,
        Vec2::new(width, 0.03),
        segments,
        0.0,
        TAU,
    );
}

/// The static course: every piece, its trim and best-line markings, and the glass rails.
pub fn course(root: &Path, lock: &Lock, c: &Course) -> Result<String> {
    const STONE: usize = 0;
    const PAINT: usize = 1;
    const NAVY: usize = 2;
    const POLISH: usize = 3;
    const CORAL: usize = 4;
    const CHEVRON: usize = 5;
    const GLOW: usize = 6;
    const CHROME: usize = 7;
    const GLASS: usize = 8;
    let mut k = Kit::new(vec![
        stone(),
        paint(),
        navy(),
        polish(),
        coral(),
        lit("route chevron"),
        cool("underglow"),
        chrome(),
        // Transmission glass (IOR above 1, a thin wall): refracts and casts a faint tinted shadow.
        Material::solid_glass(
            "guard glass",
            Glass {
                ior: 1.5,
                thickness: 0.0,
                attenuation_color: [0.82, 0.94, 1.0],
                attenuation_distance: 1.5,
            },
        ),
    ]);
    let v = Vec3::new;
    for p in &c.pieces {
        let h = p.half;
        let kerb = if p.kind == Kind::Beam { 0.1 } else { 0.16 };
        // A painted body, a matte stone top between two navy kerbs, a keel and a cool glow below.
        local(
            &mut k,
            PAINT,
            p,
            v(0.0, -0.05, 0.0),
            v(h.x, h.y - 0.05, h.z),
            Quat::IDENTITY,
        );
        local(
            &mut k,
            STONE,
            p,
            v(0.0, h.y - 0.05, 0.0),
            v(h.x - kerb * 2.0, 0.05, h.z),
            Quat::IDENTITY,
        );
        for side in [-1.0f32, 1.0] {
            local(
                &mut k,
                NAVY,
                p,
                v(side * (h.x - kerb), h.y - 0.02, 0.0),
                v(kerb, 0.08, h.z),
                Quat::IDENTITY,
            );
            local(
                &mut k,
                PAINT,
                p,
                v(side * (h.x + 0.02), -0.1, 0.0),
                v(0.04, 0.12, h.z),
                Quat::IDENTITY,
            );
        }
        local(
            &mut k,
            GLOW,
            p,
            v(0.0, -h.y - 0.02, 0.0),
            v(h.x * 0.8, 0.02, h.z * 0.92),
            Quat::IDENTITY,
        );
        local(
            &mut k,
            PAINT,
            p,
            v(0.0, -h.y - 0.35, 0.0),
            v((h.x * 0.25).min(0.5), 0.35, h.z * 0.9),
            Quat::IDENTITY,
        );
        if p.kind == Kind::Pad {
            // A polished navy disc in a coral ring (pads are level), and corner posts with
            // lacquered caps.
            let top = p.center + Vec3::Y * h.y;
            let radius = h.x.min(h.z) * 0.52;
            column(&mut k, CORAL, top, radius + 0.14, 0.006, 56);
            column(&mut k, POLISH, top, radius, 0.012, 56);
            for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let at = v(sx * (h.x - 0.4), h.y, sz * (h.z - 0.4));
                local(
                    &mut k,
                    CHROME,
                    p,
                    at + v(0.0, 0.5, 0.0),
                    v(0.08, 0.5, 0.08),
                    Quat::IDENTITY,
                );
                local(
                    &mut k,
                    CORAL,
                    p,
                    at + v(0.0, 1.05, 0.0),
                    v(0.15, 0.08, 0.15),
                    Quat::IDENTITY,
                );
            }
        } else if p.kind != Kind::Beam {
            // The polished inlay down the middle: the best line.
            local(
                &mut k,
                POLISH,
                p,
                v(0.0, h.y + 0.004, 0.0),
                v(0.55, 0.004, h.z),
                Quat::IDENTITY,
            );
        }
        // Lit chevrons along the best line, pointing the way.
        let spacing = if p.kind == Kind::Pad { 2.0 } else { 3.0 };
        let count = ((h.z * 2.0 - 1.0) / spacing).floor().max(1.0) as u32;
        for i in 0..count {
            let z = -h.z + (h.z * 2.0 - (count - 1) as f32 * spacing) * 0.5 + i as f32 * spacing;
            for side in [-1.0f32, 1.0] {
                local(
                    &mut k,
                    CHEVRON,
                    p,
                    v(side * 0.17, h.y + 0.014, z),
                    v(0.06, 0.008, 0.28),
                    Quat::from_rotation_y(-side * 38f32.to_radians()),
                );
            }
        }
    }
    for w in &c.walls {
        oriented(&mut k, GLASS, w.center, w.half, w.rot);
        let cap = w.rot * v(0.0, w.half.y + 0.03, 0.0);
        oriented(
            &mut k,
            CHROME,
            w.center + cap,
            v(w.half.x.max(0.06), 0.03, w.half.z.max(0.06)),
            w.rot,
        );
    }
    k.write(root, lock, "glint_course")
}

/// What holds the course up: a pylon under every pad and slim piers under the decks, all
/// running down into the cloud.
pub fn supports(root: &Path, lock: &Lock, c: &Course) -> Result<String> {
    const PAINT: usize = 0;
    const CORAL: usize = 1;
    const BAND: usize = 2;
    const NAVY: usize = 3;
    let mut k = Kit::new(vec![paint(), coral(), cool("pylon band"), navy()]);
    let v = Vec3::new;
    for (index, p) in c.pieces.iter().enumerate() {
        let h = p.half;
        let under = p.center - Vec3::Y * h.y;
        if p.kind == Kind::Pad {
            let r = (h.x.min(h.z) * 0.36).clamp(1.3, 4.2);
            // A flared capital, the shaft, a coral collar and a lit band below it.
            column(&mut k, PAINT, under - v(0.0, 0.6, 0.0), r * 2.0, 0.6, 32);
            column(&mut k, NAVY, under - v(0.0, 1.5, 0.0), r * 1.45, 0.9, 32);
            column(
                &mut k,
                PAINT,
                under - v(0.0, DEPTH, 0.0),
                r,
                DEPTH - 1.5,
                28,
            );
            column(&mut k, CORAL, under - v(0.0, 5.2, 0.0), r * 1.06, 0.7, 28);
            column(&mut k, BAND, under - v(0.0, 5.6, 0.0), r * 1.07, 0.16, 28);
            continue;
        }
        // Decks and the beam: piers at two points; the banked turn: one under every third segment.
        let spots: &[f32] = match p.kind {
            Kind::Bank if index % 3 == 0 => &[0.0],
            Kind::Bank => &[],
            _ if h.z > 8.0 => &[-0.62, 0.62],
            _ => &[0.0],
        };
        for along in spots {
            let top = p.center + p.rot * v(0.0, -h.y - 0.7, along * h.z);
            let r = if p.kind == Kind::Beam { 0.3 } else { 0.5 };
            column(&mut k, NAVY, top - v(0.0, 0.5, 0.0), r * 1.7, 0.5, 20);
            column(&mut k, PAINT, top - v(0.0, DEPTH, 0.0), r, DEPTH - 0.5, 16);
            column(&mut k, CORAL, top - v(0.0, 3.4, 0.0), r * 1.08, 0.4, 16);
        }
    }
    k.write(root, lock, "glint_supports")
}

/// A sliding platform, centred on its own origin.
pub fn mover(root: &Path, lock: &Lock, half: Vec3) -> Result<String> {
    const PAINT: usize = 0;
    const STONE: usize = 1;
    const CORAL: usize = 2;
    const GLOW: usize = 3;
    const CHEVRON: usize = 4;
    const CHROME: usize = 5;
    let mut k = Kit::new(vec![
        paint(),
        stone(),
        coral(),
        cool("underglow"),
        lit("platform chevron"),
        chrome(),
    ]);
    let v = Vec3::new;
    oriented(
        &mut k,
        PAINT,
        v(0.0, -0.05, 0.0),
        v(half.x, half.y - 0.05, half.z),
        Quat::IDENTITY,
    );
    oriented(
        &mut k,
        STONE,
        v(0.0, half.y - 0.05, 0.0),
        v(half.x - 0.1, 0.05, half.z - 0.3),
        Quat::IDENTITY,
    );
    // Coral bumpers at the leading and trailing edges, lit chevrons on top, a thruster below.
    for end in [-1.0f32, 1.0] {
        oriented(
            &mut k,
            CORAL,
            v(0.0, half.y - 0.02, end * (half.z - 0.15)),
            v(half.x, 0.1, 0.15),
            Quat::IDENTITY,
        );
    }
    for i in 0..3 {
        let z = -1.1 + i as f32 * 1.1;
        for side in [-1.0f32, 1.0] {
            oriented(
                &mut k,
                CHEVRON,
                v(side * 0.34, half.y + 0.012, z),
                v(0.08, 0.008, 0.52),
                Quat::from_rotation_y(-side * 38f32.to_radians()),
            );
        }
    }
    oriented(
        &mut k,
        GLOW,
        v(0.0, -half.y - 0.02, 0.0),
        v(half.x * 0.8, 0.02, half.z * 0.8),
        Quat::IDENTITY,
    );
    column(&mut k, CHROME, v(0.0, -half.y - 0.9, 0.0), 0.7, 0.9, 24);
    column(&mut k, GLOW, v(0.0, -half.y - 0.96, 0.0), 0.55, 0.06, 24);
    k.write(root, lock, "glint_mover")
}

/// A spinner bar, centred on its pivot (which sits 0.5 m over the deck).
pub fn bar(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![coral(), paint(), chrome()]);
    let (len, t) = super::BAR;
    let seg = len / 8.0;
    for i in 0..8 {
        let x = -len * 0.5 + seg * (i as f32 + 0.5);
        k.cuboid(
            i % 2,
            Vec3::new(seg, t, t),
            Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
            Vec2::ONE,
        );
    }
    column(
        &mut k,
        2,
        Vec3::new(0.0, -t * 0.5 - 0.02, 0.0),
        0.32,
        0.3,
        24,
    );
    column(&mut k, 2, Vec3::new(0.0, -0.5, 0.0), 0.1, 0.5, 16);
    k.write(root, lock, "glint_bar")
}

/// A pickup orb: a lit core inside two chrome gimbal rings.
pub fn orb(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![
        Material::emitter("orb core", [1.0, 0.5, 0.2]),
        chrome(),
    ]);
    k.sphere(0, Vec3::ZERO, 0.38, 18, Vec2::ONE);
    ring(
        &mut k,
        1,
        Mat4::from_rotation_x(FRAC_PI_2),
        0.6,
        Vec2::splat(0.05),
        24,
        0.0,
        TAU,
    );
    ring(
        &mut k,
        1,
        Mat4::from_rotation_y(0.6),
        0.5,
        Vec2::splat(0.04),
        24,
        0.0,
        TAU,
    );
    k.write(root, lock, "glint_orb")
}

/// A ring the route passes through, 1 m in radius about its Z axis (the scene scales it):
/// a white hoop with a lit inner band and four fins.
pub fn hoop(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![paint(), lit("hoop band"), coral()]);
    ring(
        &mut k,
        0,
        Mat4::IDENTITY,
        1.0,
        Vec2::new(0.05, 0.11),
        48,
        0.0,
        TAU,
    );
    ring(
        &mut k,
        1,
        Mat4::IDENTITY,
        0.965,
        Vec2::new(0.012, 0.05),
        48,
        0.0,
        TAU,
    );
    for i in 0..4 {
        let a = (i as f32 + 0.5) * FRAC_PI_2;
        k.cuboid(
            2,
            Vec3::new(0.16, 0.05, 0.16),
            Mat4::from_translation(Vec3::new(a.cos() * 1.07, a.sin() * 1.07, 0.0))
                * Mat4::from_rotation_z(a + FRAC_PI_2),
            Vec2::ONE,
        );
    }
    k.write(root, lock, "glint_hoop")
}

/// A checkpoint arch across a pad: two white piers and a half-ring with a lit inner band, tall
/// enough to read from the start, and a lit ring on the floor.
pub fn gate(root: &Path, lock: &Lock) -> Result<String> {
    const PAINT: usize = 0;
    const CORAL: usize = 1;
    const LIT: usize = 2;
    const NAVY: usize = 3;
    let mut k = Kit::new(vec![paint(), coral(), lit("gate light"), navy()]);
    let v = Vec3::new;
    let (span, rise) = (4.6f32, 5.0f32);
    for side in [-1.0f32, 1.0] {
        oriented(
            &mut k,
            PAINT,
            v(side * span, rise * 0.5, 0.0),
            v(0.3, rise * 0.5, 0.4),
            Quat::IDENTITY,
        );
        oriented(
            &mut k,
            NAVY,
            v(side * span, 0.25, 0.0),
            v(0.42, 0.25, 0.55),
            Quat::IDENTITY,
        );
        oriented(
            &mut k,
            CORAL,
            v(side * span, rise, 0.0),
            v(0.36, 0.14, 0.48),
            Quat::IDENTITY,
        );
        oriented(
            &mut k,
            LIT,
            v(side * (span - 0.31), rise * 0.5 + 0.2, 0.0),
            v(0.02, rise * 0.5 - 0.5, 0.1),
            Quat::IDENTITY,
        );
    }
    let frame = Mat4::from_translation(v(0.0, rise, 0.0));
    ring(&mut k, PAINT, frame, span, Vec2::new(0.6, 0.8), 28, 0.0, PI);
    ring(
        &mut k,
        LIT,
        frame,
        span - 0.32,
        Vec2::new(0.04, 0.2),
        28,
        0.0,
        PI,
    );
    ring(
        &mut k,
        CORAL,
        frame,
        span + 0.32,
        Vec2::new(0.06, 0.5),
        28,
        0.0,
        PI,
    );
    floor_ring(&mut k, LIT, v(0.0, 0.02, 0.0), 2.6, 0.16, 40);
    k.write(root, lock, "glint_gate")
}

/// The cannon on its own origin (the breech point on the deck); `muzzle_offset` is the
/// muzzle's offset from it, so the barrel points along it.
pub fn cannon(root: &Path, lock: &Lock, muzzle_offset: Vec3) -> Result<String> {
    const PAINT: usize = 0;
    const CORAL: usize = 1;
    const CHROME: usize = 2;
    const LIT: usize = 3;
    const DARK: usize = 4;
    let mut k = Kit::new(vec![
        paint().with(0.3, 0.1),
        coral(),
        chrome(),
        lit("cannon light"),
        Material::flat("barrel interior", [0.02, 0.02, 0.025], 0.6, 0.0),
    ]);
    let v = Vec3::new;
    let axis = muzzle_offset.normalize();
    let tilt = Quat::from_rotation_arc(Vec3::Y, axis);
    // Cradle: a wide base disc with a lit rim, two wheels with chrome hubs.
    column(&mut k, PAINT, v(0.0, -0.5, 0.0), 2.4, 0.5, 40);
    column(&mut k, CORAL, v(0.0, -0.1, 0.0), 2.5, 0.1, 40);
    floor_ring(&mut k, LIT, v(0.0, -0.47, 0.0), 3.0, 0.14, 48);
    for side in [-1.0f32, 1.0] {
        let wheel = Mat4::from_translation(v(side * 1.6, 0.6, 0.6))
            * Mat4::from_rotation_z(-FRAC_PI_2 * side);
        k.cylinder(PAINT, 1.0, 0.35, 28, wheel, Vec2::ONE);
        k.cylinder(CHROME, 0.35, 0.42, 20, wheel, Vec2::ONE);
    }
    // Barrel from the breech to the muzzle, with coral bands and a lit muzzle ring.
    let length = muzzle_offset.length() + 1.6;
    let base = v(0.0, 1.0, 0.0) - axis * 1.4;
    let at = |p: Vec3| Mat4::from_translation(p) * Mat4::from_quat(tilt);
    k.cylinder(PAINT, 1.0, length, 40, at(base), Vec2::ONE);
    for f in [0.2f32, 0.5, 0.78] {
        k.cylinder(
            CORAL,
            1.06,
            0.34,
            40,
            at(base + axis * (length * f)),
            Vec2::ONE,
        );
    }
    k.cylinder(
        CHROME,
        1.12,
        0.22,
        40,
        at(base + axis * (length - 0.22)),
        Vec2::ONE,
    );
    k.cylinder(
        LIT,
        1.02,
        0.06,
        40,
        at(base + axis * (length - 0.06)),
        Vec2::ONE,
    );
    k.cylinder(
        DARK,
        0.78,
        0.07,
        32,
        at(base + axis * (length + 0.001)),
        Vec2::ONE,
    );
    k.write(root, lock, "glint_cannon")
}

/// The goal, on the plaza's centre (local +Z is the way the ball arrives): a polished court, a
/// great lit portal the shot passes through, four spires carrying a halo, and a beacon.
pub fn goal(root: &Path, lock: &Lock) -> Result<String> {
    const PAINT: usize = 0;
    const LIT: usize = 1;
    const CORAL: usize = 2;
    const BEACON: usize = 3;
    const FLOOR: usize = 4;
    const CHROME: usize = 5;
    let mut k = Kit::new(vec![
        paint(),
        Material::emitter("goal light", [1.0, 0.6, 0.36]),
        coral(),
        Material::emitter("beacon", [1.0, 0.72, 0.5]),
        // The one mirror floor in the level.
        Material::flat("polished white stone", [0.8, 0.8, 0.82], 0.05, 0.0),
        chrome(),
    ]);
    let v = Vec3::new;
    column(&mut k, FLOOR, v(0.0, 0.0, 0.0), 10.5, 0.05, 64);
    floor_ring(&mut k, LIT, v(0.0, 0.07, 0.0), 10.7, 0.3, 72);
    floor_ring(&mut k, LIT, v(0.0, 0.07, 0.0), 4.0, 0.2, 48);
    floor_ring(&mut k, CORAL, v(0.0, 0.06, 0.0), 7.4, 0.5, 64);
    // The portal stands 6 m before the centre, where the arc is still 6 m up.
    let portal = Mat4::from_translation(v(0.0, 7.6, -6.0));
    ring(
        &mut k,
        PAINT,
        portal,
        6.6,
        Vec2::new(0.7, 1.1),
        64,
        0.0,
        TAU,
    );
    ring(&mut k, LIT, portal, 6.2, Vec2::new(0.12, 0.6), 64, 0.0, TAU);
    ring(
        &mut k,
        CORAL,
        portal,
        7.0,
        Vec2::new(0.1, 0.7),
        64,
        0.0,
        TAU,
    );
    for side in [-1.0f32, 1.0] {
        oriented(
            &mut k,
            PAINT,
            v(side * 5.6, 2.2, -6.0),
            v(0.5, 2.2, 0.7),
            Quat::IDENTITY,
        );
        oriented(
            &mut k,
            CHROME,
            v(side * 5.6, 0.2, -6.0),
            v(0.8, 0.2, 1.0),
            Quat::IDENTITY,
        );
    }
    // Spires at the corners, taller at the back, each with a lit crown; a halo between them.
    for (sx, sz, height) in [
        (-1.0f32, -1.0f32, 22.0f32),
        (1.0, -1.0, 22.0),
        (-1.0, 1.0, 36.0),
        (1.0, 1.0, 36.0),
    ] {
        let at = v(sx * 11.2, 0.0, sz * 11.2);
        oriented(
            &mut k,
            PAINT,
            at + v(0.0, height * 0.5, 0.0),
            v(0.8, height * 0.5, 0.8),
            Quat::IDENTITY,
        );
        oriented(
            &mut k,
            CORAL,
            at + v(0.0, height * 0.72, 0.0),
            v(0.86, 0.5, 0.86),
            Quat::IDENTITY,
        );
        oriented(
            &mut k,
            LIT,
            at + v(0.0, height + 0.6, 0.0),
            v(0.5, 0.6, 0.5),
            Quat::IDENTITY,
        );
        oriented(
            &mut k,
            LIT,
            at + v(0.0, height * 0.4, 0.0),
            v(0.83, height * 0.3, 0.08),
            Quat::IDENTITY,
        );
        oriented(
            &mut k,
            LIT,
            at + v(0.0, height * 0.4, 0.0),
            v(0.08, height * 0.3, 0.83),
            Quat::IDENTITY,
        );
    }
    let halo = Mat4::from_translation(v(0.0, 19.0, 0.0)) * Mat4::from_rotation_x(FRAC_PI_2);
    ring(&mut k, PAINT, halo, 15.0, Vec2::new(0.8, 0.5), 72, 0.0, TAU);
    ring(&mut k, LIT, halo, 14.5, Vec2::new(0.14, 0.3), 72, 0.0, TAU);
    // The beacon: visible from the start, from anywhere on the course.
    column(&mut k, BEACON, v(0.0, 19.0, 0.0), 0.5, 240.0, 16);
    k.write(root, lock, "glint_goal")
}

/// A cloud bank: one soft mound about 50 m across made of large, heavily overlapping matte
/// white spheres, so only gentle billows stand out (the scene scales and flattens it).
pub fn cloud(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![Material::flat("cloud", [0.95, 0.96, 0.98], 1.0, 0.0)]);
    k.sphere(0, Vec3::ZERO, 13.0, 20, Vec2::ONE);
    for (count, radius, size, height) in [
        (6u32, 9.0f32, 9.5f32, -1.0f32),
        (8, 16.5, 7.5, -3.0),
        (2, 3.5, 8.5, 6.0),
    ] {
        for i in 0..count {
            let a = (i as f32 + 0.37 * count as f32) / count as f32 * TAU;
            let wobble = (i as f32 * 2.7 + radius).sin();
            let at = Vec3::new(
                a.cos() * (radius + wobble),
                height + wobble * 0.6,
                a.sin() * (radius - wobble),
            );
            k.sphere(0, at, size + wobble * 0.8, 16, Vec2::ONE);
        }
    }
    k.write(root, lock, "glint_cloud")
}

/// A far island: a pale deck on stepped rock, three towers with lit bands and a halo.
pub fn isle(root: &Path, lock: &Lock) -> Result<String> {
    const STONE: usize = 0;
    const ROCK: usize = 1;
    const PAINT: usize = 2;
    const LIT: usize = 3;
    const CORAL: usize = 4;
    const NAVY: usize = 5;
    let mut k = Kit::new(vec![
        stone(),
        Material::flat("rock", [0.42, 0.44, 0.5], 0.9, 0.0),
        paint(),
        lit("tower light"),
        coral(),
        navy(),
    ]);
    let v = Vec3::new;
    column(&mut k, STONE, v(0.0, -2.0, 0.0), 30.0, 2.0, 40);
    column(&mut k, CORAL, v(0.0, -2.6, 0.0), 30.4, 0.6, 40);
    let mut y = -2.6;
    for (r, height) in [
        (26.0f32, 6.0f32),
        (19.0, 8.0),
        (12.0, 10.0),
        (6.0, 16.0),
        (2.5, 60.0),
    ] {
        y -= height;
        column(&mut k, ROCK, v(0.0, y, 0.0), r, height, 28);
    }
    for (x, z, r, height) in [
        (-9.0f32, 4.0f32, 4.5f32, 46.0f32),
        (8.0, -6.0, 3.4, 30.0),
        (4.0, 12.0, 2.6, 20.0),
    ] {
        column(&mut k, PAINT, v(x, 0.0, z), r, height, 24);
        column(&mut k, NAVY, v(x, height, z), r * 1.15, 1.2, 24);
        column(&mut k, LIT, v(x, height + 1.2, z), r * 0.3, 5.0, 12);
        for band in [0.3f32, 0.55, 0.8] {
            column(&mut k, LIT, v(x, height * band, z), r * 1.02, 0.5, 24);
        }
    }
    let halo = Mat4::from_translation(v(-9.0, 38.0, 4.0)) * Mat4::from_rotation_x(FRAC_PI_2);
    ring(&mut k, PAINT, halo, 9.0, Vec2::new(0.6, 0.5), 40, 0.0, TAU);
    k.write(root, lock, "glint_isle")
}

/// The sun's disc: a lit sphere of 1 m radius the scene places far away along the sun's direction.
pub fn sun(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![Material::emitter("sun", [1.0, 0.82, 0.6])]);
    k.sphere(0, Vec3::ZERO, 1.0, 24, Vec2::ONE);
    k.write(root, lock, "glint_sun")
}
