//! Atelier geometry: the room shell, the courtyard and the furniture, as glTF kits of
//! plain metallic-roughness materials (base colour, one texture set outdoors, alpha blend
//! for glass). Every piece is made for the scene; nothing depends on an Ara-only feature.
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
    const SAND: usize = 7;
    const JOINT: usize = 8;
    const BOUNDARY: usize = 9;
    const MAST: usize = 10;
    let mut k = Kit::new(vec![
        // Mid-grey polished concrete in large tiles, darker than the walls so the sun patches
        // keep their shape instead of clipping; the joints are thin dark strips.
        Material::flat("polished concrete floor", [0.18, 0.176, 0.17], 0.7, 0.0),
        Material::flat("white plaster", [0.85, 0.84, 0.8], 0.9, 0.0),
        Material::flat("black steel", [0.05, 0.05, 0.055], 0.42, 1.0),
        Material::glass("window glass", [0.88, 0.96, 1.0], 0.14),
        Material::textured("courtyard stone", "concrete_floor_02").tinted([0.34, 0.33, 0.31]),
        Material::flat("terracotta plaster", [0.55, 0.22, 0.12], 0.88, 0.0),
        Material::flat("dark gravel", [0.2, 0.19, 0.18], 0.95, 0.0),
        Material::flat("charcoal render", [0.065, 0.062, 0.06], 0.9, 0.0),
        Material::flat("floor joint", [0.05, 0.05, 0.05], 0.8, 0.0),
        // Very dark for the same reason as the render: the exposure is the room's, about
        // five stops under the sun out here, so a paler wall clips and a shadow on it
        // cannot be read. A deep rust, the planters' colour at half its value.
        Material::flat("boundary wall", [0.06, 0.025, 0.014], 0.9, 0.0),
        Material::flat("mast paint", [0.012, 0.012, 0.013], 0.6, 0.0),
    ]);
    let v = Vec3::new;
    let stone_m = meters(lock, "concrete_floor_02");
    // Floors: the room, the courtyard paving and the ground around it.
    slab(
        &mut k,
        FLOOR,
        v(-4.4, -0.3, -9.4),
        v(4.4, 0.0, 0.4),
        Vec2::ONE,
    );
    // Joints of 2 m tiles, a hair proud of the floor.
    for i in 1..4 {
        let x = -4.0 + i as f32 * 2.0;
        slab(
            &mut k,
            JOINT,
            v(x - 0.003, 0.0, -9.0),
            v(x + 0.003, 0.0015, 0.0),
            Vec2::ONE,
        );
    }
    for i in 1..5 {
        let z = -9.0 + i as f32 * 1.8;
        slab(
            &mut k,
            JOINT,
            v(-4.0, 0.0, z - 0.003),
            v(4.0, 0.0015, z + 0.003),
            Vec2::ONE,
        );
    }
    slab(
        &mut k,
        PAVING,
        v(-7.0, -0.3, 0.4),
        v(7.0, 0.0, 9.0),
        stone_m,
    );
    slab(
        &mut k,
        GROUND,
        v(-60.0, -0.6, -60.0),
        v(60.0, -0.31, 80.0),
        Vec2::ONE,
    );
    // Ceiling and roof with the sun slot (x -1.6..1.6, z -2.6..-1.6).
    for (a, b) in [
        (v(-4.4, CEILING, -1.6), v(4.4, 4.4, 0.4)),
        (v(-4.4, CEILING, -9.4), v(4.4, 4.4, -2.6)),
        (v(-4.4, CEILING, -2.6), v(-1.6, 4.4, -1.6)),
        (v(1.6, CEILING, -2.6), v(4.4, 4.4, -1.6)),
    ] {
        slab(&mut k, PLASTER, a, b, Vec2::ONE);
    }
    // Front wall (z 0..0.4) with a 4.4 x 3.4 m doorway.
    for (a, b) in [
        (v(-4.4, 0.0, 0.0), v(-2.2, CEILING, 0.4)),
        (v(2.2, 0.0, 0.0), v(4.4, CEILING, 0.4)),
        (v(-2.2, 3.4, 0.0), v(2.2, CEILING, 0.4)),
    ] {
        slab(&mut k, PLASTER, a, b, Vec2::ONE);
    }
    // Back wall and west wall. The room is white; terracotta is one painted panel on the
    // west wall, behind the sculpture, where the sun from the roof slot lands on it.
    slab(
        &mut k,
        PLASTER,
        v(-4.4, 0.0, -9.4),
        v(4.4, CEILING, -9.0),
        Vec2::ONE,
    );
    slab(
        &mut k,
        PLASTER,
        v(-4.4, 0.0, -9.0),
        v(-4.0, CEILING, 0.0),
        Vec2::ONE,
    );
    slab(
        &mut k,
        TERRACOTTA,
        v(-4.0, 0.0, -3.8),
        v(-3.97, CEILING, -1.0),
        Vec2::ONE,
    );
    // East wall with a 5.6 x 3.2 m window (z -7.4..-1.8, y 0.4..3.6).
    for (a, b) in [
        (v(4.0, 0.0, -9.0), v(4.4, CEILING, -7.4)),
        (v(4.0, 0.0, -1.8), v(4.4, CEILING, 0.0)),
        (v(4.0, 0.0, -7.4), v(4.4, 0.4, -1.8)),
        (v(4.0, 3.6, -7.4), v(4.4, CEILING, -1.8)),
    ] {
        slab(&mut k, PLASTER, a, b, Vec2::ONE);
    }
    // Window frame and mullions, and the glass pane in the middle of the wall.
    let (z0, z1, y0, y1) = (-7.4, -1.8, 0.4, 3.6);
    let (xa, xb) = (4.14, 4.26);
    slab(
        &mut k,
        STEEL,
        v(xa, y0, z0),
        v(xb, y0 + 0.06, z1),
        Vec2::ONE,
    );
    slab(
        &mut k,
        STEEL,
        v(xa, y1 - 0.06, z0),
        v(xb, y1, z1),
        Vec2::ONE,
    );
    for z in [z0, z1 - 0.06] {
        slab(&mut k, STEEL, v(xa, y0, z), v(xb, y1, z + 0.06), Vec2::ONE);
    }
    slab(&mut k, STEEL, v(xa, 2.2, z0), v(xb, 2.25, z1), Vec2::ONE);
    for z in [-6.0, -4.6, -3.2] {
        slab(
            &mut k,
            STEEL,
            v(xa, y0, z - 0.025),
            v(xb, y1, z + 0.025),
            Vec2::ONE,
        );
    }
    slab(
        &mut k,
        GLASS,
        v(4.2, y0 + 0.06, z0 + 0.06),
        v(4.212, y1 - 0.06, z1 - 0.06),
        Vec2::ONE,
    );
    // Courtyard walls, 1.3 m high.
    for (a, b) in [
        (v(6.6, 0.0, 0.4), v(7.0, 1.3, 9.0)),
        (v(-7.0, 0.0, 0.4), v(-6.6, 1.3, 9.0)),
        (v(-7.0, 0.0, 8.6), v(7.0, 1.3, 9.0)),
    ] {
        slab(&mut k, SAND, a, b, Vec2::ONE);
    }
    // A 2 cm charcoal render skin over the building's outer faces. The exposure is set for
    // the room, about five stops under the sunlit courtyard, so everything outside is dark
    // enough to keep its modelling there: a dark pavilion round a white room.
    for (a, b) in [
        (v(-4.42, 0.0, 0.4), v(-2.2, 4.42, 0.42)),
        (v(2.2, 0.0, 0.4), v(4.42, 4.42, 0.42)),
        (v(-2.2, 3.4, 0.4), v(2.2, 4.42, 0.42)),
        // East wall around its window (z -7.4..-1.8, y 0.4..3.6).
        (v(4.4, 0.0, -9.42), v(4.42, 4.4, -7.4)),
        (v(4.4, 0.0, -1.8), v(4.42, 4.4, 0.42)),
        (v(4.4, 0.0, -7.4), v(4.42, 0.4, -1.8)),
        (v(4.4, 3.6, -7.4), v(4.42, 4.4, -1.8)),
        (v(-4.42, 0.0, -9.42), v(-4.4, 4.4, 0.42)),
        (v(-4.4, 0.0, -9.42), v(4.4, 4.4, -9.4)),
    ] {
        slab(&mut k, SAND, a, b, Vec2::ONE);
    }
    // The long-shadow test, south-west of the building: three 14 m masts of 0.4, 0.2 and
    // 0.1 m diameter stand 8 m in front of a 7 m boundary wall that faces the sun. Each
    // mast's shadow crosses the gravel and climbs the wall 4.6 m to its west, as a stripe
    // 0.46, 0.23 and 0.12 m wide. From the `shadow` camera the masts are 42 to 44 m away
    // and their shadows on the wall 50 to 54 m.
    slab(
        &mut k,
        BOUNDARY,
        v(-46.0, -0.31, -44.5),
        v(2.0, 6.7, -44.0),
        Vec2::ONE,
    );
    for (x, radius) in [(-22.0, 0.2), (-16.0, 0.1), (-10.0, 0.05)] {
        post(&mut k, MAST, v(x, -0.31, -36.0), radius, 14.0);
    }
    k.write(root, lock, "atelier_shell")
}

