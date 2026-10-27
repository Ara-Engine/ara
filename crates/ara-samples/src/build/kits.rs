//! The generated kit pieces the samples share: shipping containers, the guard hut,
//! the hangar shell, the radio mast, slabs, light panels, glass, cables, puddles and
//! the road ribbon. Each writes a glTF under `kit/` (see `crate::kit`) that
//! textures itself with fetched CC0 sets; the build registers it like any model.
use crate::{
    kit::{Kit, Material, meters},
    lock::Lock,
};
use anyhow::Result;
use glam::{Mat4, Quat, Vec2, Vec3};
use std::path::Path;

fn at(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, z))
}

/// A 20 ft shipping container (6.06 × 2.59 × 2.44 m) standing on y = 0, long axis X,
/// doors at +X. `body` is the corrugated side texture set.
pub fn container(
    project: &Path,
    lock: &Lock,
    name: &str,
    body: &str,
    tint: [f32; 3],
) -> Result<String> {
    let frame = "rusty_metal_02";
    let mut kit = Kit::new(vec![
        Material::textured("body", body).tinted(tint),
        Material::textured("frame", frame).with(1.0, 1.0),
        Material::flat("door bars", [0.18, 0.17, 0.16], 0.55, 1.0),
    ]);
    let (l, h, w) = (6.06, 2.59, 2.44);
    let m = meters(lock, body);
    kit.cuboid(
        0,
        Vec3::new(l - 0.2, h - 0.25, w - 0.12),
        at(0.0, h / 2.0, 0.0),
        m,
    );
    let fm = meters(lock, frame);
    for (x, z) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        kit.cuboid(
            1,
            Vec3::new(0.16, h, 0.16),
            at(x * (l / 2.0 - 0.08), h / 2.0, z * (w / 2.0 - 0.08)),
            fm,
        );
    }
    for y in [0.08, h - 0.08] {
        for z in [1.0f32, -1.0] {
            kit.cuboid(
                1,
                Vec3::new(l, 0.16, 0.12),
                at(0.0, y, z * (w / 2.0 - 0.06)),
                fm,
            );
        }
        for x in [1.0f32, -1.0] {
            kit.cuboid(
                1,
                Vec3::new(0.12, 0.16, w),
                at(x * (l / 2.0 - 0.06), y, 0.0),
                fm,
            );
        }
    }
    for z in [-0.85, -0.35, 0.35, 0.85] {
        kit.cylinder(2, 0.022, h - 0.4, 8, at(l / 2.0 + 0.02, 0.2, z), Vec2::ONE);
    }
    kit.write(project, lock, name)
}

/// The guard hut: 3.2 × 3.2 m concrete-block walls 2.7 m high with a door gap and a
/// glazed window facing +Z, under an overhanging corrugated roof.
pub fn guard_hut(project: &Path, lock: &Lock, name: &str) -> Result<String> {
    let (wall, roof) = ("concrete_block_wall", "corrugated_iron_02");
    let mut kit = Kit::new(vec![
        Material::textured("walls", wall),
        Material::textured("roof", roof).with(1.0, 1.0),
        Material::glass("window", [0.6, 0.65, 0.62], 0.25),
        Material::textured("door", "painted_metal_shutter").with(1.0, 1.0),
    ]);
    let (s, h, t) = (3.2, 2.7, 0.2);
    let wm = meters(lock, wall);
    kit.cuboid(
        0,
        Vec3::new(s, h, t),
        at(0.0, h / 2.0, -s / 2.0 + t / 2.0),
        wm,
    );
    kit.cuboid(
        0,
        Vec3::new(t, h, s),
        at(-s / 2.0 + t / 2.0, h / 2.0, 0.0),
        wm,
    );
    kit.cuboid(
        0,
        Vec3::new(t, h, s),
        at(s / 2.0 - t / 2.0, h / 2.0, 0.0),
        wm,
    );
    // Front: door gap (x -1.3..-0.3) and window (x 0.2..1.3, y 1.0..1.9).
    let z = s / 2.0 - t / 2.0;
    kit.cuboid(0, Vec3::new(0.3, h, t), at(-1.45, h / 2.0, z), wm);
    kit.cuboid(0, Vec3::new(0.5, h, t), at(-0.05, h / 2.0, z), wm);
    kit.cuboid(0, Vec3::new(0.3, h, t), at(1.45, h / 2.0, z), wm);
    kit.cuboid(
        0,
        Vec3::new(1.0, h - 2.1, t),
        at(-0.8, (h + 2.1) / 2.0, z),
        wm,
    );
    kit.cuboid(0, Vec3::new(1.1, 1.0, t), at(0.75, 0.5, z), wm);
    kit.cuboid(
        0,
        Vec3::new(1.1, h - 1.9, t),
        at(0.75, (h + 1.9) / 2.0, z),
        wm,
    );
    kit.cuboid(2, Vec3::new(1.1, 0.9, 0.02), at(0.75, 1.45, z), Vec2::ONE);
    // Door ajar.
    kit.cuboid(
        3,
        Vec3::new(0.95, 2.05, 0.04),
        Mat4::from_translation(Vec3::new(-1.3, 1.03, z + 0.1))
            * Mat4::from_rotation_y(-0.9)
            * at(0.47, 0.0, 0.0),
        meters(lock, "painted_metal_shutter"),
    );
    kit.cuboid(
        1,
        Vec3::new(s + 0.6, 0.06, s + 0.6),
        Mat4::from_translation(Vec3::new(0.0, h + 0.1, 0.0)) * Mat4::from_rotation_x(-0.06),
        meters(lock, roof),
    );
    kit.write(project, lock, name)
}

