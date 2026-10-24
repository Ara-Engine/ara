//! "Hangar interior" at night: the Outpost hangar shell (20 × 26 m, door facing −Z,
//! centred on the origin of its own level) fitted out as a workshop. It is lit only
//! by emissive sources and a few local lights: fluorescent ceiling panels (a few dead),
//! a festoon of work-light bulbs across the bay, caged lamps, a task light over the
//! workbench and the blue glare of the welder. Workbenches, tools, shelving, the
//! covered car, an overhead crane, ducts, pipes and cables fill it.
use super::{
    Ctx,
    kits::{self, Hangar},
    outpost::light,
    place::{Models, Prop, Scene},
};
use crate::{
    kit::{Kit, Material},
    lock::Lock,
};
use anyhow::Result;
use glam::{Vec2, Vec3};
use serde_json::json;
use std::path::Path;

/// Level scene.
pub const SCENE: (&str, &str) = ("levels/hangar.scene", "sc_hangar1");

/// Festoon anchor points across the bay (model space of the `festoon` kit).
const FESTOON: [(Vec3, Vec3); 3] = [
    (Vec3::new(-9.6, 5.6, -10.0), Vec3::new(9.6, 5.8, -4.0)),
    (Vec3::new(-9.6, 5.8, -2.0), Vec3::new(9.6, 5.5, 4.0)),
    (Vec3::new(-9.6, 5.5, 6.0), Vec3::new(9.6, 5.8, 11.0)),
];
const BULBS_PER_RUN: usize = 14;

fn bulb_positions() -> Vec<Vec3> {
    let mut out = Vec::new();
    for (a, b) in FESTOON {
        for i in 1..BULBS_PER_RUN {
            let t = i as f32 / BULBS_PER_RUN as f32;
            out.push(a.lerp(b, t) - Vec3::Y * (1.1 * 4.0 * t * (1.0 - t) + 0.12));
        }
    }
    out
}

/// The festoon kit: three sagging cables with emissive bulbs (39 small emitters).
pub fn festoon(root: &Path, lock: &Lock) -> Result<String> {
    let mut kit = Kit::new(vec![
        Material::flat("cable", [0.02, 0.02, 0.02], 0.6, 0.0),
        Material::emitter("bulb", [1.0, 0.78, 0.5]),
    ]);
    for (a, b) in FESTOON {
        kits::cable(&mut kit, 0, a, b, 1.1, 0.012);
    }
    for p in bulb_positions() {
        kit.sphere(1, p, 0.035, 8, Vec2::ONE);
        kit.cuboid(
            0,
            Vec3::new(0.02, 0.12, 0.02),
            glam::Mat4::from_translation(p + Vec3::Y * 0.08),
            Vec2::ONE,
        );
    }
    kit.write(root, lock, "festoon")
}

/// Power cables: from the wall box to the welder and across to the workbench.
pub fn cables(root: &Path, lock: &Lock) -> Result<String> {
    let mut kit = Kit::new(vec![Material::flat(
        "rubber",
        [0.025, 0.025, 0.028],
        0.55,
        0.0,
    )]);
    for (a, b, sag) in [
        (Vec3::new(9.85, 1.4, -6.5), Vec3::new(9.5, 0.02, -6.0), 0.0),
        (Vec3::new(9.5, 0.02, -6.0), Vec3::new(8.6, 0.02, 4.0), 0.0),
        (Vec3::new(8.6, 0.02, 4.0), Vec3::new(6.2, 0.02, 9.4), 0.0),
        (Vec3::new(9.85, 1.3, -6.5), Vec3::new(9.85, 2.6, 2.0), 0.4),
        (Vec3::new(-9.85, 6.4, 8.0), Vec3::new(-2.0, 5.7, 8.0), 0.5),
        (Vec3::new(6.2, 0.02, 9.4), Vec3::new(5.0, 0.02, 7.6), 0.0),
    ] {
        kits::cable(&mut kit, 0, a, b, sag, 0.025);
    }
    kit.write(root, lock, "cables")
}

