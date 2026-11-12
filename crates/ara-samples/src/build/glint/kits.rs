//! Glint's geometry kits, all built from the course table (`course.rs`): bevelled pale-stone
//! decks on satin white metal with a lit underside, route strips in the accent colour, the
//! sliding platforms, spinner bars, pickups, checkpoint gates, the cannon and the goal arch.
//! Plain metallic-roughness materials; one strong accent (coral).
use super::course::{Course, Kind, Piece};
use crate::{
    kit::{Glass, Kit, Material, meters},
    lock::Lock,
};
use anyhow::Result;
use glam::{Mat4, Quat, Vec2, Vec3};
use std::{f32::consts::FRAC_PI_2, path::Path};

/// The accent colour (linear): a hot coral.
pub const ACCENT: [f32; 3] = [1.0, 0.33, 0.12];

/// A box in a piece's local frame: `at` is an offset from the piece centre.
fn local(kit: &mut Kit, material: usize, piece: &Piece, at: Vec3, half: Vec3, extra: Quat) {
    let center = piece.center + piece.rot * at;
    kit.cuboid(
        material,
        half * 2.0,
        Mat4::from_rotation_translation(piece.rot * extra, center),
        Vec2::ONE,
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

/// The static course.
pub fn course(root: &Path, lock: &Lock, c: &Course) -> Result<String> {
    const STONE: usize = 0;
    const PAINT: usize = 1;
    const ACCENT_M: usize = 2;
    const STRIP: usize = 3;
    const GLOW: usize = 4;
    const CHROME: usize = 5;
    const GLASS: usize = 6;
    let mut k = Kit::new(vec![
        Material::textured("pale marble", "marble_01").tinted([1.0, 1.0, 1.0]),
        Material::flat("satin white paint", [0.9, 0.92, 0.95], 0.3, 0.15),
        Material::flat("coral lacquer", ACCENT, 0.2, 0.0),
        Material::emitter("route strip", [1.0, 0.5, 0.3]),
        Material::emitter("underglow", [0.55, 0.8, 1.0]),
        Material::flat("chrome", [0.93, 0.93, 0.95], 0.06, 1.0),
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
    let stone_m = meters(lock, "marble_01") * 2.0;
    for p in &c.pieces {
        let h = p.half;
        // Body of satin paint with a limestone top plate inset a step, a chamfer along each
        // long edge, route strips, a fin and a glow under it.
        local(
            &mut k,
            PAINT,
            p,
            Vec3::new(0.0, -0.05, 0.0),
            Vec3::new(h.x, h.y - 0.05, h.z),
            Quat::IDENTITY,
        );
        k.cuboid(
            STONE,
            Vec3::new((h.x - 0.12) * 2.0, 0.1, (h.z - 0.12) * 2.0),
            Mat4::from_rotation_translation(
                p.rot,
                p.center + p.rot * Vec3::new(0.0, h.y - 0.05, 0.0),
            ),
            stone_m,
        );
        for sx in [-1.0f32, 1.0] {
            local(
                &mut k,
                PAINT,
                p,
                Vec3::new(sx * (h.x - 0.02), h.y - 0.04, 0.0),
                Vec3::new(0.09, 0.09, h.z),
                Quat::from_rotation_z(45f32.to_radians()),
            );
            let inset = if p.kind == Kind::Beam { 0.2 } else { 0.5 };
            local(
                &mut k,
                STRIP,
                p,
                Vec3::new(sx * (h.x - inset), h.y + 0.012, 0.0),
                Vec3::new(0.05, 0.012, h.z - 0.5),
                Quat::IDENTITY,
            );
        }
        // The cross strip that marks where a piece begins.
        if p.kind != Kind::Beam {
            local(
                &mut k,
                STRIP,
                p,
                Vec3::new(0.0, h.y + 0.012, -h.z + 0.5),
                Vec3::new(h.x - 0.5, 0.012, 0.04),
                Quat::IDENTITY,
            );
        }
        // Underglow and a keel fin make a floating piece read as hovering.
        local(
            &mut k,
            GLOW,
            p,
            Vec3::new(0.0, -h.y - 0.02, 0.0),
            Vec3::new(h.x * 0.82, 0.03, h.z * 0.92),
            Quat::IDENTITY,
        );
        if p.kind != Kind::Beam {
            local(
                &mut k,
                PAINT,
                p,
                Vec3::new(0.0, -h.y - 0.55, 0.0),
                Vec3::new(0.14, 0.55, h.z * 0.6),
                Quat::IDENTITY,
            );
        }
        if p.kind == Kind::Pad {
            // Corner posts with lacquered caps.
            for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                local(
                    &mut k,
                    CHROME,
                    p,
                    Vec3::new(sx * (h.x - 0.35), h.y + 0.45, sz * (h.z - 0.35)),
                    Vec3::new(0.09, 0.45, 0.09),
                    Quat::IDENTITY,
                );
                local(
                    &mut k,
                    ACCENT_M,
                    p,
                    Vec3::new(sx * (h.x - 0.35), h.y + 0.95, sz * (h.z - 0.35)),
                    Vec3::new(0.16, 0.1, 0.16),
                    Quat::IDENTITY,
                );
            }
        }
    }
    // Guard glass along the back and sides of the launch deck (their collision is in the scene).
    if let Some(p) = c.pieces.first() {
        let h = p.half;
        for (at, half) in [
            (
                Vec3::new(0.0, h.y + 0.5, -h.z + 0.1),
                Vec3::new(h.x, 0.5, 0.025),
            ),
            (
                Vec3::new(-h.x + 0.1, h.y + 0.5, 0.0),
                Vec3::new(0.025, 0.5, h.z),
            ),
            (
                Vec3::new(h.x - 0.1, h.y + 0.5, 0.0),
                Vec3::new(0.025, 0.5, h.z),
            ),
        ] {
            local(&mut k, GLASS, p, at, half, Quat::IDENTITY);
            local(
                &mut k,
                CHROME,
                p,
                at + Vec3::new(0.0, half.y + 0.03, 0.0),
                Vec3::new(half.x.max(0.05), 0.03, half.z.max(0.05)),
                Quat::IDENTITY,
            );
        }
    }
    k.write(root, lock, "glint_course")
}

/// A sliding platform, centred on its own origin.
pub fn mover(root: &Path, lock: &Lock, half: Vec3) -> Result<String> {
    const PAINT: usize = 0;
    const STONE: usize = 1;
    const ACCENT_M: usize = 2;
    const GLOW: usize = 3;
    let mut k = Kit::new(vec![
        Material::flat("satin white paint", [0.9, 0.92, 0.95], 0.3, 0.15),
        Material::textured("pale marble", "marble_01").tinted([1.0, 1.0, 1.0]),
        Material::flat("coral lacquer", ACCENT, 0.2, 0.0),
        Material::emitter("underglow", [0.55, 0.8, 1.0]),
    ]);
    let v = Vec3::new;
    let stone_m = meters(lock, "marble_01") * 2.0;
    oriented(
        &mut k,
        PAINT,
        v(0.0, -0.05, 0.0),
        v(half.x, half.y - 0.05, half.z),
        Quat::IDENTITY,
    );
    k.cuboid(
        STONE,
        v((half.x - 0.1) * 2.0, 0.1, (half.z - 0.1) * 2.0),
        Mat4::from_translation(v(0.0, half.y - 0.05, 0.0)),
        stone_m,
    );
    // Coral bumpers along the leading and trailing edges, chevron strips on top.
    for sz in [-1.0f32, 1.0] {
        oriented(
            &mut k,
            ACCENT_M,
            v(0.0, half.y - 0.02, sz * (half.z - 0.1)),
            v(half.x, 0.12, 0.12),
            Quat::IDENTITY,
        );
    }
    for i in 0..4 {
        let z = -half.z + 0.9 + i as f32 * 0.85;
        oriented(
            &mut k,
            ACCENT_M,
            v(0.0, half.y + 0.012, z),
            v(half.x - 0.5, 0.012, 0.08),
            Quat::IDENTITY,
        );
    }
    oriented(
        &mut k,
        GLOW,
        v(0.0, -half.y - 0.02, 0.0),
        v(half.x * 0.8, 0.03, half.z * 0.8),
        Quat::IDENTITY,
    );
    k.write(root, lock, "glint_mover")
}

/// A spinner bar, centred on its pivot (which sits 0.5 m over the deck).
pub fn bar(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![
        Material::flat("coral lacquer", ACCENT, 0.2, 0.0),
        Material::flat("satin white paint", [0.92, 0.93, 0.96], 0.3, 0.1),
        Material::flat("chrome", [0.93, 0.93, 0.95], 0.06, 1.0),
    ]);
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
    k.cylinder(
        2,
        0.32,
        0.3,
        24,
        Mat4::from_translation(Vec3::new(0.0, -t * 0.5 - 0.02, 0.0)),
        Vec2::ONE,
    );
    k.cylinder(
        2,
        0.1,
        0.5,
        16,
        Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0)),
        Vec2::ONE,
    );
    k.write(root, lock, "glint_bar")
}

