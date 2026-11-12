//! Glint's geometry kits: the course (slabs with plated tops, amber edge lines and girders
//! below), the sliding platform, the spinner bar, the checkpoint pads, the goal, and the
//! foundry yard far below. All plain metallic-roughness materials from CC0 texture sets or
//! flat factors; one accent colour (amber) beside stone, steel and rust.
//! World frame as in the scene: Y up, +Z along the course.
use crate::{
    kit::{Kit, Material, meters},
    lock::Lock,
};
use anyhow::Result;
use glam::{Mat4, Vec2, Vec3};
use std::{f32::consts::FRAC_PI_2, path::Path};

/// Thickness of every slab, metres.
pub const SLAB: f32 = 1.0;

/// How a slab's top is finished.
#[derive(Clone, Copy, PartialEq)]
pub enum Top {
    /// Poured concrete (docks and pads).
    Concrete,
    /// Steel tread plate (runway, beam, islands).
    Plate,
}

/// A static slab: footprint, the height of its top face, its finish.
pub struct Slab {
    pub id: &'static str,
    pub name: &'static str,
    /// Centre X, centre Z.
    pub at: (f32, f32),
    /// Size X, size Z.
    pub size: (f32, f32),
    pub top: f32,
    pub finish: Top,
}

const fn slab(
    id: &'static str,
    name: &'static str,
    at: (f32, f32),
    size: (f32, f32),
    top: f32,
    finish: Top,
) -> Slab {
    Slab {
        id,
        name,
        at,
        size,
        top,
        finish,
    }
}

/// The static course along +Z, from the start dock to the goal island.
pub const SLABS: [Slab; 10] = [
    slab("e_s0", "Start dock", (0.0, 0.0), (8.0, 10.0), 0.0, Top::Concrete),
    slab("e_s1", "Runway", (0.0, 12.0), (4.0, 14.0), 0.0, Top::Plate),
    slab("e_s2", "Pad B", (0.0, 25.0), (6.0, 6.0), 0.8, Top::Concrete),
    slab("e_s3", "Checkpoint pad 1", (0.0, 40.0), (6.0, 6.0), 0.8, Top::Concrete),
    slab("e_s4", "Beam", (0.0, 53.0), (1.8, 20.0), 0.8, Top::Plate),
    slab("e_s5", "Checkpoint pad 2", (0.0, 66.0), (6.0, 6.0), 0.8, Top::Concrete),
    slab("e_s6", "Island 1", (0.0, 74.0), (3.4, 3.4), 0.2, Top::Plate),
    slab("e_s7", "Island 2", (2.0, 80.0), (3.4, 3.4), -0.5, Top::Plate),
    slab("e_s8", "Island 3", (-1.0, 86.0), (3.4, 3.4), -1.2, Top::Plate),
    slab("e_s9", "Goal island", (0.0, 94.0), (8.0, 8.0), -1.8, Top::Concrete),
];

/// Height of the yard floor under the course.
pub const YARD_FLOOR: f32 = -24.0;
/// The yard's chimneys: X, Z, radius, height above the floor.
pub const CHIMNEYS: [(f32, f32, f32, f32); 5] = [
    (-52.0, 30.0, 2.4, 44.0),
    (-64.0, 78.0, 2.0, 38.0),
    (56.0, 50.0, 2.6, 48.0),
    (70.0, 104.0, 2.2, 40.0),
    (-44.0, 118.0, 2.2, 34.0),
];

/// The two big furnaces beside the route: centre X, Z of the building (mouth faces -Z).
pub const FURNACES: [(f32, f32); 2] = [(26.0, 24.0), (-28.0, 60.0)];

/// The moving platform's size (X, Z).
pub const MOVER_SIZE: (f32, f32) = (3.2, 3.2);
/// The spinner bar's length, thickness.
pub const BAR: (f32, f32) = (3.6, 0.3);

fn slab_box(kit: &mut Kit, material: usize, min: Vec3, max: Vec3, uv: Vec2) {
    kit.cuboid(
        material,
        max - min,
        Mat4::from_translation((min + max) * 0.5),
        uv,
    );
}

