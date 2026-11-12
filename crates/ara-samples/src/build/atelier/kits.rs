//! Atelier geometry: the room shell, the courtyard and the furniture, as glTF kits of
//! plain metallic-roughness materials (base colour, one texture set, alpha blend for
//! glass). Every piece is made for the scene; nothing depends on an Ara-only feature.
//! Room frame: Y up, right-handed, metres. The room's floor is `y = 0` and spans
//! `x ∈ [-4, 4]`, `z ∈ [-9, 0]`; the courtyard lies on the `+Z` side of the front wall.
use crate::{
    kit::{Kit, Material, meters},
    lock::Lock,
};
use anyhow::Result;
use glam::{Mat4, Vec2, Vec3};
use std::path::Path;

/// Ceiling height of the room (underside), metres.
pub const CEILING: f32 = 4.0;

/// Adds an axis-aligned box from `min` to `max` in material `material`.
fn slab(kit: &mut Kit, material: usize, min: Vec3, max: Vec3, meters: Vec2) {
    kit.cuboid(
        material,
        max - min,
        Mat4::from_translation((min + max) * 0.5),
        meters,
    );
}

/// A box given by its centre and size.
fn block(kit: &mut Kit, material: usize, center: Vec3, size: Vec3) {
    kit.cuboid(material, size, Mat4::from_translation(center), Vec2::ONE);
}

/// A box turned about Y.
fn turned(kit: &mut Kit, material: usize, center: Vec3, size: Vec3, yaw: f32) {
    kit.cuboid(
        material,
        size,
        Mat4::from_translation(center) * Mat4::from_rotation_y(yaw.to_radians()),
        Vec2::ONE,
    );
}

/// A vertical cylinder standing on `base`.
fn post(kit: &mut Kit, material: usize, base: Vec3, radius: f32, height: f32) {
    kit.cylinder(
        material,
        radius,
        height,
        24,
        Mat4::from_translation(base),
        Vec2::ONE,
    );
}