/// A pickup orb: a lit coral core in a thin chrome ring.
pub fn orb(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![
        Material::emitter("orb core", [1.0, 0.55, 0.3]),
        Material::flat("chrome", [0.93, 0.93, 0.95], 0.06, 1.0),
    ]);
    k.sphere(0, Vec3::ZERO, 0.3, 14, Vec2::ONE);
    for i in 0..16 {
        let a = i as f32 / 16.0 * std::f32::consts::TAU;
        k.cuboid(
            1,
            Vec3::new(0.2, 0.035, 0.035),
            Mat4::from_translation(Vec3::new(a.cos() * 0.46, 0.0, a.sin() * 0.46))
                * Mat4::from_rotation_y(-a - FRAC_PI_2),
            Vec2::ONE,
        );
    }
    k.write(root, lock, "glint_orb")
}

/// A checkpoint gate: two chrome posts, a lacquered lintel with a lit band, a ring on the deck.
pub fn gate(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![
        Material::flat("chrome", [0.93, 0.93, 0.95], 0.06, 1.0),
        Material::flat("coral lacquer", ACCENT, 0.2, 0.0),
        Material::emitter("gate light", [1.0, 0.5, 0.3]),
    ]);
    let v = Vec3::new;
    for sx in [-1.0f32, 1.0] {
        k.cylinder(
            0,
            0.12,
            3.4,
            16,
            Mat4::from_translation(v(sx * 2.4, 0.0, 0.0)),
            Vec2::ONE,
        );
        k.cylinder(
            1,
            0.2,
            0.25,
            16,
            Mat4::from_translation(v(sx * 2.4, 3.4, 0.0)),
            Vec2::ONE,
        );
    }
    k.cuboid(
        1,
        v(5.0, 0.22, 0.3),
        Mat4::from_translation(v(0.0, 3.35, 0.0)),
        Vec2::ONE,
    );
    k.cuboid(
        2,
        v(4.6, 0.1, 0.34),
        Mat4::from_translation(v(0.0, 3.35, 0.0)),
        Vec2::ONE,
    );
    for i in 0..28 {
        let a = i as f32 / 28.0 * std::f32::consts::TAU;
        k.cuboid(
            2,
            v(0.36, 0.03, 0.14),
            Mat4::from_translation(v(a.cos() * 2.0, 0.02, a.sin() * 2.0))
                * Mat4::from_rotation_y(-a - FRAC_PI_2),
            Vec2::ONE,
        );
    }
    k.write(root, lock, "glint_gate")
}