/// The static course: plated tops, rust-steel bodies, amber edge lines and girders.
pub fn course(root: &Path, lock: &Lock) -> Result<String> {
    const BODY: usize = 0;
    const CONCRETE: usize = 1;
    const PLATE: usize = 2;
    const AMBER: usize = 3;
    const GIRDER: usize = 4;
    const RAIL: usize = 5;
    let mut k = Kit::new(vec![
        Material::textured("rust steel", "rusty_metal_02").tinted([0.55, 0.5, 0.46]),
        Material::textured("poured concrete", "concrete_floor_02").tinted([0.9, 0.88, 0.84]),
        Material::textured("tread plate", "metal_plate").tinted([0.95, 0.97, 1.0]),
        Material::flat("amber paint", [0.85, 0.42, 0.08], 0.5, 0.0),
        Material::flat("black girder steel", [0.06, 0.065, 0.07], 0.55, 1.0),
        Material::flat("galvanised rail", [0.72, 0.74, 0.76], 0.38, 1.0),
    ]);
    let body_m = meters(lock, "rusty_metal_02");
    let concrete_m = meters(lock, "concrete_floor_02");
    let plate_m = meters(lock, "metal_plate");
    let v = Vec3::new;
    for s in &SLABS {
        let (x0, x1) = (s.at.0 - s.size.0 * 0.5, s.at.0 + s.size.0 * 0.5);
        let (z0, z1) = (s.at.1 - s.size.1 * 0.5, s.at.1 + s.size.1 * 0.5);
        let bottom = s.top - SLAB;
        let cap = 0.12;
        slab_box(&mut k, BODY, v(x0, bottom, z0), v(x1, s.top - cap, z1), body_m);
        let (m, uv) = match s.finish {
            Top::Concrete => (CONCRETE, concrete_m),
            Top::Plate => (PLATE, plate_m),
        };
        slab_box(&mut k, m, v(x0, s.top - cap, z0), v(x1, s.top, z1), uv);
        // A concrete kerb along the border, then the amber edge line just inside it.
        if s.size.0 > 2.0 {
            let kerb = 0.14;
            for (a, b) in [
                (v(x0, s.top, z0), v(x1, s.top + 0.07, z0 + kerb)),
                (v(x0, s.top, z1 - kerb), v(x1, s.top + 0.07, z1)),
                (v(x0, s.top, z0 + kerb), v(x0 + kerb, s.top + 0.07, z1 - kerb)),
                (v(x1 - kerb, s.top, z0 + kerb), v(x1, s.top + 0.07, z1 - kerb)),
            ] {
                slab_box(&mut k, CONCRETE, a, b, concrete_m);
            }
        }
        let e = 0.22;
        let w = 0.1;
        let y = s.top + 0.012;
        for (a, b) in [
            (v(x0 + e, y - 0.012, z0 + e), v(x1 - e, y, z0 + e + w)),
            (v(x0 + e, y - 0.012, z1 - e - w), v(x1 - e, y, z1 - e)),
            (v(x0 + e, y - 0.012, z0 + e), v(x0 + e + w, y, z1 - e)),
            (v(x1 - e - w, y - 0.012, z0 + e), v(x1 - e, y, z1 - e)),
        ] {
            if (b.x - a.x) > 0.0 && (b.z - a.z) > 0.0 && s.size.0 > 2.0 {
                slab_box(&mut k, AMBER, a, b, Vec2::ONE);
            }
        }
        // Girders: a pair under the long pieces, a column under the squares and islands.
        let depth = 26.0;
        if s.size.1 > s.size.0 * 2.0 {
            let mut z = z0 + 1.0;
            while z < z1 {
                for dx in [-s.size.0 * 0.5 + 0.2, s.size.0 * 0.5 - 0.2] {
                    slab_box(&mut k, GIRDER, v(s.at.0 + dx - 0.12, bottom - 5.0, z - 0.12), v(s.at.0 + dx + 0.12, bottom, z + 0.12), Vec2::ONE);
                }
                slab_box(&mut k, GIRDER, v(x0, bottom - 0.5, z - 0.1), v(x1, bottom - 0.3, z + 0.1), Vec2::ONE);
                z += 4.0;
            }
        } else {
            let c = (s.size.0.min(s.size.1) * 0.12).max(0.18);
            slab_box(&mut k, GIRDER, v(s.at.0 - c, bottom - depth, s.at.1 - c), v(s.at.0 + c, bottom, s.at.1 + c), Vec2::ONE);
        }
    }
    // The start dock's railing: posts every 1.6 m along the back and both sides, an amber top
    // rail and a steel mid rail. (Its collision is added by the scene; the runway side is open.)
    let post = |k: &mut Kit, x: f32, z: f32| slab_box(k, RAIL, v(x - 0.04, 0.0, z - 0.04), v(x + 0.04, 1.05, z + 0.04), Vec2::ONE);
    let mut x = -3.9;
    while x <= 3.95 {
        post(&mut k, x, -4.9);
        x += 1.3;
    }
    for sx in [-3.9, 3.9] {
        let mut z = -4.9;
        while z <= 4.95 {
            post(&mut k, sx, z);
            z += 1.3;
        }
        slab_box(&mut k, AMBER, v(sx - 0.035, 1.0, -4.94), v(sx + 0.035, 1.07, 5.0), Vec2::ONE);
        slab_box(&mut k, RAIL, v(sx - 0.025, 0.55, -4.94), v(sx + 0.025, 0.6, 5.0), Vec2::ONE);
    }
    slab_box(&mut k, AMBER, v(-3.94, 1.0, -4.94), v(3.94, 1.07, -4.86), Vec2::ONE);
    slab_box(&mut k, RAIL, v(-3.94, 0.55, -4.94), v(3.94, 0.6, -4.88), Vec2::ONE);
    k.write(root, lock, "glint_course")
}