/// The hangar shell, width along X, depth along Z, door opening facing -Z:
/// corrugated walls on a steel frame, a pitched roof with a clerestory glass strip,
/// a concrete floor and two sliding doors (one pushed aside).
pub struct Hangar {
    /// Width along X (m).
    pub width: f32,
    /// Depth along Z (m).
    pub depth: f32,
    /// Eave height (m).
    pub height: f32,
    /// Door opening width (m).
    pub door: f32,
    /// Door opening height (m).
    pub door_height: f32,
}

impl Hangar {
    /// The Outpost hangar.
    pub const OUTPOST: Self = Self {
        width: 20.0,
        depth: 26.0,
        height: 7.0,
        door: 10.0,
        door_height: 6.0,
    };

    /// Writes the shell kit.
    pub fn write(&self, project: &Path, lock: &Lock, name: &str) -> Result<String> {
        let (skin, steel, floor) = (
            "worn_corrugated_iron",
            "green_metal_rust",
            "hangar_concrete_floor",
        );
        let mut kit = Kit::new(vec![
            Material::textured("cladding", skin).with(1.0, 1.0),
            Material::textured("steel", steel).with(1.0, 1.0),
            Material::textured("floor", floor),
            Material::glass("clerestory", [0.55, 0.6, 0.6], 0.3),
            Material::textured("doors", "rusty_corrugated_iron").with(1.0, 1.0),
        ]);
        let (w, d, h, t) = (self.width, self.depth, self.height, 0.12);
        let (cm, sm) = (meters(lock, skin), meters(lock, steel));
        // Floor slab (top at y = 0.05).
        kit.cuboid(
            2,
            Vec3::new(w + 1.0, 0.3, d + 1.0),
            at(0.0, -0.1, 0.0),
            meters(lock, floor),
        );
        // Side and back walls up to the eaves, leaving a 0.6 m glazed strip.
        let wall = h - 0.6;
        kit.cuboid(0, Vec3::new(t, wall, d), at(-w / 2.0, wall / 2.0, 0.0), cm);
        kit.cuboid(0, Vec3::new(t, wall, d), at(w / 2.0, wall / 2.0, 0.0), cm);
        kit.cuboid(0, Vec3::new(w, wall, t), at(0.0, wall / 2.0, d / 2.0), cm);
        for x in [-w / 2.0, w / 2.0] {
            kit.cuboid(
                3,
                Vec3::new(0.03, 0.6, d),
                at(x, wall + 0.3, 0.0),
                Vec2::ONE,
            );
        }
        // Front wall around the door opening.
        let side = (w - self.door) / 2.0;
        for x in [-1.0, 1.0] {
            kit.cuboid(
                0,
                Vec3::new(side, h, t),
                at(x * (self.door / 2.0 + side / 2.0), h / 2.0, -d / 2.0),
                cm,
            );
        }
        kit.cuboid(
            0,
            Vec3::new(self.door, h - self.door_height, t),
            at(0.0, (h + self.door_height) / 2.0, -d / 2.0),
            cm,
        );
        // Gables: stacked strips stepping in under the 2.5 m pitched roof.
        let rise = 2.5;
        for i in 0..10 {
            let y0 = h + i as f32 * rise / 10.0;
            let span = w * (1.0 - (i as f32 + 0.5) / 10.0);
            for z in [-d / 2.0, d / 2.0] {
                kit.cuboid(
                    0,
                    Vec3::new(span, rise / 10.0, t),
                    at(0.0, y0 + rise / 20.0, z),
                    cm,
                );
            }
        }
        // Roof planes.
        let half = (w / 2.0).hypot(rise);
        let pitch = (rise / (w / 2.0)).atan();
        for s in [-1.0f32, 1.0] {
            let rot = Mat4::from_rotation_z(s * pitch);
            kit.cuboid(
                0,
                Vec3::new(half + 0.4, 0.08, d + 0.6),
                at(s * (-w / 4.0), h + rise / 2.0 + 0.05, 0.0) * rot,
                cm,
            );
        }
        // Portal frames every 6.5 m: columns, rafters, a tie beam.
        let frames = (d / 6.5).round() as i32;
        for i in 0..=frames {
            let z = -d / 2.0 + i as f32 * d / frames as f32;
            for x in [-w / 2.0 + 0.25, w / 2.0 - 0.25] {
                kit.cuboid(1, Vec3::new(0.3, h, 0.3), at(x, h / 2.0, z), sm);
            }
            for s in [-1.0f32, 1.0] {
                kit.cuboid(
                    1,
                    Vec3::new(half, 0.35, 0.22),
                    at(s * (-w / 4.0), h + rise / 2.0 - 0.25, z) * Mat4::from_rotation_z(s * pitch),
                    sm,
                );
            }
            kit.cuboid(1, Vec3::new(w - 0.5, 0.2, 0.18), at(0.0, h - 0.1, z), sm);
        }
        // Sliding doors: one closed on the left half, one pushed onto the right wall.
        let dm = meters(lock, "rusty_corrugated_iron");
        let leaf = self.door / 2.0 + 0.2;
        kit.cuboid(
            4,
            Vec3::new(leaf, self.door_height, 0.1),
            at(-self.door / 4.0, self.door_height / 2.0, -d / 2.0 - 0.25),
            dm,
        );
        kit.cuboid(
            4,
            Vec3::new(leaf, self.door_height, 0.1),
            at(
                self.door / 2.0 + leaf / 2.0 - 0.6,
                self.door_height / 2.0,
                -d / 2.0 - 0.35,
            ),
            dm,
        );
        kit.cuboid(
            1,
            Vec3::new(w + 1.0, 0.2, 0.2),
            at(0.0, self.door_height + 0.2, -d / 2.0 - 0.3),
            sm,
        );
        kit.write(project, lock, name)
    }
}

