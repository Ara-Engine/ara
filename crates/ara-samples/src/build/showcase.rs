//! "Material and character showcase": a studio stage lit by a photo-studio HDRI and
//! a soft key light. The CC0 mannequin idles on a turntable while a second one jogs a
//! loop around the stage; CC0 hero props stand on plinths; an 18-ball material grid
//! (metals, plastics, cloth, skin-like, glass, emissive, scanned surfaces) sits behind.
use super::{
    Ctx, anim, clean,
    outpost::light,
    place::{Models, Prop, Scene, transform},
};
use crate::kit::{Kit, Material};
use anyhow::Result;
use glam::{Mat4, Vec2, Vec3};
use serde_json::json;

/// Level scene.
pub const SCENE: (&str, &str) = ("levels/showcase.scene", "sc_showcase1");
const ANIMATION: (&str, &str) = ("anim/human.animation", "ac_human1");

/// One material ball: name, material, and the radiance scale for emitters.
fn balls() -> Vec<(&'static str, Material, f32)> {
    let flat = Material::flat;
    vec![
        ("gold", flat("gold", [1.0, 0.77, 0.34], 0.18, 1.0), 0.0),
        ("copper", flat("copper", [0.95, 0.64, 0.54], 0.3, 1.0), 0.0),
        ("chrome", flat("chrome", [0.55, 0.56, 0.55], 0.04, 1.0), 0.0),
        (
            "aluminium",
            flat("brushed aluminium", [0.91, 0.92, 0.92], 0.42, 1.0),
            0.0,
        ),
        (
            "rust",
            Material::textured("rusted steel", "rusty_metal_02").with(1.0, 1.0),
            0.0,
        ),
        (
            "plate",
            Material::textured("tread plate", "metal_plate").with(1.0, 1.0),
            0.0,
        ),
        (
            "plastic_red",
            flat("glossy red plastic", [0.6, 0.03, 0.02], 0.12, 0.0),
            0.0,
        ),
        (
            "plastic_white",
            flat("satin white plastic", [0.8, 0.8, 0.78], 0.38, 0.0),
            0.0,
        ),
        (
            "rubber",
            flat("black rubber", [0.025, 0.025, 0.025], 0.85, 0.0),
            0.0,
        ),
        ("denim", Material::textured("denim", "denim_fabric"), 0.0),
        (
            "leather",
            Material::textured("leather", "fabric_leather_02"),
            0.0,
        ),
        ("velvet", flat("velvet", [0.18, 0.02, 0.06], 0.9, 0.0), 0.0),
        ("skin", flat("skin-like", [0.78, 0.53, 0.42], 0.5, 0.0), 0.0),
        ("marble", Material::textured("marble", "marble_01"), 0.0),
        ("wood", Material::textured("wood", "wood_table_001"), 0.0),
        (
            "glass",
            Material::glass("clear glass", [0.92, 0.95, 0.95], 0.12),
            0.0,
        ),
        (
            "glass_tinted",
            Material::glass("tinted glass", [0.2, 0.55, 0.45], 0.35),
            0.0,
        ),
        (
            "emissive",
            Material::emitter("emissive", [1.0, 0.45, 0.12]),
            2400.0,
        ),
    ]
}