/// The sliding platform, centred on its own origin: a rust body under an amber painted top.
pub fn mover(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![
        Material::textured("rust steel", "rusty_metal_02").tinted([0.5, 0.45, 0.42]),
        Material::flat("amber painted plate", [0.85, 0.42, 0.08], 0.42, 0.0),
        Material::flat("black rubber", [0.03, 0.03, 0.03], 0.8, 0.0),
    ]);
    let (sx, sz) = MOVER_SIZE;
    let v = Vec3::new;
    slab_box(&mut k, 0, v(-sx * 0.5, -SLAB * 0.5, -sz * 0.5), v(sx * 0.5, SLAB * 0.5 - 0.1, sz * 0.5), meters(lock, "rusty_metal_02"));
    slab_box(&mut k, 1, v(-sx * 0.5, SLAB * 0.5 - 0.1, -sz * 0.5), v(sx * 0.5, SLAB * 0.5, sz * 0.5), Vec2::ONE);
    for dx in [-sx * 0.5 + 0.15, sx * 0.5 - 0.15] {
        slab_box(&mut k, 2, v(dx - 0.08, SLAB * 0.5 - 0.1, -sz * 0.5), v(dx + 0.08, SLAB * 0.5 + 0.01, sz * 0.5), Vec2::ONE);
    }
    k.write(root, lock, "glint_mover")
}

/// The spinner bar, centred on its pivot: alternating amber and black segments, a steel hub.
pub fn bar(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![
        Material::flat("amber paint", [0.85, 0.42, 0.08], 0.45, 0.0),
        Material::flat("black paint", [0.03, 0.03, 0.035], 0.5, 0.0),
        Material::flat("brushed steel", [0.7, 0.72, 0.75], 0.35, 1.0),
    ]);
    let (len, t) = BAR;
    let seg = len / 6.0;
    for i in 0..6 {
        let x = -len * 0.5 + seg * (i as f32 + 0.5);
        k.cuboid(i % 2, Vec3::new(seg, t, t), Mat4::from_translation(Vec3::new(x, 0.0, 0.0)), Vec2::ONE);
    }
    k.cylinder(2, 0.28, 0.2, 24, Mat4::from_translation(Vec3::new(0.0, -t * 0.5 - 0.02, 0.0)), Vec2::ONE);
    k.cylinder(2, 0.12, 1.4, 16, Mat4::from_translation(Vec3::new(0.0, -1.45, 0.0)), Vec2::ONE);
    k.write(root, lock, "glint_bar")
}