/// A three-legged lattice radio mast `height` m tall on y = 0 (legs taper from a
/// 1.6 m to a 0.5 m triangle), with braces every 2 m, a platform and an antenna.
pub fn mast(project: &Path, lock: &Lock, name: &str, height: f32) -> Result<String> {
    let steel = "rusty_metal_02";
    let mut kit = Kit::new(vec![
        Material::textured("steel", steel).with(1.0, 1.0),
        Material::emitter("beacon", [1.0, 0.08, 0.04]),
        Material::flat("antenna", [0.75, 0.75, 0.72], 0.4, 1.0),
    ]);
    let sm = meters(lock, steel);
    let corner = |y: f32, i: usize| {
        let r = 0.9 - 0.6 * (y / height);
        let a = i as f32 * std::f32::consts::TAU / 3.0;
        Vec3::new(a.cos() * r, y, a.sin() * r)
    };
    let beam = |kit: &mut Kit, a: Vec3, b: Vec3, thickness: f32| {
        let dir = b - a;
        let length = dir.length();
        let rot = Quat::from_rotation_arc(Vec3::Y, dir / length);
        kit.cuboid(
            0,
            Vec3::new(thickness, length, thickness),
            Mat4::from_rotation_translation(rot, (a + b) * 0.5),
            sm,
        );
    };
    let segments = (height / 2.0).round() as usize;
    for s in 0..segments {
        let (y0, y1) = (
            s as f32 * height / segments as f32,
            (s + 1) as f32 * height / segments as f32,
        );
        for i in 0..3 {
            let (a, b) = (corner(y0, i), corner(y1, i));
            beam(&mut kit, a, b, 0.09);
            beam(&mut kit, a, corner(y1, (i + 1) % 3), 0.035);
            beam(&mut kit, corner(y1, i), corner(y1, (i + 1) % 3), 0.04);
        }
    }
    kit.cuboid(
        0,
        Vec3::new(1.4, 0.06, 1.4),
        at(0.0, height * 0.72, 0.0),
        sm,
    );
    kit.cylinder(2, 0.04, 4.0, 10, at(0.0, height, 0.0), Vec2::ONE);
    kit.cylinder(2, 0.08, 0.6, 10, at(0.25, height - 1.5, 0.0), Vec2::ONE);
    kit.sphere(1, Vec3::new(0.0, height + 4.1, 0.0), 0.12, 8, Vec2::ONE);
    kit.write(project, lock, name)
}

/// A rectangular slab (`size`) of a texture set, top face at y = 0.
pub fn slab(project: &Path, lock: &Lock, name: &str, set: &str, size: Vec3) -> Result<String> {
    let mut kit = Kit::new(vec![Material::textured("slab", set)]);
    kit.cuboid(0, size, at(0.0, -size.y / 2.0, 0.0), meters(lock, set));
    kit.write(project, lock, name)
}