/// The table's centre on the floor.
pub const TABLE: Vec3 = Vec3::new(0.2, 0.0, -6.0);
/// The table lamp's glowing globe, from `TABLE`: it rests on the nickel tray.
pub const GLOBE: Vec3 = Vec3::new(0.45, 0.877, -0.03);
const GLOBE_RADIUS: f32 = 0.1;
/// The floor lamp's foot on the floor.
const FLOOR_LAMP: Vec3 = Vec3::new(-3.4, 0.0, -5.7);

/// The room's lamp fittings that enclose their own light: the table lamp's glowing globe,
/// the pendant's brass shade and bulb, and the floor lamp's bulb. Their own model, because
/// a fitting must not shadow the light inside it and that is a switch on the mesh renderer
/// (`castShadows: false`), not on a part of a model.
pub fn lamps(root: &Path, lock: &Lock) -> Result<String> {
    const BRASS: usize = 0;
    const BULB: usize = 1;
    let mut k = Kit::new(vec![
        Material::flat("brushed brass", [0.93, 0.72, 0.36], 0.28, 1.0),
        Material::emitter("warm bulb", [1.0, 0.8, 0.55]),
    ]);
    let v = Vec3::new;
    let t = TABLE;
    k.sphere(BULB, t + GLOBE, GLOBE_RADIUS, 32, Vec2::ONE);
    k.cylinder(
        BRASS,
        0.22,
        0.22,
        32,
        Mat4::from_translation(v(t.x, 2.45, t.z)),
        Vec2::ONE,
    );
    k.sphere(BULB, v(t.x, 2.5, t.z), 0.1, 24, Vec2::ONE);
    k.sphere(BULB, FLOOR_LAMP + v(0.0, 1.7, 0.0), 0.09, 24, Vec2::ONE);
    k.write(root, lock, "atelier_lamps")
}