/// Builds the showcase project.
pub fn build(ctx: &Ctx) -> Result<()> {
    clean(
        ctx,
        &[
            "levels",
            "anim",
            "prefabs",
            "materials",
            "assets/kit",
            ".scratch",
        ],
    )?;
    let mut models = Models::default();
    for (name, asset) in [
        ("flight_helmet", "khronos_flight_helmet"),
        ("scifi_helmet", "khronos_scifi_helmet"),
        ("antique_camera", "khronos_antique_camera"),
        ("water_bottle", "khronos_water_bottle"),
        ("camera", "camera_01"),
        ("lantern", "lantern_01"),
        ("binoculars", "binoculars"),
        ("blowtorch", "brass_blowtorch"),
        ("boombox", "boombox"),
        ("radio", "vintage_radio_transceiver"),
        ("gas_mask", "old_gas_mask"),
    ] {
        ctx.third_party(&mut models, name, asset)?;
    }
    super::outpost::human(ctx, &mut models)?;
    let lock = &ctx.lock;
    let root = &ctx.root;
    // Stage pieces.
    let mut stage = Kit::new(vec![
        Material::textured("floor", "concrete_floor_02").tinted([0.32, 0.31, 0.3]),
        Material::flat("turntable", [0.06, 0.06, 0.065], 0.35, 0.0),
        Material::flat("plinth", [0.72, 0.71, 0.69], 0.55, 0.0),
    ]);
    stage.cuboid(
        0,
        Vec3::new(30.0, 0.2, 30.0),
        Mat4::from_translation(Vec3::new(0.0, -0.1, 0.0)),
        crate::kit::meters(lock, "concrete_floor_02"),
    );
    stage.cylinder(1, 1.6, 0.12, 64, Mat4::IDENTITY, Vec2::ONE);
    let path = stage.write(root, lock, "stage")?;
    ctx.model(&mut models, "stage", &path, "as_kitstage")?;
    let mut plinth = Kit::new(vec![Material::flat(
        "plinth",
        [0.72, 0.71, 0.69],
        0.55,
        0.0,
    )]);
    plinth.cuboid(
        0,
        Vec3::new(0.6, 1.0, 0.6),
        Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0)),
        Vec2::ONE,
    );
    let path = plinth.write(root, lock, "plinth")?;
    ctx.model(&mut models, "plinth", &path, "as_kitplinth")?;
    let mut emitters = Vec::new();
    for (name, material, radiance) in balls() {
        let mut kit = Kit::new(vec![material]);
        kit.sphere(0, Vec3::new(0.0, 0.3, 0.0), 0.3, 32, Vec2::new(2.0, 1.0));
        let kit_name = format!("ball_{name}");
        let path = kit.write(root, lock, &kit_name)?;
        ctx.model(
            &mut models,
            &kit_name,
            &path,
            &format!("as_kit{}", super::place::short(&kit_name)),
        )?;
        emitters.push((kit_name, radiance));
    }
    let human = lock.get("m2m_human").expect("m2m_human pinned");
    anim::locomotion(
        ctx,
        ANIMATION.0,
        ANIMATION.1,
        "an_locomotion",
        &human.dir(root).join("human-base-animations.glb"),
        "Idle_A",
        &[
            anim::Gait {
                clip: "Walk",
                speed: 1.5,
            },
            anim::Gait {
                clip: "Jog",
                speed: 3.2,
            },
            anim::Gait {
                clip: "Sprint",
                speed: 5.5,
            },
        ],
    )?;

    ctx.new_document(SCENE.0, "scene", SCENE.1)?;
    let mut s = Scene::new(&models);
    s.entity(
        "e_env",
        "Studio",
        json!({
            "Environment": {"sky": [0.5, 0.48, 0.45, 1], "horizon": [0.6, 0.55, 0.5, 1], "ground": [0.25, 0.22, 0.2, 1], "intensity": 900, "exposure": 0.002},
            // Product lighting: Neutral keeps material colours accurate.
            "PostProcess": {"toneMapping": "neutral", "bloom": 0.04}
        }),
    );
    s.commands.extend(light(
        "e_key",
        "Key light",
        Vec3::new(0.0, 10.0, 0.0),
        json!({"kind": "directional", "color": [1.0, 0.95, 0.88, 1], "intensity": 2200}),
        Some(Vec3::new(0.45, -0.75, -0.5)),
    ));
    for (id, p, color, lumens) in [
        (
            "e_rim",
            Vec3::new(-3.5, 3.2, 3.0),
            [0.75, 0.85, 1.0],
            9000.0,
        ),
        ("e_fill", Vec3::new(4.0, 2.5, -3.5), [1.0, 0.9, 0.8], 4000.0),
    ] {
        s.commands.extend(light(
            id,
            "Studio spot",
            p,
            json!({"kind": "spot", "color": [color[0], color[1], color[2], 1], "intensity": lumens, "range": 14, "innerAngle": 25, "outerAngle": 45}),
            Some(Vec3::new(0.0, 1.0, 0.0) - p),
        ));
    }
    s.prop(
        Prop::new("e_stage", "Stage", "stage", Vec3::ZERO, 0.0)
            .at_origin()
            .decor(),
    );
    s.entity(
        "e_stagec",
        "Floor collider",
        json!({"Transform": {"position": [0.0, -0.1, 0.0]}, "RigidBody": {}, "Collider": {"halfExtents": [15.0, 0.1, 15.0]}}),
    );
    // The turntable character and a jogger looping around the stage.
    s.entity(
        "e_model",
        "Mannequin (turntable)",
        json!({
            "Transform": transform(Vec3::new(0.0, 0.12, 0.0), 200.0, 1.0),
            "MeshRenderer": models.get("human").renderer,
            "Animator": {"controller": format!("@{}#an_locomotion", ANIMATION.0)},
            "CharacterController": {},
            "PlayerControl": {"camera": "e_camera", "walkSpeed": 1.5, "runSpeed": 5.0, "yaw": 200},
        }),
    );
    s.entity(
        "e_jogger",
        "Mannequin (jogging)",
        json!({
        "Transform": transform(Vec3::new(-3.5, 0.0, 2.5), 90.0, 1.0),
        "MeshRenderer": models.get("human").renderer,
        "Animator": {"controller": format!("@{}#an_locomotion", ANIMATION.0)},
        "Wanderer": {"min": [-4.5, 0.0, -1.0], "max": [4.5, 0.0, 4.5], "pauseTicks": 20},
        "NavAgent": {"height": 1.8, "maxSpeed": 3.0},
        "RigidBody": {"motion": "kinematic"},
        "Collider": {"kind": "capsule", "radius": 0.3, "halfHeight": 0.6},
        }),
    );
    s.entity(
        "e_navsurface",
        "Stage navigation",
        json!({"Transform": {}, "NavMeshSurface": {"min": [-7.0, -1.0, -7.0], "max": [7.0, 3.0, 7.0]}}),
    );
    s.entity(
        "e_camera",
        "Camera",
        json!({"Transform": {"position":[0,2.2,-7.5]}, "Camera": {"fov": 45}}),
    );
    s.commands.push((
        "transform.lookAt",
        json!({"entity":"e_camera","point":[0,2.04,-6.5]}),
    ));
    // Hero props on plinths in an arc in front.
    for (i, (name, scale, yaw)) in [
        ("flight_helmet", 1.0, 200.0),
        ("scifi_helmet", 0.2, 160.0),
        ("antique_camera", 0.11, 210.0),
        ("water_bottle", 1.6, 180.0),
        ("camera", 2.2, 150.0),
        ("lantern", 2.2, 190.0),
        ("binoculars", 2.0, 140.0),
        ("blowtorch", 2.0, 200.0),
        ("boombox", 1.0, 170.0),
        ("radio", 1.0, 180.0),
        ("gas_mask", 1.4, 160.0),
    ]
    .into_iter()
    .enumerate()
    {
        // A wide arc of plinths turned toward the turntable.
        let a = (-75.0 + i as f32 * 15.0).to_radians();
        let base = Vec3::new(a.sin() * 4.4, 0.0, -a.cos() * 4.4);
        s.prop(Prop::new(
            &format!("e_plinth{i}"),
            "Plinth",
            "plinth",
            base,
            -a.to_degrees(),
        ));
        s.prop(
            Prop::new(&format!("e_hero{i}"), name, name, base + Vec3::Y * 1.0, yaw)
                .scaled(scale)
                .decor(),
        );
    }
    // Material grid on low plinths behind the turntable: 6 x 3.
    for (i, (kit_name, radiance)) in emitters.iter().enumerate() {
        let (col, row) = ((i % 6) as f32, (i / 6) as f32);
        let p = Vec3::new((col - 2.5) * 0.9, 0.0, 3.0 + row * 0.9);
        let mut prop = Prop::new(&format!("e_ball{i}"), kit_name, kit_name, p, 0.0).decor();
        if *radiance > 0.0 {
            prop = prop.renderer(json!({"emissive": [radiance, radiance, radiance]}));
        }
        s.prop(prop);
    }
    let calls = std::mem::take(&mut s.commands);
    ctx.apply(
        SCENE.0,
        "Showcase stage: turntable character, hero props, material grid",
        calls,
    )?;
    ctx.set_main(SCENE.0, SCENE.1)?;
    ctx.check()?;
    Ok(())
}