/// A checkpoint pad plate with a lit amber core (emission scaled by the entity's renderer).
pub fn pad(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![
        Material::flat("black frame", [0.04, 0.04, 0.045], 0.5, 1.0),
        Material::emitter("amber light", [1.0, 0.62, 0.22]),
    ]);
    let v = Vec3::new;
    slab_box(&mut k, 0, v(-2.1, 0.0, -2.1), v(2.1, 0.05, 2.1), Vec2::ONE);
    slab_box(&mut k, 1, v(-1.8, 0.05, -1.8), v(1.8, 0.07, 1.8), Vec2::ONE);
    // A flag post at the back left: a pole and a lit pennant.
    k.cylinder(0, 0.05, 2.4, 12, Mat4::from_translation(v(-1.9, 0.05, 1.9)), Vec2::ONE);
    slab_box(&mut k, 1, v(-1.9, 1.7, 1.86), v(-1.9 + 0.8, 2.3, 1.9 + 0.0), Vec2::ONE);
    // A lit pillar over the pad, so a checkpoint reads from the far end of the course.
    k.cylinder(1, 0.09, 16.0, 12, Mat4::from_translation(v(0.0, 0.07, 0.0)), Vec2::ONE);
    k.write(root, lock, "glint_pad")
}

/// The goal: a plate and a ring of lit pylons.
pub fn goal(root: &Path, lock: &Lock) -> Result<String> {
    let mut k = Kit::new(vec![
        Material::flat("black frame", [0.04, 0.04, 0.045], 0.5, 1.0),
        Material::emitter("cyan light", [0.25, 0.85, 1.0]),
        Material::glass("goal glass", [0.7, 0.95, 1.0], 0.25),
    ]);
    let v = Vec3::new;
    slab_box(&mut k, 0, v(-3.0, 0.0, -3.0), v(3.0, 0.06, 3.0), Vec2::ONE);
    slab_box(&mut k, 1, v(-2.6, 0.06, -2.6), v(2.6, 0.08, 2.6), Vec2::ONE);
    for i in 0..6 {
        let a = i as f32 / 6.0 * std::f32::consts::TAU;
        let p = v(a.cos() * 2.7, 0.0, a.sin() * 2.7);
        k.cylinder(0, 0.14, 3.6, 12, Mat4::from_translation(p), Vec2::ONE);
        k.sphere(1, p + v(0.0, 3.7, 0.0), 0.2, 16, Vec2::ONE);
    }
    // The beacon: a tall lit pillar, visible from the start.
    k.cylinder(1, 0.24, 48.0, 16, Mat4::from_translation(v(0.0, 0.08, 0.0)), Vec2::ONE);
    // A glass cylinder of light over the plate.
    k.cylinder(2, 2.4, 3.4, 48, Mat4::from_translation(v(0.0, 0.08, 0.0)), Vec2::ONE);
    k.write(root, lock, "glint_goal")
}

/// A horizontal cylinder from `start` along +X (`along == 0`) or +Z (`along == 1`).
fn pipe(kit: &mut Kit, material: usize, radius: f32, start: Vec3, along: u32, length: f32) {
    let rotation = if along == 0 {
        Mat4::from_rotation_z(-FRAC_PI_2)
    } else {
        Mat4::from_rotation_x(FRAC_PI_2)
    };
    kit.cylinder(material, radius, length, 20, Mat4::from_translation(start) * rotation, Vec2::ONE);
}