/// The room and courtyard shell: floors, walls with their openings, the ceiling with
/// its sun slot, the east window frame and its glass.
pub fn shell(root: &Path, lock: &Lock) -> Result<String> {
    const FLOOR: usize = 0;
    const PLASTER: usize = 1;
    const STEEL: usize = 2;
    const GLASS: usize = 3;
    const PAVING: usize = 4;
    const TERRACOTTA: usize = 5;
    const GROUND: usize = 6;
    let mut k = Kit::new(vec![
        Material::textured("oak floor", "wood_table_001").tinted([0.92, 0.84, 0.74]),
        Material::flat("white plaster", [0.82, 0.8, 0.76], 0.9, 0.0),
        Material::flat("black steel", [0.05, 0.05, 0.055], 0.42, 1.0),
        Material::glass("window glass", [0.88, 0.96, 1.0], 0.14),
        Material::textured("courtyard stone", "concrete_floor_02").tinted([0.78, 0.74, 0.68]),
        Material::flat("terracotta plaster", [0.55, 0.22, 0.12], 0.88, 0.0),
        Material::flat("packed earth", [0.33, 0.28, 0.22], 0.95, 0.0),
    ]);
    let v = Vec3::new;
    let floor_m = meters(lock, "wood_table_001");
    let stone_m = meters(lock, "concrete_floor_02");
    // Floors: the room, the courtyard paving and the ground around it.
    slab(&mut k, FLOOR, v(-4.4, -0.3, -9.4), v(4.4, 0.0, 0.4), floor_m);
    slab(&mut k, PAVING, v(-7.0, -0.3, 0.4), v(7.0, 0.0, 9.0), stone_m);
    slab(&mut k, GROUND, v(-60.0, -0.6, -60.0), v(60.0, -0.31, 80.0), Vec2::ONE);
    // Ceiling and roof with the sun slot (x -1.2..1.2, z -1.4..-0.6).
    for (a, b) in [
        (v(-4.4, CEILING, -0.6), v(4.4, 4.4, 0.4)),
        (v(-4.4, CEILING, -9.4), v(4.4, 4.4, -1.4)),
        (v(-4.4, CEILING, -1.4), v(-1.2, 4.4, -0.6)),
        (v(1.2, CEILING, -1.4), v(4.4, 4.4, -0.6)),
    ] {
        slab(&mut k, PLASTER, a, b, Vec2::ONE);
    }
    // Front wall (z 0..0.4) with a 3.2 x 3.0 m doorway.
    for (a, b) in [
        (v(-4.4, 0.0, 0.0), v(-1.6, CEILING, 0.4)),
        (v(1.6, 0.0, 0.0), v(4.4, CEILING, 0.4)),
        (v(-1.6, 3.0, 0.0), v(1.6, CEILING, 0.4)),
    ] {
        slab(&mut k, PLASTER, a, b, Vec2::ONE);
    }
    // Back wall and west wall (terracotta skin on the inside).
    slab(&mut k, PLASTER, v(-4.4, 0.0, -9.4), v(4.4, CEILING, -9.0), Vec2::ONE);
    slab(&mut k, PLASTER, v(-4.4, 0.0, -9.0), v(-4.0, CEILING, 0.0), Vec2::ONE);
    slab(&mut k, TERRACOTTA, v(-4.0, 0.0, -9.0), v(-3.97, CEILING, 0.0), Vec2::ONE);
    // East wall with a 4.2 x 3.0 m window (z -6.6..-2.4, y 0.5..3.5).
    for (a, b) in [
        (v(4.0, 0.0, -9.0), v(4.4, CEILING, -6.6)),
        (v(4.0, 0.0, -2.4), v(4.4, CEILING, 0.0)),
        (v(4.0, 0.0, -6.6), v(4.4, 0.5, -2.4)),
        (v(4.0, 3.5, -6.6), v(4.4, CEILING, -2.4)),
    ] {
        slab(&mut k, PLASTER, a, b, Vec2::ONE);
    }
    // Window frame and mullions, and the glass pane in the middle of the wall.
    let (z0, z1, y0, y1) = (-6.6, -2.4, 0.5, 3.5);
    let (xa, xb) = (4.14, 4.26);
    slab(&mut k, STEEL, v(xa, y0, z0), v(xb, y0 + 0.06, z1), Vec2::ONE);
    slab(&mut k, STEEL, v(xa, y1 - 0.06, z0), v(xb, y1, z1), Vec2::ONE);
    for z in [z0, z1 - 0.06] {
        slab(&mut k, STEEL, v(xa, y0, z), v(xb, y1, z + 0.06), Vec2::ONE);
    }
    slab(&mut k, STEEL, v(xa, 2.0, z0), v(xb, 2.05, z1), Vec2::ONE);
    for z in [-5.2, -3.8] {
        slab(&mut k, STEEL, v(xa, y0, z - 0.025), v(xb, y1, z + 0.025), Vec2::ONE);
    }
    slab(&mut k, GLASS, v(4.2, y0 + 0.06, z0 + 0.06), v(4.212, y1 - 0.06, z1 - 0.06), Vec2::ONE);
    // Courtyard walls, 1.3 m high.
    for (a, b) in [
        (v(6.6, 0.0, 0.4), v(7.0, 1.3, 9.0)),
        (v(-7.0, 0.0, 0.4), v(-6.6, 1.3, 9.0)),
        (v(-7.0, 0.0, 8.6), v(7.0, 1.3, 9.0)),
    ] {
        slab(&mut k, PLASTER, a, b, Vec2::ONE);
    }
    k.write(root, lock, "atelier_shell")
}