/// The cannon on its own origin (the breech point on the deck); `muzzle_offset` is the
/// muzzle's offset from it, so the barrel points along it.
pub fn cannon(root: &Path, lock: &Lock, muzzle_offset: Vec3) -> Result<String> {
    const PAINT: usize = 0;
    const ACCENT_M: usize = 1;
    const CHROME: usize = 2;
    const LIT: usize = 3;
    const DARK: usize = 4;
    let mut k = Kit::new(vec![
        Material::flat("satin white paint", [0.9, 0.92, 0.95], 0.3, 0.15),
        Material::flat("coral lacquer", ACCENT, 0.2, 0.0),
        Material::flat("chrome", [0.93, 0.93, 0.95], 0.06, 1.0),
        Material::emitter("cannon light", [1.0, 0.5, 0.3]),
        Material::flat("barrel interior", [0.02, 0.02, 0.025], 0.6, 0.0),
    ]);
    let v = Vec3::new;
    let axis = muzzle_offset.normalize();
    let tilt = Quat::from_rotation_arc(Vec3::Y, axis);
    // Cradle: a wide base disc, two wheels with chrome hubs.
    k.cylinder(
        PAINT,
        2.4,
        0.5,
        40,
        Mat4::from_translation(v(0.0, -0.5, 0.0)),
        Vec2::ONE,
    );
    k.cylinder(
        ACCENT_M,
        2.5,
        0.1,
        40,
        Mat4::from_translation(v(0.0, -0.1, 0.0)),
        Vec2::ONE,
    );
    for sx in [-1.0f32, 1.0] {
        let wheel =
            Mat4::from_translation(v(sx * 1.6, 0.6, 0.6)) * Mat4::from_rotation_z(-FRAC_PI_2 * sx);
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
            ACCENT_M,
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

/// The goal: an arch of chrome pylons carrying a lit coral ring, and a pillar of light.
pub fn goal(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![
        Material::flat("chrome", [0.93, 0.93, 0.95], 0.06, 1.0),
        Material::emitter("goal ring", [1.0, 0.55, 0.32]),
        Material::flat("coral lacquer", ACCENT, 0.2, 0.0),
        Material::emitter("beacon", [1.0, 0.7, 0.5]),
    ]);
    let v = Vec3::new;
    for sx in [-1.0f32, 1.0] {
        k.cylinder(
            0,
            0.4,
            9.0,
            24,
            Mat4::from_translation(v(sx * 5.6, 0.0, 0.0)),
            Vec2::ONE,
        );
        k.cylinder(
            2,
            0.55,
            0.5,
            24,
            Mat4::from_translation(v(sx * 5.6, 0.0, 0.0)),
            Vec2::ONE,
        );
    }
    let segments = 56;
    for i in 0..segments {
        let a = i as f32 / segments as f32 * std::f32::consts::TAU;
        k.cuboid(
            1,
            v(0.46, 0.34, 0.34),
            Mat4::from_translation(v(a.cos() * 4.6, 6.2 + a.sin() * 4.6, 0.0))
                * Mat4::from_rotation_z(a + FRAC_PI_2),
            Vec2::ONE,
        );
    }
    // The pillar: visible from the start, from anywhere on the course.
    k.cylinder(
        3,
        0.22,
        260.0,
        14,
        Mat4::from_translation(v(0.0, 0.0, 0.0)),
        Vec2::ONE,
    );
    k.write(root, lock, "glint_goal")
}