/// The furniture and objects of the room: sculpture on its plinth, the table with
/// glass, the nickel tray and chrome sphere of the glow group, the armchair, bookshelf,
/// the lamps' stems and cord (their glowing parts and the pendant's shade are `lamps`), rug
/// and the painting.
pub fn furniture(root: &Path, lock: &Lock) -> Result<String> {
    const OAK: usize = 0;
    const WALNUT: usize = 1;
    const LEATHER: usize = 2;
    const STEEL: usize = 3;
    const BRASS: usize = 4;
    const CHROME: usize = 5;
    const MARBLE: usize = 6;
    const GLASS: usize = 7;
    const RUG: usize = 9;
    const BOOKS: usize = 10;
    const TERRACOTTA_BOOK: usize = 13;
    const NICKEL: usize = 17;
    let mut k = Kit::new(vec![
        Material::flat("dark oak", [0.2, 0.12, 0.07], 0.5, 0.0),
        Material::flat("walnut", [0.3, 0.18, 0.1], 0.4, 0.0),
        Material::flat("graphite leather", [0.09, 0.09, 0.1], 0.45, 0.0),
        Material::flat("blackened steel", [0.07, 0.07, 0.075], 0.4, 1.0),
        Material::flat("brushed brass", [0.93, 0.72, 0.36], 0.28, 1.0),
        Material::flat("chrome", [0.95, 0.95, 0.95], 0.03, 1.0),
        Material::flat("black stone plinth", [0.04, 0.04, 0.045], 0.4, 0.0),
        Material::glass("clear glass", [0.9, 0.97, 1.0], 0.16),
        // Index 8 was the bulbs' emitter, now in `lamps`; kept so the indices below hold.
        Material::flat("unused", [0.5, 0.5, 0.5], 1.0, 0.0),
        Material::flat("oatmeal wool rug", [0.66, 0.6, 0.5], 1.0, 0.0),
        Material::flat("book cream", [0.78, 0.74, 0.64], 0.75, 0.0),
        Material::flat("book charcoal", [0.12, 0.12, 0.13], 0.7, 0.0),
        Material::flat("book tan", [0.5, 0.4, 0.28], 0.75, 0.0),
        Material::flat("book terracotta", [0.55, 0.22, 0.12], 0.7, 0.0),
        Material::flat("canvas white", [0.85, 0.84, 0.8], 0.9, 0.0),
        Material::flat("canvas charcoal", [0.14, 0.14, 0.15], 0.85, 0.0),
        Material::flat("canvas oatmeal", [0.62, 0.55, 0.42], 0.85, 0.0),
        // A near-mirror: just over the roughness under which a reflection is one ray.
        Material::flat("polished nickel", [0.72, 0.7, 0.66], 0.07, 1.0),
    ]);
    let v = Vec3::new;
    // Sculpture: marble plinth, brass disc and a chrome sphere in the front sun patch.
    let s = v(-1.6, 0.0, -2.4);
    block(&mut k, MARBLE, s + v(0.0, 0.45, 0.0), v(0.7, 0.9, 0.7));
    post(&mut k, BRASS, s + v(0.0, 0.9, 0.0), 0.3, 0.04);
    k.sphere(CHROME, s + v(0.0, 1.27, 0.0), 0.33, 48, Vec2::ONE);
    // Table (walnut top on steel legs), rug beneath, objects on it.
    let t = TABLE;
    block(&mut k, WALNUT, t + v(0.0, 0.74, 0.0), v(1.9, 0.05, 0.95));
    for (dx, dz) in [(-0.9, -0.42), (0.9, -0.42), (-0.9, 0.42), (0.9, 0.42)] {
        post(&mut k, STEEL, t + v(dx, 0.0, dz), 0.025, 0.74);
    }
    block(&mut k, RUG, t + v(0.0, 0.006, 0.0), v(3.4, 0.012, 2.4));
    for (dx, dz, h, yaw) in [
        (-0.45, 0.1, 0.22, 10.0),
        (-0.15, -0.12, 0.3, 40.0),
        (0.05, 0.14, 0.16, 25.0),
    ] {
        turned(
            &mut k,
            GLASS,
            t + v(dx, 0.765 + h * 0.5, dz),
            v(0.16, h, 0.16),
            yaw,
        );
    }
    // The glow group, in the table's shade: a glowing globe (the table lamp) on a polished
    // nickel tray, with a small chrome sphere beside it. The tray shows the globe in a
    // near-mirror and the sphere shows it in chrome; the `glow` camera frames the three.
    block(
        &mut k,
        NICKEL,
        t + v(0.605, 0.771, -0.13),
        v(0.65, 0.012, 0.54),
    );
    k.sphere(CHROME, t + v(0.47, 0.852, -0.29), 0.075, 32, Vec2::ONE);
    // Pendant over the table: its cord. The shade and bulb are in `lamps`.
    post(&mut k, STEEL, v(t.x, 2.7, t.z), 0.006, 1.3);
    // Armchair in leather on dark oak legs.
    let a = v(-2.85, 0.0, -7.1);
    let yaw = 35.0;
    let at = |dx: f32, y: f32, dz: f32| {
        let p = Mat4::from_rotation_y(f32::to_radians(yaw)).transform_point3(v(dx, y, dz));
        a + p
    };
    turned(&mut k, LEATHER, at(0.0, 0.42, 0.0), v(0.9, 0.22, 0.85), yaw);
    turned(
        &mut k,
        LEATHER,
        at(0.0, 0.8, -0.38),
        v(0.9, 0.55, 0.14),
        yaw,
    );
    for dx in [-0.5, 0.5] {
        turned(&mut k, LEATHER, at(dx, 0.62, 0.0), v(0.14, 0.34, 0.85), yaw);
    }
    for (dx, dz) in [(-0.38, -0.34), (0.38, -0.34), (-0.38, 0.34), (0.38, 0.34)] {
        let p = at(dx, 0.0, dz);
        post(&mut k, OAK, p, 0.03, 0.3);
    }
    // Floor lamp beside the chair: brass stem and base. Its bulb is in `lamps`.
    let l = FLOOR_LAMP;
    post(&mut k, BRASS, l, 0.16, 0.03);
    post(&mut k, BRASS, l, 0.012, 1.55);
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
            block(
                &mut k,
                material.min(BOOKS + 3),
                b + v(x + w * 0.5, y + h * 0.5, 0.0),
                v(w, h, 0.26),
            );
            x += w + 0.004;
        }
    }
    // A large canvas on the west wall between the terracotta panel and the reading corner:
    // the same three colours as the small painting, in a thin steel frame.
    let c = v(-3.96, 1.75, -5.0);
    block(&mut k, STEEL, c, v(0.03, 1.5, 2.2));
    block(&mut k, 14, c + v(0.02, 0.0, 0.0), v(0.02, 1.42, 2.12));
    block(&mut k, 15, c + v(0.035, -0.1, 0.35), v(0.01, 0.9, 1.0));
    block(&mut k, 16, c + v(0.035, 0.2, -0.5), v(0.01, 0.7, 0.7));
    block(
        &mut k,
        TERRACOTTA_BOOK,
        c + v(0.04, -0.38, -0.55),
        v(0.01, 0.3, 0.5),
    );
    // A low walnut console under it, with a brass bowl and three books lying flat.
    let console = v(-3.72, 0.0, -5.0);
    block(
        &mut k,
        WALNUT,
        console + v(0.0, 0.42, 0.0),
        v(0.42, 0.05, 2.0),
    );
    for z in [-0.9, 0.9] {
        block(&mut k, STEEL, console + v(0.0, 0.2, z), v(0.36, 0.4, 0.03));
    }
    k.sphere(BRASS, console + v(0.0, 0.56, 0.55), 0.12, 32, Vec2::ONE);
    for (i, material) in [BOOKS, BOOKS + 1, TERRACOTTA_BOOK].into_iter().enumerate() {
        block(
            &mut k,
            material,
            console + v(0.0, 0.465 + i as f32 * 0.04, -0.5),
            v(0.3 - i as f32 * 0.03, 0.036, 0.22),
        );
    }
    // Two books and a closed notebook on the table's near corner.
    for (i, material) in [BOOKS + 1, BOOKS].into_iter().enumerate() {
        turned(
            &mut k,
            material,
            t + v(0.6, 0.785 + i as f32 * 0.04, 0.28),
            v(0.3, 0.036, 0.22),
            12.0 - i as f32 * 20.0,
        );
    }
    // A painting on the back wall: canvas blocks in a steel frame.
    let p = v(1.6, 1.9, -8.97);
    block(&mut k, STEEL, p, v(1.3, 1.0, 0.03));
    block(&mut k, 14, p + v(0.0, 0.0, 0.02), v(1.22, 0.92, 0.02));
    block(&mut k, 15, p + v(-0.2, -0.05, 0.035), v(0.6, 0.62, 0.01));
    block(&mut k, 16, p + v(0.3, 0.12, 0.035), v(0.4, 0.45, 0.01));
    block(
        &mut k,
        TERRACOTTA_BOOK,
        p + v(0.25, -0.25, 0.04),
        v(0.3, 0.2, 0.01),
    );
    k.write(root, lock, "atelier_furniture")
}