/// Writes the night hangar level.
pub fn build(ctx: &Ctx, models: &Models) -> Result<()> {
    ctx.new_document(SCENE.0, "scene", SCENE.1)?;
    let mut s = Scene::new(models);
    let h = Hangar::OUTPOST;
    let floor = 0.05;
    let f = |x: f32, z: f32| Vec3::new(x, floor, z);

    s.entity(
        "e_env",
        "Night sky",
        json!({"Environment": {"sky": [0.01, 0.015, 0.035, 1], "horizon": [0.04, 0.045, 0.06, 1], "ground": [0.01, 0.01, 0.01, 1], "intensity": 30, "exposure": 0.02}}),
    );
    s.commands.extend(light(
        "e_moon",
        "Moonlight",
        Vec3::new(0.0, 30.0, 0.0),
        json!({"kind": "directional", "color": [0.55, 0.65, 1.0, 1], "intensity": 0.25}),
        Some(Vec3::new(0.3, -0.8, 0.5)),
    ));
    s.prop(
        Prop::new("e_hangar", "Hangar", "hangar", Vec3::ZERO, 0.0)
            .at_origin()
            .decor(),
    );
    s.entity(
        "e_floorc",
        "Floor collider",
        json!({"Transform": {"position": [0.0, -0.1, 0.0]}, "RigidBody": {}, "Collider": {"halfExtents": [11.0, 0.15, 14.0]}}),
    );
    super::outpost::hangar_colliders_at(&mut s, Vec3::ZERO);

    // Player and camera at the door.
    s.entity(
        "e_player",
        "Salvager",
        json!({
            "Transform": super::place::transform(f(-5.5, -10.0), 30.0, 1.0),
            "MeshRenderer": models.get("human").renderer,
            "Animator": {"controller": "@anim/human.animation#an_locomotion"},
            "CharacterController": {},
            "PlayerControl": {"camera": "e_camera", "walkSpeed": 1.5, "runSpeed": 5.0},
            "AudioListener": {},
        }),
    );
    s.entity(
        "e_camera",
        "Follow camera",
        json!({"Transform": {"position": [1.5, 2.0, -14.0]}, "Camera": {"fov": 60},
            "FollowCamera": {"target": "e_player", "distance": 3.6, "height": 1.55, "pitch": 10}}),
    );

    // Ceiling panels: 3 rows x 4 under the tie beams; two are dead, one dim.
    let rows = [-7.0, 0.5, 8.0];
    let cols = [-6.0, -2.0, 2.0, 6.0];
    let mut n = 0;
    for (r, z) in rows.iter().enumerate() {
        for (c, x) in cols.iter().enumerate() {
            // Night shift: most of the bay is dark, a few tubes still work.
            let radiance = match (r, c) {
                (0, 1) | (1, 2) | (2, 2) => 3600.0,
                (1, 0) => 700.0,
                _ => 0.0,
            };
            s.prop(
                Prop::new(
                    &format!("e_panel{n}"),
                    "Ceiling light panel",
                    "light_panel",
                    Vec3::new(*x, h.height - 0.65, *z),
                    90.0,
                )
                .at_origin()
                .decor()
                .scaled(1.6)
                .renderer(json!({"emissive": [radiance, radiance * 0.98, radiance * 0.92]})),
            );
            n += 1;
        }
    }
    // Festoon work lights (emissive bulbs only: many small emitters for ReSTIR).
    s.prop(
        Prop::new(
            "e_festoon",
            "Festoon work lights",
            "festoon",
            Vec3::ZERO,
            0.0,
        )
        .at_origin()
        .decor()
        .renderer(json!({"emissive": [22000, 22000, 22000]})),
    );
    s.prop(
        Prop::new("e_cables", "Power cables", "cables", Vec3::ZERO, 0.0)
            .at_origin()
            .decor(),
    );

    // The car under its cover, with hanging lamps above.
    s.prop(Prop::new(
        "e_car",
        "Covered car",
        "car",
        f(-3.0, -1.5),
        168.0,
    ));
    for (i, (x, z)) in [(-3.0, -4.0), (-3.0, 1.0)].into_iter().enumerate() {
        let top = Vec3::new(x, h.height - 0.2, z);
        s.prop(
            Prop::new(
                &format!("e_hlamp{i}"),
                "Hanging lamp",
                "hanging_lamp",
                top,
                0.0,
            )
            .at_origin()
            .decor()
            .renderer(json!({"emissive": [9000, 7600, 5600]})),
        );
        s.commands.extend(light(
            &format!("e_hlampl{i}"),
            "Hanging lamp light",
            top - Vec3::Y * 1.45,
            json!({"kind": "spot", "color": [1.0, 0.82, 0.6, 1], "intensity": 2600, "range": 12, "innerAngle": 40, "outerAngle": 70}),
            Some(Vec3::NEG_Y),
        ));
    }

    // Workbench run on the east wall (x = +9.3), drill press and tools on it.
    for (i, z) in [-4.2f32, -2.8, 2.0].into_iter().enumerate() {
        s.prop(Prop::new(
            &format!("e_bench{i}"),
            "Workbench",
            "workbench",
            f(9.3, z),
            270.0,
        ));
    }
    let top = 0.05 + 0.83;
    s.prop(Prop::new(
        "e_drillpress",
        "Bench drill press",
        "drill_press",
        Vec3::new(9.35, top, -4.4),
        270.0,
    ));
    s.prop(
        Prop::new(
            "e_vice",
            "Bench vice",
            "bench_vice",
            Vec3::new(9.2, top + 0.09, -2.4),
            270.0,
        )
        .at_origin()
        .decor(),
    );
    s.prop(
        Prop::new(
            "e_toolchest",
            "Tool chest",
            "tool_chest",
            Vec3::new(9.4, top, -3.1),
            260.0,
        )
        .decor(),
    );
    for (i, (name, x, z, yaw)) in [
        ("multimeter", 9.0, -3.6, 250.0),
        ("wrench", 9.05, -2.9, 0.0),
        ("pipe_wrench", 9.0, -2.1, 0.0),
        ("hammer", 9.1, -1.95, 20.0),
        ("oil_can", 9.5, 1.6, 0.0),
        ("spray_paint", 9.45, 2.3, 30.0),
        ("toolbox", 9.3, 2.0, 270.0),
    ]
    .into_iter()
    .enumerate()
    {
        s.prop(
            Prop::new(
                &format!("e_tool{i}"),
                "Tool",
                name,
                Vec3::new(x, top, z),
                yaw,
            )
            .decor(),
        );
    }
    s.prop(Prop::new("e_stool", "Stool", "stool", f(8.2, -3.4), 30.0));
    s.prop(
        Prop::new(
            "e_benchlight",
            "Bench fluorescent",
            "fluorescent",
            Vec3::new(9.4, 2.4, -3.3),
            90.0,
        )
        .at_origin()
        .decor()
        .renderer(json!({"emissive": [7000, 7000, 6600]})),
    );
    s.commands.extend(light(
        "e_benchlightl",
        "Workbench task light",
        Vec3::new(9.2, 2.3, -3.3),
        json!({"kind": "spot", "color": [0.95, 0.97, 1.0, 1], "intensity": 1800, "range": 6, "innerAngle": 45, "outerAngle": 75}),
        Some(Vec3::new(-0.3, -1.0, 0.0)),
    ));
    s.prop(
        Prop::new(
            "e_powerbox",
            "Power box",
            "power_box",
            Vec3::new(9.88, 1.4, -6.5),
            270.0,
        )
        .at_origin()
        .decor(),
    );
    s.prop(
        Prop::new(
            "e_pipes",
            "Wall pipes",
            "pipes",
            Vec3::new(9.85, 2.2, 6.0),
            270.0,
        )
        .at_origin()
        .decor(),
    );

    // Welding corner: cart, sparks, harsh blue-white light.
    s.prop(Prop::new(
        "e_welder",
        "Welding cart",
        "welding_cart",
        f(6.2, 9.4),
        200.0,
    ));
    s.entity(
        "e_sparks",
        "Welding sparks",
        json!({"Transform": {"position": [5.0, 0.6, 7.6]}, "VisualEffect": {"effect": "sparks"}}),
    );
    s.commands.extend(light(
        "e_weldlight",
        "Welding arc",
        Vec3::new(5.0, 0.7, 7.6),
        json!({"color": [0.6, 0.75, 1.0, 1], "intensity": 2400, "range": 9}),
        None,
    ));
    s.prop(Prop::new(
        "e_toolcart",
        "Tool cart",
        "tool_cart",
        f(1.0, -6.6),
        20.0,
    ));
    s.prop(Prop::new(
        "e_storagecart",
        "Storage cart",
        "storage_cart",
        f(-7.6, -6.0),
        90.0,
    ));

    // Back wall shelving with stock.
    for (i, x) in [-7.2f32, -4.6, 4.6, 7.2].into_iter().enumerate() {
        s.prop(Prop::new(
            &format!("e_shelf{i}"),
            "Shelving",
            "shelves",
            f(x, 12.55),
            180.0,
        ));
    }
    for (i, (name, x, y, z, yaw)) in [
        ("box", -7.8, 0.52, 12.5, 10.0),
        ("box", -7.3, 0.52, 12.6, 80.0),
        ("crate_plastic", -6.6, 0.52, 12.4, 0.0),
        ("jerrycan", -4.9, 1.24, 12.5, 90.0),
        ("jerrycan_green", -4.5, 1.24, 12.5, 90.0),
        ("oil_can", -4.1, 1.24, 12.5, 0.0),
        ("crate_plastic", 4.2, 0.52, 12.4, 0.0),
        ("box", 5.0, 1.24, 12.5, 30.0),
        ("box", 7.4, 0.52, 12.5, 0.0),
        ("gas_mask", 7.0, 1.95, 12.5, 180.0),
    ]
    .into_iter()
    .enumerate()
    {
        s.prop(
            Prop::new(
                &format!("e_stock{i}"),
                "Stock",
                name,
                Vec3::new(x, y, z),
                yaw,
            )
            .decor(),
        );
    }

    // West wall: racks, the office corner, ducts overhead, sconces.
    for (i, z) in [-1.0f32, 1.2].into_iter().enumerate() {
        s.prop(Prop::new(
            &format!("e_rack{i}"),
            "Rack",
            "rack",
            f(-9.4, z),
            90.0,
        ));
    }
    s.prop(Prop::new(
        "e_desk",
        "Office desk",
        "desk",
        f(-8.6, 8.4),
        90.0,
    ));
    s.prop(
        Prop::new(
            "e_laptop",
            "Laptop",
            "laptop",
            Vec3::new(-8.7, 0.84, 8.2),
            100.0,
        )
        .decor(),
    );
    s.prop(
        Prop::new(
            "e_radio",
            "Radio",
            "radio",
            Vec3::new(-8.6, 0.84, 9.2),
            70.0,
        )
        .decor(),
    );
    s.prop(Prop::new(
        "e_deskstool",
        "Stool",
        "stool",
        f(-7.5, 8.6),
        0.0,
    ));
    for (i, z) in [-9.0f32, -6.3, -3.6, -0.9, 1.8, 4.5, 7.2]
        .into_iter()
        .enumerate()
    {
        s.prop(
            Prop::new(
                &format!("e_duct{i}"),
                "Air duct",
                "duct",
                Vec3::new(-8.9, 5.6, z),
                90.0,
            )
            .at_origin()
            .decor(),
        );
    }
    for (i, z) in [-5.0f32, 5.0].into_iter().enumerate() {
        let p = Vec3::new(-9.85, 3.2, z);
        s.prop(
            Prop::new(&format!("e_sconce{i}"), "Caged sconce", "sconce", p, 90.0)
                .at_origin()
                .decor()
                .renderer(json!({"emissive": [6000, 5000, 3600]})),
        );
        s.commands.extend(light(
            &format!("e_sconcel{i}"),
            "Sconce light",
            p + Vec3::new(0.35, -0.1, 0.0),
            json!({"color": [1.0, 0.8, 0.55, 1], "intensity": 900, "range": 8}),
            None,
        ));
    }
    s.prop(Prop::new(
        "e_ladder",
        "Ladder",
        "ladder_tall",
        f(-9.3, -8.6),
        90.0,
    ));

    // Overhead crane across the back bay, barrels and gas by the door.
    s.prop(
        Prop::new(
            "e_crane",
            "Overhead crane",
            "crane",
            Vec3::new(0.0, h.height - 1.6, 6.0),
            0.0,
        )
        .at_origin()
        .decor(),
    );
    for (i, (name, x, z, yaw)) in [
        ("barrel_blue", -8.8, -11.2, 0.0),
        ("barrel_red", -8.1, -11.6, 40.0),
        ("propane", -9.2, -10.4, 0.0),
        ("compressor", 7.6, 9.0, 270.0),
        ("generator", 2.8, 10.6, 180.0),
        ("crate_military", -1.0, 10.8, 5.0),
        ("crate_long", -2.6, 11.1, 90.0),
        ("old_drill_press", 3.6, 3.8, 200.0),
        // A work island mid-floor: crates, barrels and gas by the car.
        ("crate_military", 1.6, 2.4, 10.0),
        ("crate_military", 1.7, 2.5, 95.0),
        ("crate_long", 2.9, 1.6, 30.0),
        ("barrel_rust", -0.6, 3.2, 0.0),
        ("barrel_blue", -0.1, 3.7, 40.0),
        ("lpg", 0.5, 3.0, 0.0),
        ("jerrycan", 0.9, 1.4, 70.0),
        ("box", 3.6, 2.8, 20.0),
        ("box", 3.9, 3.3, 75.0),
    ]
    .into_iter()
    .enumerate()
    {
        s.prop(Prop::new(
            &format!("e_misc{i}"),
            "Clutter",
            name,
            f(x, z),
            yaw,
        ));
    }
    for (i, (x, z, yaw)) in [(-0.4, -3.8, 20.0), (4.0, 5.5, 140.0)]
        .into_iter()
        .enumerate()
    {
        s.prop(
            Prop::new(
                &format!("e_puddle{i}"),
                "Oil puddle",
                if i == 0 { "puddle_b" } else { "puddle_a" },
                Vec3::new(x, floor, z),
                yaw,
            )
            .at_origin()
            .decor(),
        );
    }

    // Ambience.
    s.entity(
        "e_drone",
        "Hangar drone",
        json!({"Transform": {"position": [0.0, 4.0, 0.0]}, "AudioSource": {"clip": format!("@{}#as_drone", ctx.lock.get("oga_freezer").expect("drone").relative("freezer.ogg")), "looping": true, "referenceDistance": 10, "maxDistance": 60, "gain": 0.3}}),
    );

    let calls = std::mem::take(&mut s.commands);
    ctx.apply(
        SCENE.0,
        "Hangar interior at night: workshop fit-out and emissive lighting",
        calls,
    )
}