/// The foundry yard far below the course: textured ground, route markings, slag channels,
/// furnaces, chimneys and tanks with banding, gantry frames, pipe racks, plate stacks, sheds
/// and a glazed control tower. Pure scenery, no collision.
pub fn yard(root: &Path, lock: &Lock) -> Result<String> {
    const GROUND: usize = 0;
    const BRICK: usize = 1;
    const TANK: usize = 2;
    const SLAG: usize = 3;
    const SHED: usize = 4;
    const GLASS: usize = 5;
    const WINDOW: usize = 6;
    const PIPE: usize = 7;
    const AMBER: usize = 8;
    const MARK: usize = 9;
    const STEEL: usize = 10;
    const PLATES: usize = 11;
    let mut k = Kit::new(vec![
        Material::textured("yard concrete", "concrete_floor_02").tinted([0.075, 0.08, 0.095]),
        Material::flat("sooty brick", [0.1, 0.09, 0.09], 0.92, 0.0),
        Material::flat("tank steel", [0.16, 0.18, 0.22], 0.6, 0.45),
        Material::emitter("slag", [1.0, 0.38, 0.08]),
        Material::textured("shed cladding", "metal_plate").tinted([0.1, 0.11, 0.14]),
        Material::glass("control glass", [0.6, 0.8, 0.9], 0.3),
        Material::emitter("window light", [1.0, 0.82, 0.55]),
        Material::flat("pipe steel", [0.34, 0.35, 0.37], 0.5, 0.8),
        Material::flat("amber paint", [0.85, 0.42, 0.08], 0.5, 0.0),
        Material::flat("road marking", [0.7, 0.66, 0.55], 0.85, 0.0),
        Material::flat("black girder steel", [0.06, 0.065, 0.07], 0.55, 1.0),
        Material::textured("stacked plate", "metal_plate").tinted([0.2, 0.22, 0.26]),
    ]);
    let v = Vec3::new;
    let floor = YARD_FLOOR;
    let ground_m = meters(lock, "concrete_floor_02") * 2.0;
    let plate_m = meters(lock, "metal_plate") * 2.0;
    slab_box(&mut k, GROUND, v(-220.0, floor - 1.0, -120.0), v(220.0, floor, 260.0), ground_m);
    // Slag channels: long glowing strips in the ground.
    for (x, z0, z1) in [(-22.0, -40.0, 140.0), (26.0, -40.0, 140.0), (-60.0, 10.0, 120.0)] {
        slab_box(&mut k, SLAG, v(x - 1.4, floor + 0.01, z0), v(x + 1.4, floor + 0.06, z1), Vec2::ONE);
    }
    for z in [20.0, 60.0, 100.0] {
        slab_box(&mut k, SLAG, v(-80.0, floor + 0.01, z - 1.2), v(80.0, floor + 0.05, z + 1.2), Vec2::ONE);
    }
    // Floor markings that follow the route: a dashed centre line under the catwalk.
    let mut z = -30.0;
    while z < 124.0 {
        slab_box(&mut k, MARK, v(-0.18, floor + 0.01, z), v(0.18, floor + 0.03, z + 2.4), Vec2::ONE);
        z += 5.0;
    }
    // Furnaces: a dark block, a framed glowing mouth, a hood, a stack, amber hazard borders.
    for (x, z) in FURNACES {
        slab_box(&mut k, SHED, v(x - 9.0, floor, z - 6.0), v(x + 9.0, floor + 11.0, z + 6.0), plate_m);
        slab_box(&mut k, SLAG, v(x - 5.0, floor + 1.2, z - 6.05), v(x + 5.0, floor + 6.0, z - 6.0), Vec2::ONE);
        for (a, b) in [
            (v(x - 5.6, floor + 0.9, z - 6.3), v(x - 5.0, floor + 6.6, z - 5.9)),
            (v(x + 5.0, floor + 0.9, z - 6.3), v(x + 5.6, floor + 6.6, z - 5.9)),
            (v(x - 5.6, floor + 6.0, z - 6.3), v(x + 5.6, floor + 6.6, z - 5.9)),
        ] {
            slab_box(&mut k, STEEL, a, b, Vec2::ONE);
        }
        slab_box(&mut k, SHED, v(x - 6.0, floor + 6.6, z - 7.4), v(x + 6.0, floor + 7.6, z - 5.8), plate_m);
        k.cylinder(BRICK, 1.6, 16.0, 24, Mat4::from_translation(v(x + 5.0, floor + 11.0, z + 2.0)), Vec2::ONE);
        for band in [0.0, 5.0, 10.0] {
            k.cylinder(PIPE, 1.7, 0.4, 24, Mat4::from_translation(v(x + 5.0, floor + 11.5 + band, z + 2.0)), Vec2::ONE);
        }
        let (hx0, hx1, hz0, hz1) = (x - 10.0, x + 10.0, z - 16.0, z - 7.0);
        for (a, b) in [
            (v(hx0, floor + 0.01, hz0), v(hx1, floor + 0.04, hz0 + 0.25)),
            (v(hx0, floor + 0.01, hz1 - 0.25), v(hx1, floor + 0.04, hz1)),
            (v(hx0, floor + 0.01, hz0), v(hx0 + 0.25, floor + 0.04, hz1)),
            (v(hx1 - 0.25, floor + 0.01, hz0), v(hx1, floor + 0.04, hz1)),
        ] {
            slab_box(&mut k, AMBER, a, b, Vec2::ONE);
        }
    }
    // Chimneys: sooty brick with steel bands, a platform ring and a lit mouth.
    for (x, z, r, h) in CHIMNEYS {
        k.cylinder(BRICK, r, h, 32, Mat4::from_translation(v(x, floor, z)), Vec2::ONE);
        for f in [0.3, 0.6, 0.88] {
            k.cylinder(PIPE, r + 0.07, 0.4, 32, Mat4::from_translation(v(x, floor + h * f, z)), Vec2::ONE);
        }
        k.cylinder(STEEL, r + 0.9, 0.18, 32, Mat4::from_translation(v(x, floor + h - 1.4, z)), Vec2::ONE);
        k.cylinder(SLAG, r * 0.8, 0.3, 24, Mat4::from_translation(v(x, floor + h - 0.2, z)), Vec2::ONE);
        slab_box(&mut k, SLAG, v(x - 2.2, floor, z - r - 0.1), v(x + 2.2, floor + 3.6, z - r), Vec2::ONE);
    }
    // Tanks: banded steel, a cap, a caged ladder and a pipe stub.
    for (x, z, r, h) in [(-34.0, 60.0, 6.0, 12.0), (34.0, 20.0, 5.0, 10.0), (38.0, 96.0, 7.0, 14.0), (-30.0, 8.0, 4.5, 9.0)] {
        k.cylinder(TANK, r, h, 40, Mat4::from_translation(v(x, floor, z)), Vec2::ONE);
        for f in [0.12, 0.5, 0.88] {
            k.cylinder(STEEL, r + 0.1, 0.45, 40, Mat4::from_translation(v(x, floor + h * f, z)), Vec2::ONE);
        }
        k.cylinder(STEEL, r * 1.03, 0.3, 40, Mat4::from_translation(v(x, floor + h, z)), Vec2::ONE);
        let lx = x - r - 0.15;
        for dz in [-0.3, 0.3] {
            slab_box(&mut k, PIPE, v(lx - 0.03, floor, z + dz - 0.03), v(lx + 0.03, floor + h + 0.8, z + dz + 0.03), Vec2::ONE);
        }
        let mut y = 0.5;
        while y < h {
            slab_box(&mut k, PIPE, v(lx - 0.03, floor + y, z - 0.3), v(lx + 0.03, floor + y + 0.05, z + 0.3), Vec2::ONE);
            y += 0.6;
        }
        pipe(&mut k, PIPE, 0.28, v(x + r - 0.2, floor + 1.0, z), 1, 9.0);
    }
    // Gantry frames beside and over the route: tall uprights, a top beam above the deck (its
    // shadow rakes across the catwalk), diagonal braces, a hanging lamp.
    for z in [14.0, 46.0, 78.0] {
        let top = floor + 31.0;
        for x in [-8.0, 8.0] {
            slab_box(&mut k, STEEL, v(x - 0.4, floor, z - 0.4), v(x + 0.4, top + 0.6, z + 0.4), Vec2::ONE);
            let flip = if x < 0.0 { 1.0 } else { -1.0 };
            for dz in [-1.0, 1.0] {
                k.cuboid(STEEL, v(0.25, 7.6, 0.25), Mat4::from_translation(v(x + flip * 2.3, top - 3.5, z + dz * 0.1)) * Mat4::from_rotation_z(flip * 0.52), Vec2::ONE);
            }
        }
        slab_box(&mut k, STEEL, v(-8.4, top, z - 0.5), v(8.4, top + 0.9, z + 0.5), Vec2::ONE);
        slab_box(&mut k, AMBER, v(-1.5, top - 1.4, z - 0.9), v(1.5, top, z + 0.9), Vec2::ONE);
        slab_box(&mut k, WINDOW, v(-0.5, top - 2.2, z - 0.5), v(0.5, top - 1.4, z + 0.5), Vec2::ONE);
    }
    // Pipe racks: two long runs along the route on frames, one run across the yard.
    for (x, h) in [(-18.0, 6.0), (18.0, 4.5)] {
        pipe(&mut k, PIPE, 0.5, v(x - 0.6, floor + h, -30.0), 1, 160.0);
        pipe(&mut k, PIPE, 0.34, v(x + 0.7, floor + h, -30.0), 1, 160.0);
        let mut zf = -28.0;
        while zf < 128.0 {
            slab_box(&mut k, STEEL, v(x - 1.2, floor, zf - 0.15), v(x - 0.9, floor + h - 0.5, zf + 0.15), Vec2::ONE);
            slab_box(&mut k, STEEL, v(x + 0.9, floor, zf - 0.15), v(x + 1.2, floor + h - 0.5, zf + 0.15), Vec2::ONE);
            slab_box(&mut k, STEEL, v(x - 1.2, floor + h - 0.5, zf - 0.15), v(x + 1.2, floor + h - 0.3, zf + 0.15), Vec2::ONE);
            // A flange at each frame.
            k.cylinder(PIPE, 0.62, 0.25, 20, Mat4::from_translation(v(x - 0.6, floor + h, zf)) * Mat4::from_rotation_x(FRAC_PI_2), Vec2::ONE);
            zf += 12.0;
        }
    }
    pipe(&mut k, PIPE, 0.42, v(-60.0, floor + 4.0, -26.0), 0, 120.0);
    // Stacked plates and a few drums by the route.
    for (x, z) in [(10.0, -14.0), (-12.0, -8.0), (16.0, 70.0), (-14.0, 90.0), (-8.0, 40.0)] {
        for i in 0..6 {
            let off = ((i * 37 + (x as i32) * 3) % 5) as f32 * 0.12 - 0.25;
            slab_box(&mut k, PLATES, v(x - 2.0 + off, floor + 0.2 * i as f32, z - 1.0), v(x + 2.0 + off, floor + 0.2 * (i as f32 + 1.0), z + 1.0), plate_m);
        }
    }
    // Sheds with ribs and a lit window band facing the course; a glazed control tower.
    for (x, z, sx, sz, h) in [(-70.0, 40.0, 24.0, 40.0, 14.0), (66.0, 78.0, 28.0, 30.0, 12.0), (-4.0, 140.0, 60.0, 16.0, 16.0), (80.0, 10.0, 20.0, 26.0, 10.0)] {
        slab_box(&mut k, SHED, v(x - sx * 0.5, floor, z - sz * 0.5), v(x + sx * 0.5, floor + h, z + sz * 0.5), plate_m);
        slab_box(&mut k, WINDOW, v(x - sx * 0.5 + 1.0, floor + h * 0.55, z - sz * 0.5 - 0.05), v(x + sx * 0.5 - 1.0, floor + h * 0.55 + 0.9, z - sz * 0.5), Vec2::ONE);
        let mut rx = x - sx * 0.5 + 1.5;
        while rx < x + sx * 0.5 - 1.0 {
            slab_box(&mut k, STEEL, v(rx - 0.12, floor, z - sz * 0.5 - 0.25), v(rx + 0.12, floor + h, z - sz * 0.5), Vec2::ONE);
            rx += 3.0;
        }
        slab_box(&mut k, STEEL, v(x - sx * 0.5 - 0.3, floor + h, z - sz * 0.5 - 0.3), v(x + sx * 0.5 + 0.3, floor + h + 0.5, z + sz * 0.5 + 0.3), Vec2::ONE);
    }
    let (tx, tz) = (-48.0, 2.0);
    slab_box(&mut k, SHED, v(tx - 1.0, floor, tz - 1.0), v(tx + 1.0, floor + 14.0, tz + 1.0), plate_m);
    slab_box(&mut k, WINDOW, v(tx - 4.0, floor + 15.0, tz - 4.0), v(tx + 4.0, floor + 15.4, tz + 4.0), Vec2::ONE);
    slab_box(&mut k, GLASS, v(tx - 4.5, floor + 15.4, tz - 4.5), v(tx + 4.5, floor + 18.4, tz + 4.5), Vec2::ONE);
    slab_box(&mut k, STEEL, v(tx - 5.0, floor + 18.4, tz - 5.0), v(tx + 5.0, floor + 18.9, tz + 5.0), Vec2::ONE);
    k.write(root, lock, "glint_yard")
}