/// A lantern's frame round its centre `c`: four corner bars (not a box: a solid frame
/// round the glass hid it completely), a cap, a foot and two bands.
fn lantern_frame(k: &mut Kit, steel: usize, c: Vec3) {
    let v = Vec3::new;
    for (dx, dz) in [(-0.09, -0.09), (0.09, -0.09), (-0.09, 0.09), (0.09, 0.09)] {
        block(k, steel, c + v(dx, 0.0, dz), v(0.02, 0.34, 0.02));
    }
    block(k, steel, c + v(0.0, 0.18, 0.0), v(0.24, 0.03, 0.24));
    block(k, steel, c + v(0.0, -0.18, 0.0), v(0.22, 0.03, 0.22));
    for y in [-0.08, 0.08] {
        block(k, steel, c + v(0.0, y, 0.0), v(0.2, 0.012, 0.2));
    }
}
/// The glowing glass of the lanterns at `centres`, as its own model (`name`): the glass
/// encloses the lantern's light, so its mesh renderer must not cast shadows (`lamps`).
fn lantern_glass(root: &Path, lock: &Lock, name: &str, centres: &[Vec3]) -> Result<String> {
    let mut k = Kit::new(vec![Material::emitter("lantern glass", [1.0, 0.78, 0.5])]);
    for c in centres {
        block(&mut k, 0, *c, Vec3::new(0.17, 0.3, 0.17));
    }
    k.write(root, lock, name)
}
/// The centre of the courtyard wall lantern's glass, where its light is.
pub const WALL_LANTERN: Vec3 = Vec3::new(3.3, 2.4, 0.68);
/// The courtyard wall lantern's glass.
pub fn wall_lantern_glass(root: &Path, lock: &Lock) -> Result<String> {
    lantern_glass(root, lock, "atelier_lantern_glass", &[WALL_LANTERN])
}
/// The night lanterns' glass.
pub fn night_glass(root: &Path, lock: &Lock) -> Result<String> {
    lantern_glass(root, lock, "atelier_night_glass", &night_lanterns())
}