/// The furniture and objects of the room: sculpture on its plinth, the table with
/// glass and brass, the armchair, bookshelf, lamps, rug and the painting.
pub fn furniture(root: &Path, lock: &Lock) -> Result<String> {
    const OAK: usize = 0;
    const LACQUER: usize = 1;
    const LEATHER: usize = 2;
    const STEEL: usize = 3;
    const BRASS: usize = 4;
    const CHROME: usize = 5;
    const MARBLE: usize = 6;
    const GLASS: usize = 7;
    const BULB: usize = 8;
    const RUG: usize = 9;
    const BOOKS: usize = 10;
    let mut k = Kit::new(vec![
        Material::flat("dark oak", [0.2, 0.12, 0.07], 0.5, 0.0),
        Material::flat("red lacquer", [0.5, 0.03, 0.025], 0.1, 0.0),
        Material::textured("leather", "fabric_leather_02").tinted([0.55, 0.36, 0.24]),
        Material::flat("brushed steel", [0.78, 0.79, 0.8], 0.34, 1.0),
        Material::flat("brushed brass", [0.93, 0.72, 0.36], 0.28, 1.0),
        Material::flat("chrome", [0.95, 0.95, 0.95], 0.03, 1.0),
        Material::textured("marble", "marble_01"),
        Material::glass("clear glass", [0.9, 0.97, 1.0], 0.16),
        Material::emitter("warm bulb", [1.0, 0.8, 0.55]),
        Material::flat("indigo wool rug", [0.09, 0.14, 0.26], 1.0, 0.0),
        Material::flat("book teal", [0.05, 0.3, 0.32], 0.7, 0.0),
        Material::flat("book ochre", [0.62, 0.42, 0.08], 0.7, 0.0),
        Material::flat("book cream", [0.78, 0.74, 0.62], 0.75, 0.0),
        Material::flat("book crimson", [0.45, 0.06, 0.08], 0.7, 0.0),
        Material::flat("canvas white", [0.85, 0.84, 0.8], 0.9, 0.0),
        Material::flat("canvas blue", [0.1, 0.22, 0.55], 0.85, 0.0),
        Material::flat("canvas yellow", [0.85, 0.62, 0.08], 0.85, 0.0),
    ]);
    let v = Vec3::new;
    // Sculpture: marble plinth, brass disc and a chrome sphere in the front sun patch.
    let s = v(-1.6, 0.0, -2.4);
    block(&mut k, MARBLE, s + v(0.0, 0.45, 0.0), v(0.7, 0.9, 0.7));
    post(&mut k, BRASS, s + v(0.0, 0.9, 0.0), 0.3, 0.04);
    k.sphere(CHROME, s + v(0.0, 1.27, 0.0), 0.33, 48, Vec2::ONE);
    // Table (red lacquer top on steel legs), rug beneath, objects on it.
    let t = v(0.2, 0.0, -6.0);
    block(&mut k, LACQUER, t + v(0.0, 0.74, 0.0), v(1.9, 0.05, 0.95));
    for (dx, dz) in [(-0.9, -0.42), (0.9, -0.42), (-0.9, 0.42), (0.9, 0.42)] {
        post(&mut k, STEEL, t + v(dx, 0.0, dz), 0.025, 0.74);
    }
    block(&mut k, RUG, t + v(0.0, 0.006, 0.0), v(3.4, 0.012, 2.4));
    for (dx, dz, h, yaw) in [(-0.45, 0.1, 0.22, 10.0), (-0.15, -0.12, 0.3, 40.0), (0.05, 0.14, 0.16, 25.0)] {
        turned(&mut k, GLASS, t + v(dx, 0.765 + h * 0.5, dz), v(0.16, h, 0.16), yaw);
    }
    post(&mut k, BRASS, t + v(0.55, 0.765, 0.0), 0.11, 0.02);
    k.sphere(BRASS, t + v(0.55, 0.9, 0.0), 0.11, 32, Vec2::ONE);
    // Pendant over the table: cord, brass shade, bulb.
    post(&mut k, STEEL, v(t.x, 2.7, t.z), 0.006, 1.3);
    k.cylinder(BRASS, 0.22, 0.22, 32, Mat4::from_translation(v(t.x, 2.45, t.z)), Vec2::ONE);
    k.sphere(BULB, v(t.x, 2.5, t.z), 0.1, 24, Vec2::ONE);
    // Armchair in leather on dark oak legs.
    let a = v(-2.85, 0.0, -7.1);
    let yaw = 35.0;
    let at = |dx: f32, y: f32, dz: f32| {
        let p = Mat4::from_rotation_y(f32::to_radians(yaw)).transform_point3(v(dx, y, dz));
        a + p
    };
    turned(&mut k, LEATHER, at(0.0, 0.42, 0.0), v(0.9, 0.22, 0.85), yaw);
    turned(&mut k, LEATHER, at(0.0, 0.8, -0.38), v(0.9, 0.55, 0.14), yaw);
    for dx in [-0.5, 0.5] {
        turned(&mut k, LEATHER, at(dx, 0.62, 0.0), v(0.14, 0.34, 0.85), yaw);
    }
    for (dx, dz) in [(-0.38, -0.34), (0.38, -0.34), (-0.38, 0.34), (0.38, 0.34)] {
        let p = at(dx, 0.0, dz);
        post(&mut k, OAK, p, 0.03, 0.3);
    }
    // Floor lamp beside the chair: brass stem, base, emissive shade.
    let l = v(-3.4, 0.0, -5.7);
    post(&mut k, BRASS, l, 0.16, 0.03);
    post(&mut k, BRASS, l, 0.012, 1.55);
    k.sphere(BULB, l + v(0.0, 1.7, 0.0), 0.09, 24, Vec2::ONE);
    // Bookshelf on the back wall: frame, shelves and books.
    let b = v(-2.4, 0.0, -8.75);
    for x in [-1.0, 1.0] {
        block(&mut k, OAK, b + v(x, 1.1, 0.0), v(0.04, 2.2, 0.4));
    }
    for y in [0.05, 0.6, 1.15, 1.7, 2.2] {
        block(&mut k, OAK, b + v(0.0, y, 0.0), v(2.04, 0.04, 0.4));
    }
    let mut seed = 7u32;
    let mut rand = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    for (row, y) in [0.07, 0.62, 1.17, 1.72].into_iter().enumerate() {
        let mut x = -0.95;
        while x < 0.85 - (row as f32) * 0.2 {
            let w = 0.03 + rand() * 0.035;
            let h = 0.28 + rand() * 0.2;
            let material = BOOKS + (rand() * 4.0) as usize;
            block(&mut k, material.min(BOOKS + 3), b + v(x + w * 0.5, y + h * 0.5, 0.0), v(w, h, 0.26));
            x += w + 0.004;
        }
    }
    // A painting on the back wall: canvas blocks in a steel frame.
    let p = v(1.6, 1.9, -8.97);
    block(&mut k, STEEL, p, v(1.3, 1.0, 0.03));
    block(&mut k, 14, p + v(0.0, 0.0, 0.02), v(1.22, 0.92, 0.02));
    block(&mut k, 15, p + v(-0.2, -0.05, 0.035), v(0.6, 0.62, 0.01));
    block(&mut k, 16, p + v(0.3, 0.12, 0.035), v(0.4, 0.45, 0.01));
    block(&mut k, LACQUER, p + v(0.25, -0.25, 0.04), v(0.3, 0.2, 0.01));
    k.write(root, lock, "atelier_furniture")
}

