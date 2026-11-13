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
/// The second material family: a dark blue-grey stone for what holds the course up and for
/// the far islands, so the pale decks separate from the cloud.
fn slate() -> Material {
    Material::flat("slate", [0.2, 0.235, 0.3], 0.8, 0.0)
}
/// The darker running lane down a deck.
fn lane() -> Material {
    Material::flat("lane stone", [0.25, 0.29, 0.38], 0.7, 0.0)
}
/// Panel seams across a deck.
fn seam() -> Material {
    Material::flat("deck seam", [0.13, 0.15, 0.19], 0.85, 0.0)
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
    const BRIDGE: usize = 9;
    const LANE: usize = 10;
    const SEAM: usize = 11;
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
        // The bridge's slab: thick transmission glass, faintly blue through its depth.
        Material::solid_glass(
            "bridge glass",
            Glass {
                ior: 1.5,
                thickness: 0.16,
                attenuation_color: [0.7, 0.9, 1.0],
                attenuation_distance: 0.6,
            },
        ),
        lane(),
        seam(),
    ]);
    let v = Vec3::new;
    for p in &c.pieces {
        let h = p.half;
        if p.kind == Kind::Glass {
            // A glass bridge: a slab the ball rolls on, in a chrome frame on painted ribs, with
            // the chevrons laid on the glass. Nothing under it but the drop.
            local(
                &mut k,
                BRIDGE,
                p,
                v(0.0, h.y - 0.08, 0.0),
                v(h.x - 0.24, 0.08, h.z - 0.24),
                Quat::IDENTITY,
            );
            for side in [-1.0f32, 1.0] {
                local(
                    &mut k,
                    CHROME,
                    p,
                    v(side * (h.x - 0.12), h.y - 0.2, 0.0),
                    v(0.12, 0.2, h.z),
                    Quat::IDENTITY,
                );
                local(
                    &mut k,
                    CHROME,
                    p,
                    v(0.0, h.y - 0.2, side * (h.z - 0.12)),
                    v(h.x, 0.2, 0.12),
                    Quat::IDENTITY,
                );
                local(
                    &mut k,
                    NAVY,
                    p,
                    v(side * (h.x - 0.12), h.y + 0.03, 0.0),
                    v(0.12, 0.03, h.z),
                    Quat::IDENTITY,
                );
            }
            let ribs = (h.z * 2.0 / 3.0).floor() as u32;
            for i in 1..ribs {
                let z = -h.z + h.z * 2.0 * i as f32 / ribs as f32;
                local(
                    &mut k,
                    PAINT,
                    p,
                    v(0.0, h.y - 0.34, z),
                    v(h.x, 0.1, 0.07),
                    Quat::IDENTITY,
                );
                for side in [-1.0f32, 1.0] {
                    local(
                        &mut k,
                        CHEVRON,
                        p,
                        v(side * 0.17, h.y + 0.014, z + 1.5),
                        v(0.06, 0.008, 0.28),
                        Quat::from_rotation_y(-side * 38f32.to_radians()),
                    );
                }
            }
            continue;
        }
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
        // The spinner deck is dark from kerb to kerb: its own colour block in a wide shot.
        let dark = p.name == "Spinner deck";
        local(
            &mut k,
            if dark { LANE } else { STONE },
            p,
            v(0.0, h.y - 0.05, 0.0),
            v(h.x - kerb * 2.0, 0.05, h.z),
            Quat::IDENTITY,
        );
        // Each section has its own deck: the fast ones (descent, run-up, kicker) wear coral
        // kerbs, the climbs are stepped with bands, the rest keep navy.
        let fast = matches!(p.name, "Descent" | "Run-up" | "Kicker");
        let climb = matches!(p.name, "Climb" | "Tower ramp");
        for side in [-1.0f32, 1.0] {
            local(
                &mut k,
                if fast { CORAL } else { NAVY },
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
            // A darker running lane between two coral pinstripes, pale shoulders outside it.
            let lane = (h.x * 0.6).max(1.0);
            local(
                &mut k,
                LANE,
                p,
                v(0.0, h.y + 0.002, 0.0),
                v(lane, 0.002, h.z),
                Quat::IDENTITY,
            );
            // Every straight deck opens with a navy band from kerb to kerb: a section's
            // entry reads from across the course.
            if p.kind == Kind::Deck {
                local(
                    &mut k,
                    NAVY,
                    p,
                    v(0.0, h.y + 0.0052, -h.z + 1.3),
                    v(h.x - kerb * 2.0, 0.0022, 1.3),
                    Quat::IDENTITY,
                );
            }
            for side in [-1.0f32, 1.0] {
                local(
                    &mut k,
                    CORAL,
                    p,
                    v(side * lane, h.y + 0.003, 0.0),
                    v(0.045, 0.003, h.z),
                    Quat::IDENTITY,
                );
            }
            // Panel seams across the deck every 4 m; a climb is stepped instead, with a dark
            // band every 2 m.
            let step = if climb { 2.0 } else { 4.0 };
            let seams = (h.z * 2.0 / step).floor() as u32;
            for i in 1..seams.max(1) {
                let z = -h.z + h.z * 2.0 * i as f32 / seams as f32;
                local(
                    &mut k,
                    SEAM,
                    p,
                    v(0.0, h.y + 0.005, z),
                    v(h.x - kerb * 2.0, 0.0015, if climb { 0.45 } else { 0.05 }),
                    Quat::IDENTITY,
                );
            }
            // The kicker's lip is marked: a coral band across its last half metre.
            if p.name == "Kicker" {
                local(
                    &mut k,
                    CORAL,
                    p,
                    v(0.0, h.y + 0.006, h.z - 0.3),
                    v(h.x - kerb * 2.0, 0.003, 0.25),
                    Quat::IDENTITY,
                );
            }
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
        // Small lit studs along both kerbs: they carry the route through shade and at angles
        // where the sun does not reach the deck.
        if p.kind != Kind::Beam {
            let studs = (h.z * 2.0 / 3.0).floor().max(1.0) as u32;
            for i in 0..studs {
                let z = -h.z + h.z * 2.0 * (i as f32 + 0.5) / studs as f32;
                for side in [-1.0f32, 1.0] {
                    local(
                        &mut k,
                        CHEVRON,
                        p,
                        v(side * (h.x - kerb), h.y + 0.07, z),
                        v(0.05, 0.015, 0.16),
                        Quat::IDENTITY,
                    );
                }
            }
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
    // Under each spinner bar its sweep is painted on the deck: a dark disc in a coral ring.
    let reach = super::BAR.0 * 0.5 + 0.2;
    for bar in &c.bars {
        if let Some(deck) = c.pieces.iter().find(|p| p.name == "Spinner deck") {
            let top = Vec3::new(
                bar.pivot.x,
                deck.center.y + deck.half.y + 0.009,
                bar.pivot.z,
            );
            column(&mut k, SEAM, top, reach.min(deck.half.x - 0.5), 0.004, 48);
            floor_ring(
                &mut k,
                CORAL,
                top + Vec3::Y * 0.006,
                reach.min(deck.half.x - 0.5),
                0.12,
                48,
            );
        }
    }
    // The launch deck's start line: two rows of navy and white blocks ahead of the disc.
    if let Some(p) = c.pieces.iter().find(|p| p.name == "Launch deck") {
        let h = p.half;
        let block = 0.5;
        let across = ((h.x - 0.6) / block).floor() as i32;
        for row in 0..2 {
            for i in -across..across {
                if (i + row).rem_euclid(2) == 0 {
                    local(
                        &mut k,
                        NAVY,
                        p,
                        v(
                            (i as f32 + 0.5) * block,
                            h.y + 0.004,
                            h.z * 0.62 + row as f32 * block,
                        ),
                        v(block * 0.5, 0.004, block * 0.5),
                        Quat::IDENTITY,
                    );
                }
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

/// A tapered pylon hanging down from `top`: `radius` at the top, wider toward the cloud, with
/// a navy collar and a lit band under its head.
fn pylon(k: &mut Kit, top: Vec3, radius: f32) {
    const PAINT: usize = 0;
    const CORAL: usize = 1;
    const BAND: usize = 2;
    // The taper is all in the stretch seen above the cloud; below it the shaft is straight.
    // Slim ones flare the most: a 1 m shaft is a hair from across the course.
    const FLARE: f32 = 50.0;
    let wide = (radius * 2.2).max(radius + 3.4);
    let flare = Mat4::from_translation(top - Vec3::Y * FLARE);
    k.frustum(PAINT, wide, radius, FLARE, 28, flare);
    let base = Mat4::from_translation(top - Vec3::Y * DEPTH);
    k.frustum(PAINT, wide, wide, DEPTH - FLARE, 28, base);
    column(k, CORAL, top - Vec3::Y * 4.4, radius * 1.12, 0.7, 28);
    column(k, BAND, top - Vec3::Y * 4.8, radius * 1.14, 0.16, 28);
}

/// What holds the course up, all of it running down into the cloud: a pylon with a flared
/// head under every pad, one pier with a bracket under each long deck and two under the banked
/// turn, and a mast under each of the beam's two rings (the beam rests in them on cradles).
/// The other rings on the route hang from their deck.
pub fn supports(root: &Path, lock: &Lock, c: &Course) -> Result<String> {
    const PAINT: usize = 0;
    const NAVY: usize = 3;
    // Slate shafts and heads: the supports are the course's dark family against the cloud.
    let mut k = Kit::new(vec![slate(), coral(), cool("pylon band"), navy()]);
    let v = Vec3::new;
    let banks: Vec<usize> = (0..c.pieces.len())
        .filter(|i| c.pieces[*i].kind == Kind::Bank)
        .collect();
    for (index, p) in c.pieces.iter().enumerate() {
        let h = p.half;
        let under = p.center - Vec3::Y * h.y;
        match p.kind {
            Kind::Pad => {
                // A flared head: two steps out to the pad, then the shaft.
                let r = (h.x.min(h.z) * 0.4).clamp(1.6, 4.6);
                k.frustum(
                    PAINT,
                    r * 1.25,
                    r * 2.3,
                    1.0,
                    36,
                    Mat4::from_translation(under - v(0.0, 1.0, 0.0)),
                );
                k.frustum(
                    NAVY,
                    r,
                    r * 1.25,
                    0.9,
                    36,
                    Mat4::from_translation(under - v(0.0, 1.9, 0.0)),
                );
                pylon(&mut k, under - v(0.0, 1.9, 0.0), r);
            }
            Kind::Beam => {}
            _ => {
                // One pier under a long deck, the kicker's run-up and the detour's middle;
                // two under the banked turn.
                let long =
                    p.kind == Kind::Deck && (h.z >= 14.0 || matches!(p.name, "Run-up" | "Detour"));
                let third = banks.len() / 3;
                let bank = p.kind == Kind::Bank
                    && (index == banks[third] || index == banks[banks.len() - 1 - third]);
                if !long && !bank {
                    continue;
                }
                let top = p.center + p.rot * v(0.0, -h.y - 0.75, 0.0);
                // A bracket across the deck's underside, then the head and the shaft: as
                // thick as the deck allows, so it still reads as a column from far off.
                let r = (h.x * 0.4).clamp(0.9, 1.6);
                oriented(
                    &mut k,
                    NAVY,
                    top + v(0.0, 0.2, 0.0),
                    v(h.x * 0.8, 0.22, 0.9),
                    Quat::from_rotation_y(p.rot.to_euler(glam::EulerRot::YXZ).0),
                );
                k.frustum(
                    PAINT,
                    r,
                    (r * 1.8).min(h.x * 0.95),
                    0.9,
                    28,
                    Mat4::from_translation(top - v(0.0, 0.9, 0.0)),
                );
                pylon(&mut k, top - v(0.0, 0.9, 0.0), r);
            }
        }
    }
    for r in &c.rings {
        // A ring round a deck hangs from that deck on a cradle; the beam's two rings stand
        // on masts and carry the beam; the two on the cannon's arc float free.
        let foot = r.at - Vec3::Y * (r.radius * 1.03);
        let Some(under) = c
            .pieces
            .iter()
            .find(|p| (p.center - r.at).length() < p.half.z)
        else {
            continue;
        };
        let gap = (under.center.y - under.half.y) - (r.at.y - r.radius);
        if gap > 0.0 {
            oriented(
                &mut k,
                PAINT,
                foot + v(0.0, gap * 0.5, 0.0),
                v(0.3, gap * 0.5, 0.5),
                r.rot,
            );
        }
        if under.kind == Kind::Beam {
            k.frustum(
                NAVY,
                1.2,
                1.9,
                0.8,
                24,
                Mat4::from_translation(foot - v(0.0, 0.8, 0.0)),
            );
            pylon(&mut k, foot - v(0.0, 0.8, 0.0), 1.2);
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
    // A navy top: the sliders are their own colour block between the pale pads.
    let mut k = Kit::new(vec![
        paint(),
        navy(),
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
    // The barrel is polished metal: it mirrors the deck, the sky and the ball rolling up to it.
    k.cylinder(CHROME, 1.0, length, 40, at(base), Vec2::ONE);
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
/// great lit portal in a mirror casing the shot passes through, four spires carrying a halo, and a beacon.
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
        CHROME,
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
    // The great gate behind the plaza: an upright ring 56 m across with a lit inner band,
    // the thing to aim at from the launch deck.
    let gate = Mat4::from_translation(v(0.0, 22.0, 15.5));
    ring(&mut k, PAINT, gate, 28.0, Vec2::new(1.8, 1.4), 96, 0.0, TAU);
    ring(&mut k, LIT, gate, 26.6, Vec2::new(0.3, 0.9), 96, 0.0, TAU);
    ring(&mut k, CORAL, gate, 29.6, Vec2::new(0.5, 0.5), 96, 0.0, TAU);
    // The beacon: visible from the start, from anywhere on the course.
    column(&mut k, BEACON, v(0.0, 19.0, 0.0), 1.4, 240.0, 20);
    k.write(root, lock, "glint_goal")
}

/// A cloud bank: one smooth swell about 60 m across and 19 m tall, flat underneath, its top
/// rising in two or three broad billows (`seed` moves them, so the banks differ). One closed
/// surface with its own normals: no facets and no creases. Finely meshed, because the scene
/// scales it several times over and a low sun on coarse triangles sparkles.
pub fn cloud(root: &Path, lock: &Lock, seed: u32) -> Result<String> {
    let mut k = Kit::new(vec![Material::flat("cloud", [0.95, 0.95, 0.95], 1.0, 0.0)]);
    let s = seed as f32 * 1.7;
    k.blob(0, Vec3::ZERO, 80, |d| {
        // Low frequencies only: anything finer reads as lumps from across the course.
        let billow = 0.22 * (2.2 * d.x + s).sin() * (1.9 * d.z + 0.4 * s).sin()
            + 0.11 * (3.4 * d.x + 1.6 * d.z + 1.3 * s).sin();
        // Billows only on top; the underside is a shallow, smooth dish.
        let top = d.y.max(0.0);
        let radius = 30.0 * (1.0 + billow * (0.35 + 0.65 * top));
        let height = if d.y >= 0.0 { 0.55 } else { 0.14 };
        Vec3::new(d.x * radius, d.y * radius * height, d.z * radius)
    });
    k.write(root, lock, &format!("glint_cloud{seed}"))
}

/// The ball's trail at speed: a tapering lit streak 1 m long from its origin back along -Z
/// (the script stretches and turns it).
pub fn trail(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![
        lit("trail core"),
        Material::emitter("trail edge", [1.0, 0.72, 0.5]),
    ]);
    for (i, (width, from, to)) in [
        (0.34f32, 0.0f32, 0.45f32),
        (0.24, 0.45, 0.75),
        (0.13, 0.75, 1.0),
    ]
    .into_iter()
    .enumerate()
    {
        let mid = -(from + to) * 0.5;
        k.cuboid(
            0,
            Vec3::new(width, 0.012, to - from),
            Mat4::from_translation(Vec3::new(0.0, 0.0, mid)),
            Vec2::ONE,
        );
        if i < 2 {
            k.cuboid(
                1,
                Vec3::new(width * 0.3, 0.014, to - from),
                Mat4::from_translation(Vec3::new(0.0, 0.0, mid)),
                Vec2::ONE,
            );
        }
    }
    k.write(root, lock, "glint_trail")
}

/// The great arch over the banked turn, on its own origin at deck level (its plane is local
/// XY, the route passes through along Z): a half ring 21 m in radius with a lit inner band,
/// standing on two pylons. The largest thing between the start and the cannon, so the far
/// half of the route reads from the launch deck.
pub fn span(root: &Path, lock: &Lock) -> Result<String> {
    const PAINT: usize = 0;
    const NAVY: usize = 3;
    const LIT: usize = 4;
    let mut k = Kit::new(vec![
        paint(),
        coral(),
        cool("pylon band"),
        navy(),
        lit("arch light"),
    ]);
    let v = Vec3::new;
    let radius = 21.0;
    // The ring's centre sits 3 m under the deck, so its feet are below the route.
    let frame = Mat4::from_translation(v(0.0, -3.0, 0.0));
    ring(
        &mut k,
        PAINT,
        frame,
        radius,
        Vec2::new(1.3, 1.8),
        64,
        0.0,
        PI,
    );
    ring(
        &mut k,
        LIT,
        frame,
        radius - 0.72,
        Vec2::new(0.1, 0.7),
        64,
        0.0,
        PI,
    );
    ring(
        &mut k,
        1,
        frame,
        radius + 0.72,
        Vec2::new(0.12, 1.2),
        64,
        0.0,
        PI,
    );
    for side in [-1.0f32, 1.0] {
        let foot = v(side * radius, -3.0, 0.0);
        oriented(
            &mut k,
            NAVY,
            foot - v(0.0, 0.6, 0.0),
            v(1.5, 0.6, 1.6),
            Quat::IDENTITY,
        );
        pylon(&mut k, foot - v(0.0, 1.2, 0.0), 1.3);
    }
    k.write(root, lock, "glint_span")
}

/// The cannon's gantry, on its own origin on the cannon deck (local +Z is the shot): two
/// leaning legs outside the deck's edges meeting 30 m up under a lit ring that frames the
/// shot, with a crossbeam and a beacon mast.
pub fn gantry(root: &Path, lock: &Lock) -> Result<String> {
    const PAINT: usize = 0;
    const CORAL: usize = 1;
    const NAVY: usize = 3;
    const LIT: usize = 4;
    // A slate frame with coral and lit trim, like everything else that holds the course up.
    let mut k = Kit::new(vec![
        slate(),
        coral(),
        cool("pylon band"),
        navy(),
        lit("gantry light"),
    ]);
    let v = Vec3::new;
    let (spread, top, height) = (8.4f32, 2.6f32, 30.0f32);
    for side in [-1.0f32, 1.0] {
        let foot = v(side * spread, -0.8, 0.0);
        let head = v(side * top, height, 0.0);
        let lean = Quat::from_rotation_arc(Vec3::Y, (head - foot).normalize());
        k.frustum(
            PAINT,
            1.1,
            0.55,
            (head - foot).length(),
            24,
            Mat4::from_rotation_translation(lean, foot),
        );
        oriented(&mut k, NAVY, foot, v(1.5, 0.5, 1.5), Quat::IDENTITY);
        pylon(&mut k, foot - v(0.0, 0.5, 0.0), 1.1);
        // A lit strip up the inside of each leg.
        let mid = (foot + head) * 0.5 - v(side * 0.62, 0.0, 0.0);
        oriented(
            &mut k,
            LIT,
            mid,
            v(0.06, (head - foot).length() * 0.42, 0.14),
            lean,
        );
    }
    oriented(
        &mut k,
        CORAL,
        v(0.0, 17.0, 0.0),
        v(4.9, 0.35, 0.45),
        Quat::IDENTITY,
    );
    let crown = Mat4::from_translation(v(0.0, height + 3.4, 0.0));
    ring(
        &mut k,
        PAINT,
        crown,
        4.2,
        Vec2::new(0.55, 0.9),
        48,
        0.0,
        TAU,
    );
    ring(&mut k, LIT, crown, 3.9, Vec2::new(0.1, 0.5), 48, 0.0, TAU);
    column(&mut k, PAINT, v(0.0, height + 7.6, 0.0), 0.22, 9.0, 12);
    column(&mut k, LIT, v(0.0, height + 16.6, 0.0), 0.4, 1.2, 12);
    k.write(root, lock, "glint_gantry")
}

/// A far island: a pale deck on stepped rock, three towers with lit bands and a halo.
pub fn isle(root: &Path, lock: &Lock) -> Result<String> {
    const STONE: usize = 0;
    const ROCK: usize = 1;
    const PAINT: usize = 2;
    const LIT: usize = 3;
    const CORAL: usize = 4;
    let mut k = Kit::new(vec![
        stone(),
        // Slate rock and slate towers with lit bands: dark shapes on the pale cloud.
        slate(),
        slate(),
        lit("tower light"),
        coral(),
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
        column(&mut k, CORAL, v(x, height, z), r * 1.15, 1.2, 24);
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