/// The night scene's extra lanterns, by the centre of each one's glass (where its light
/// is): the wall lantern's twin left of the doorway, then eighteen on posts along the
/// tops of the courtyard walls (five west, five east, eight on the far wall).
pub fn night_lanterns() -> Vec<Vec3> {
    let v = Vec3::new;
    // Wall top 1.3, a 0.5 m post, then the body's foot.
    let y = 1.3 + 0.5 + 0.195;
    let sides = [1.6, 3.2, 4.8, 6.4, 8.0];
    std::iter::once(v(-3.3, 2.4, 0.68))
        .chain(sides.iter().map(|z| v(-6.8, y, *z)))
        .chain(sides.iter().map(|z| v(6.8, y, *z)))
        .chain((0..8).map(|i| v(-5.25 + 1.5 * i as f32, y, 8.8)))
        .collect()
}

/// The night scene's extra lanterns (`night_lanterns`) without their glass
/// (`night_glass`): the courtyard wall lantern's frame, the first on a bracket on the
/// front wall, the rest on posts.
pub fn night(root: &Path, lock: &Lock) -> Result<String> {
    const STEEL: usize = 0;
    let mut k = Kit::new(vec![Material::flat(
        "black steel",
        [0.05, 0.05, 0.055],
        0.42,
        1.0,
    )]);
    let v = Vec3::new;
    for (i, c) in night_lanterns().into_iter().enumerate() {
        if i == 0 {
            // As the courtyard's: an arm from the wall and a bracket behind the body.
            block(&mut k, STEEL, c + v(0.0, 0.0, -0.184), v(0.03, 0.03, 0.128));
            block(&mut k, STEEL, c + v(0.0, 0.0, -0.1), v(0.04, 0.5, 0.04));
        } else {
            post(&mut k, STEEL, v(c.x, 1.3, c.z), 0.02, 0.5);
        }
        lantern_frame(&mut k, STEEL, c);
    }
    k.write(root, lock, "atelier_night")
}