/// A flat emissive light panel (`size`), facing -Y, with a thin housing.
pub fn light_panel(
    project: &Path,
    lock: &Lock,
    name: &str,
    size: Vec2,
    tint: [f32; 3],
) -> Result<String> {
    let mut kit = Kit::new(vec![
        Material::emitter("diffuser", tint),
        Material::flat("housing", [0.32, 0.33, 0.34], 0.5, 1.0),
    ]);
    kit.cuboid(
        1,
        Vec3::new(size.x + 0.04, 0.06, size.y + 0.04),
        at(0.0, 0.03, 0.0),
        Vec2::ONE,
    );
    kit.cuboid(
        0,
        Vec3::new(size.x, 0.01, size.y),
        at(0.0, -0.005, 0.0),
        Vec2::ONE,
    );
    kit.write(project, lock, name)
}

/// A glass pane (`size` in X/Y, facing ±Z) in a steel frame.
pub fn window(project: &Path, lock: &Lock, name: &str, size: Vec2) -> Result<String> {
    let mut kit = Kit::new(vec![
        Material::glass("glass", [0.7, 0.75, 0.75], 0.18),
        Material::flat("frame", [0.12, 0.13, 0.13], 0.45, 1.0),
    ]);
    kit.cuboid(
        0,
        Vec3::new(size.x, size.y, 0.01),
        at(0.0, size.y / 2.0, 0.0),
        Vec2::ONE,
    );
    for (sx, sy, x, y) in [
        (size.x + 0.1, 0.05, 0.0, 0.0),
        (size.x + 0.1, 0.05, 0.0, size.y),
        (0.05, size.y, -size.x / 2.0, size.y / 2.0),
        (0.05, size.y, size.x / 2.0, size.y / 2.0),
        (0.04, size.y, 0.0, size.y / 2.0),
    ] {
        kit.cuboid(1, Vec3::new(sx, sy, 0.05), at(x, y, 0.0), Vec2::ONE);
    }
    kit.write(project, lock, name)
}

/// A sagging cable between `a` and `b` (model space), as thin box segments.
pub fn cable(kit: &mut Kit, material: usize, a: Vec3, b: Vec3, sag: f32, thickness: f32) {
    let points: Vec<Vec3> = (0..=12)
        .map(|i| {
            let t = i as f32 / 12.0;
            a.lerp(b, t) - Vec3::Y * sag * 4.0 * t * (1.0 - t)
        })
        .collect();
    for pair in points.windows(2) {
        let dir = pair[1] - pair[0];
        let length = dir.length();
        let rot = Quat::from_rotation_arc(Vec3::Y, dir / length);
        kit.cuboid(
            material,
            Vec3::new(thickness, length + thickness, thickness),
            Mat4::from_rotation_translation(rot, (pair[0] + pair[1]) * 0.5),
            Vec2::ONE,
        );
    }
}

/// A puddle: a flat, irregular disc of still water 1.5 cm above y = 0.
pub fn puddle(project: &Path, lock: &Lock, name: &str, radius: f32, seed: u32) -> Result<String> {
    let mut kit = Kit::new(vec![Material::flat(
        "water",
        [0.035, 0.032, 0.03],
        0.02,
        0.0,
    )]);
    kit.disc(0, radius, seed, 0.015);
    kit.write(project, lock, name)
}

/// Glowing coals and charred logs for a fire pit (about 0.9 m across, on y = 0).
pub fn embers(project: &Path, lock: &Lock, name: &str) -> Result<String> {
    let mut kit = Kit::new(vec![
        Material::emitter("coals", [1.0, 0.36, 0.08]),
        Material::flat("charred wood", [0.045, 0.038, 0.032], 0.92, 0.0),
    ]);
    let mut seed = 0x51ed_270bu32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    for _ in 0..34 {
        let (a, r) = (rand() * std::f32::consts::TAU, rand().sqrt() * 0.32);
        let p = Vec3::new(a.cos() * r, 0.02 + rand() * 0.05, a.sin() * r);
        kit.sphere(0, p, 0.025 + rand() * 0.035, 6, Vec2::ONE);
    }
    for i in 0..5 {
        let a = i as f32 / 5.0 * std::f32::consts::TAU;
        let rot = Mat4::from_rotation_y(a) * Mat4::from_rotation_z(1.25);
        kit.cylinder(
            1,
            0.045,
            0.62,
            8,
            Mat4::from_translation(Vec3::new(a.cos() * 0.3, 0.02, a.sin() * 0.3)) * rot,
            Vec2::ONE,
        );
    }
    kit.write(project, lock, name)
}