/// Courtyard dressing: a bench, stone planters and the wall lantern.
pub fn courtyard(root: &Path, lock: &Lock) -> Result<String> {
    const OAK: usize = 0;
    const STONE: usize = 1;
    const STEEL: usize = 2;
    const LANTERN: usize = 3;
    const SOIL: usize = 4;
    let mut k = Kit::new(vec![
        Material::flat("teak", [0.4, 0.25, 0.12], 0.6, 0.0),
        Material::flat("limestone", [0.72, 0.68, 0.6], 0.85, 0.0),
        Material::flat("black steel", [0.05, 0.05, 0.055], 0.42, 1.0),
        Material::emitter("lantern glass", [1.0, 0.78, 0.5]),
        Material::flat("dark soil", [0.1, 0.07, 0.05], 1.0, 0.0),
    ]);
    let v = Vec3::new;
    // Bench: slatted seat on steel legs, against the west wall.
    let bench = v(-5.6, 0.0, 4.0);
    for i in 0..5 {
        block(&mut k, OAK, bench + v(0.0, 0.46, -0.28 + i as f32 * 0.14), v(1.6, 0.04, 0.11));
    }
    for x in [-0.7, 0.7] {
        block(&mut k, STEEL, bench + v(x, 0.22, 0.0), v(0.04, 0.44, 0.7));
    }
    // Planters along the east wall and by the doorway.
    for (x, z, w, d) in [(5.7, 2.5, 0.8, 2.0), (5.7, 6.0, 0.8, 2.4), (-2.6, 1.3, 1.4, 0.7), (2.6, 1.3, 1.4, 0.7)] {
        block(&mut k, STONE, v(x, 0.3, z), v(w, 0.6, d));
        block(&mut k, SOIL, v(x, 0.605, z), v(w - 0.08, 0.02, d - 0.08));
    }
    // Wall lantern right of the doorway: bracket, frame and glowing glass.
    let l = v(2.2, 2.4, 0.5);
    block(&mut k, STEEL, l + v(0.0, 0.0, 0.08), v(0.04, 0.5, 0.04));
    block(&mut k, STEEL, l + v(0.0, 0.0, 0.18), v(0.2, 0.34, 0.2));
    block(&mut k, LANTERN, l + v(0.0, 0.0, 0.18), v(0.17, 0.3, 0.17));
    k.write(root, lock, "atelier_courtyard")
}