/// Courtyard dressing: a bench, weathered-steel planters and the wall lantern's frame.
pub fn courtyard(root: &Path, lock: &Lock) -> Result<String> {
    const OAK: usize = 0;
    const STONE: usize = 1;
    const STEEL: usize = 2;
    const SOIL: usize = 4;
    const NICKEL: usize = 5;
    let mut k = Kit::new(vec![
        Material::flat("teak", [0.4, 0.25, 0.12], 0.6, 0.0),
        Material::flat("weathered steel", [0.13, 0.055, 0.03], 0.85, 0.0),
        Material::flat("black steel", [0.05, 0.05, 0.055], 0.42, 1.0),
        // Index 3 was the lantern's glass, now `wall_lantern_glass`; kept so the indices hold.
        Material::flat("unused", [0.5, 0.5, 0.5], 1.0, 0.0),
        Material::flat("dark soil", [0.1, 0.07, 0.05], 1.0, 0.0),
        // The furniture's near-mirror, for the lantern's backplate.
        Material::flat("polished nickel", [0.72, 0.7, 0.66], 0.07, 1.0),
    ]);
    let v = Vec3::new;
    // Bench: slatted seat on steel legs, against the west wall.
    let bench = v(-5.6, 0.0, 4.0);
    for i in 0..5 {
        block(
            &mut k,
            OAK,
            bench + v(0.0, 0.46, -0.28 + i as f32 * 0.14),
            v(1.6, 0.04, 0.11),
        );
    }
    for x in [-0.7, 0.7] {
        block(&mut k, STEEL, bench + v(x, 0.22, 0.0), v(0.04, 0.44, 0.7));
    }
    // Planters along the east wall and by the doorway.
    for (x, z, w, d) in [
        (5.7, 2.5, 0.8, 2.0),
        (5.7, 6.0, 0.8, 2.4),
        (-2.6, 1.3, 1.4, 0.7),
        (2.6, 1.3, 1.4, 0.7),
    ] {
        block(&mut k, STONE, v(x, 0.3, z), v(w, 0.6, d));
        block(&mut k, SOIL, v(x, 0.605, z), v(w - 0.08, 0.02, d - 0.08));
    }
    // Wall lantern right of the doorway: backplate, arm, bracket and frame. Its glowing
    // glass is `wall_lantern_glass`.
    let l = v(3.3, 2.4, 0.5);
    // A polished plate on the wall behind it, set to its right and a little high so the
    // `lantern` camera, above and to the right, sees the lantern's reflection beside the
    // lantern: the scene's emitter in a near-mirror in daylight.
    block(&mut k, NICKEL, v(3.42, 2.45, 0.426), v(0.6, 0.6, 0.012));
    block(&mut k, STEEL, l + v(0.0, 0.0, -0.004), v(0.03, 0.03, 0.128));
    block(&mut k, STEEL, l + v(0.0, 0.0, 0.08), v(0.04, 0.5, 0.04));
    lantern_frame(&mut k, STEEL, WALL_LANTERN);
    k.write(root, lock, "atelier_courtyard")
}
