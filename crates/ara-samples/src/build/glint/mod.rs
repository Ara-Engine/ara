//! "Glint": a rolling-ball game, one course ("Skyward"): a floating route of decks, a beam, a
//! banked turn, sliders, spinner bars and a cannon that fires the ball across a 76 m gap to the
//! goal, over open sky, in bright low sun. The ball is a dynamic sphere steered by C#
//! (`samples/glint/content/scripts/Glint`).
//!
//! Everything comes from `course.rs` (the route as data). Two scenes share one art pass:
//! `levels/glint.scene` (the game: colliders, `Surface` boxes, movers, the ball, the director)
//! and `levels/menu.scene` (the native menu's backdrop: the same art, no gameplay components, a
//! fly-over camera marked by two points). The UI's bindings and resources are generated here too.
mod course;
mod icons;
mod kits;
mod ui;

use super::{
    Ctx, clean,
    outpost::light,
    place::{Models, Prop, Scene, round3, short},
};
use anyhow::Result;
use course::Course;
use glam::{Quat, Vec3};
use serde_json::{Value, json};

/// Gameplay scene.
pub const SCENE: (&str, &str) = ("levels/glint.scene", "sc_glint000001");
/// The menu backdrop the native shell requires at this path.
const MENU: (&str, &str) = ("levels/menu.scene", "sc_glint000002");
/// Fixed exposure (EV100): no auto-exposure anywhere.
const EV100: f32 = 13.5;
/// Direction the sun's light travels: low and warm, from the right and ahead of the start.
const SUN: Vec3 = Vec3::new(-0.62, -0.38, -0.68);
/// The spinner bar's length and thickness (m).
pub const BAR: (f32, f32) = (6.4, 0.32);
/// Gold, silver and bronze times in seconds (the UI prints the same numbers).
pub const MEDALS: [u32; 3] = [60, 75, 100];

fn euler(rot: Quat) -> [f32; 3] {
    let (x, y, z) = rot.to_euler(glam::EulerRot::XYZ);
    [x.to_degrees(), y.to_degrees(), z.to_degrees()]
}

fn lamp(
    id: &str,
    name: &str,
    at: Vec3,
    color: [f32; 3],
    lumens: f32,
    radius: f32,
    range: f32,
) -> Vec<(&'static str, Value)> {
    light(
        id,
        name,
        at,
        json!({"kind": "point", "color": [color[0], color[1], color[2], 1], "intensity": lumens,
            "units": "lumens", "range": range, "sourceRadius": radius}),
        None,
    )
}

/// Registers the kits as models.
fn models(ctx: &Ctx, c: &Course) -> Result<Models> {
    let mut models = Models::default();
    let root = &ctx.root;
    let lock = &ctx.lock;
    let slider = c
        .sliders
        .first()
        .map_or(Vec3::new(2.2, 0.4, 2.2), |s| s.half);
    let cannon = c.cannon.as_ref().expect("the course has a cannon");
    for (name, path) in [
        ("glint_course", kits::course(root, lock, c)?),
        ("glint_mover", kits::mover(root, lock, slider)?),
        ("glint_bar", kits::bar(root, lock)?),
        ("glint_orb", kits::orb(root, lock)?),
        ("glint_gate", kits::gate(root, lock)?),
        (
            "glint_cannon",
            kits::cannon(root, lock, cannon.muzzle - cannon.breech)?,
        ),
        ("glint_goal", kits::goal(root, lock)?),
        ("glint_blob", blob(root, lock)?),
    ] {
        ctx.model(&mut models, name, &path, &format!("as_kit{}", short(name)))?;
    }
    Ok(models)
}

/// The ball's contact shadow: three stacked plain-alpha discs (`BLEND` is coverage, not glass),
/// widest and faintest at the bottom, so the edge falls off softly.
fn blob(root: &std::path::Path, lock: &crate::lock::Lock) -> Result<String> {
    use crate::kit::{Kit, Material};
    use glam::{Mat4, Vec2};
    let film = |name: &str, alpha: f32| Material {
        color: [0.03, 0.05, 0.12, alpha],
        blend: true,
        ..Material::flat(name, [0.03, 0.05, 0.12], 1.0, 0.0)
    };
    let mut k = Kit::new(vec![
        film("shadow outer", 0.14),
        film("shadow mid", 0.2),
        film("shadow core", 0.3),
    ]);
    k.cylinder(0, 0.5, 0.004, 40, Mat4::IDENTITY, Vec2::ONE);
    k.cylinder(
        1,
        0.36,
        0.004,
        40,
        Mat4::from_translation(Vec3::new(0.0, 0.004, 0.0)),
        Vec2::ONE,
    );
    k.cylinder(
        2,
        0.2,
        0.004,
        40,
        Mat4::from_translation(Vec3::new(0.0, 0.008, 0.0)),
        Vec2::ONE,
    );
    k.write(root, lock, "glint_blob")
}

/// Sky, sun, haze, lights and the visual course: everything both scenes share. With
/// `play` the gameplay components the director and the cannon read are added too.
fn art(s: &mut Scene, c: &Course, play: bool) {
    let models = s.models;
    s.entity(
        "e_env",
        "Clear sky and fixed exposure",
        json!({
            "Environment": {"sky": [0.05, 0.2, 0.7, 1], "horizon": [0.6, 0.78, 1.0, 1], "skyFalloff": 2.6,
                "glow": [0.9, 0.5, 0.25, 1], "glowFalloff": 14, "ground": [1.5, 1.62, 1.85, 1],
                "intensity": 7000, "skyLight": 1},
            // A bright, faintly cool haze: the drop below the course fades into pale haze.
            "HeightFog": {"density": 0.0004, "falloff": 60, "albedo": [0.6, 0.76, 1.0, 1], "anisotropy": 0.4},
            "PostProcessVolume": {"overrides": [
                {"exposure": "manual", "ev100": EV100},
                {"module": "toneMapping", "toneMapping": "neutral"},
                {"module": "bloom", "bloom": 0.12},
            ]},
        }),
    );
    s.commands.extend(light(
        "e_sun",
        "Sun",
        Vec3::new(0.0, 60.0, 30.0),
        json!({"kind": "directional", "color": [1.0, 0.86, 0.68, 1], "intensity": 95_000, "sourceAngle": 0.5357}),
        Some(SUN),
    ));
    for (i, (at, _)) in c.checkpoints.iter().enumerate() {
        s.commands.extend(lamp(
            &format!("e_lcp{i}"),
            "Gate light",
            *at + Vec3::new(0.0, 3.0, 0.0),
            [1.0, 0.45, 0.25],
            90_000.0,
            0.3,
            14.0,
        ));
    }
    s.commands.extend(lamp(
        "e_lgoal",
        "Goal light",
        c.goal + Vec3::new(0.0, 6.0, 0.0),
        [1.0, 0.55, 0.3],
        220_000.0,
        0.6,
        26.0,
    ));
    s.prop(
        Prop::new("e_course", "Course", "glint_course", Vec3::ZERO, 0.0)
            .at_origin()
            .decor(),
    );
    // Checkpoint gates, the cannon, the goal arch.
    for (i, (at, heading)) in c.checkpoints.iter().enumerate() {
        s.prop(
            Prop::new(
                &format!("e_gate{i}"),
                "Checkpoint gate",
                "glint_gate",
                *at,
                *heading,
            )
            .at_origin()
            .decor()
            .renderer(json!({"emissive": [9000, 9000, 9000]})),
        );
        if play {
            s.commands.push((
                "component.add",
                json!({"entity": format!("e_gate{i}"), "component": "Checkpoint", "value": {"index": i + 1, "radius": 3.2}}),
            ));
        }
    }
    let cannon = c.cannon.as_ref().expect("cannon");
    s.prop(
        Prop::new(
            "e_cannon",
            "Cannon",
            "glint_cannon",
            cannon.breech - Vec3::new(0.0, 0.5, 0.0),
            cannon.heading,
        )
        .at_origin()
        .decor()
        .renderer(json!({"emissive": [12000, 12000, 12000]})),
    );
    s.prop(
        Prop::new("e_goal", "Goal arch", "glint_goal", c.goal, cannon.heading)
            .at_origin()
            .decor()
            .renderer(json!({"emissive": [16000, 16000, 16000]})),
    );
    if play {
        s.commands.push((
            "component.add",
            json!({"entity": "e_cannon", "component": "Cannon", "value": {
                "breech": cannon.breech.to_array(), "muzzle": cannon.muzzle.to_array(), "launch": cannon.launch.to_array()}}),
        ));
        s.commands.push((
            "component.add",
            json!({"entity": "e_cannon", "component": "CannonController", "value": {}}),
        ));
        s.commands.push((
            "component.add",
            json!({"entity": "e_goal", "component": "Goal", "value": {"radius": 5.0}}),
        ));
    }
    let orb = models.get("glint_orb").renderer["model"].clone();
    for (i, p) in c.pickups.iter().enumerate() {
        let id = format!("e_orb{i}");
        let mut components = json!({
            "Transform": {"position": round3(*p)},
            "MeshRenderer": {"model": orb, "emissive": [12000, 12000, 12000]},
        });
        if play {
            components["Pickup"] = json!({"size": 1.0});
            components["Spinner"] = json!({"degreesPerSecond": 110, "startYaw": (i * 37 % 360)});
        }
        s.entity(&id, "Orb", components);
    }
}

/// The gameplay scene.
fn game(ctx: &Ctx, c: &Course) -> Result<()> {
    ctx.new_document(SCENE.0, "scene", SCENE.1)?;
    let models = models(ctx, c)?;
    let mut s = Scene::new(&models);
    art(&mut s, c, true);
    // Invisible collision and walkable boxes: the course kit draws the pieces.
    for p in &c.pieces {
        let half = p.half.to_array();
        s.entity(
            &p.id,
            p.name,
            json!({
                "Transform": {"position": round3(p.center), "rotation": round3(Vec3::from_array(p.euler()))},
                "RigidBody": {"motion": "static"},
                "Collider": {"halfExtents": half},
                "Surface": {"half": half},
            }),
        );
    }
    // The launch deck's glass guard is solid: back and both sides.
    let first = &c.pieces[0];
    for (i, (at, half)) in [
        (
            Vec3::new(0.0, first.half.y + 0.5, -first.half.z + 0.1),
            Vec3::new(first.half.x, 0.5, 0.06),
        ),
        (
            Vec3::new(-first.half.x + 0.1, first.half.y + 0.5, 0.0),
            Vec3::new(0.06, 0.5, first.half.z),
        ),
        (
            Vec3::new(first.half.x - 0.1, first.half.y + 0.5, 0.0),
            Vec3::new(0.06, 0.5, first.half.z),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        s.entity(
            &format!("e_guard{i}"),
            "Guard glass",
            json!({"Transform": {"position": round3(first.center + first.rot * at), "rotation": round3(Vec3::from_array(first.euler()))},
                "RigidBody": {"motion": "static"}, "Collider": {"halfExtents": half.to_array()}}),
        );
    }
    // Sliders and bars.
    let mover = models.get("glint_mover").renderer.clone();
    for m in &c.sliders {
        let half = m.half.to_array();
        s.entity(
            &m.id,
            "Moving platform",
            json!({
                "Transform": {"position": round3(m.origin), "rotation": round3(Vec3::from_array(euler(m.rot)))},
                "MeshRenderer": mover,
                "RigidBody": {"motion": "kinematic"},
                "Collider": {"halfExtents": half},
                "Surface": {"half": half},
                "Mover": {"origin": m.origin.to_array(), "travel": m.travel.to_array(), "periodTicks": m.period, "phaseTicks": m.phase},
            }),
        );
    }
    let bar = models.get("glint_bar").renderer.clone();
    for b in &c.bars {
        s.entity(
            &b.id,
            "Spinner bar",
            json!({
                "Transform": {"position": round3(b.pivot), "rotation": [0, b.yaw, 0]},
                "MeshRenderer": bar,
                "RigidBody": {"motion": "kinematic"},
                "Collider": {"halfExtents": [BAR.0 * 0.5, BAR.1 * 0.5, BAR.1 * 0.5]},
                "Spinner": {"degreesPerSecond": b.rate, "startYaw": b.yaw},
            }),
        );
    }
    // The route the camera follows.
    for (i, w) in c.waypoints.iter().enumerate() {
        s.entity(
            &format!("e_wp{i:02}"),
            "Waypoint",
            json!({"Transform": {"position": round3(*w)}, "Waypoint": {"index": i}}),
        );
    }
    // The ball: a chrome dynamic sphere driven by BallController; PlayerControl gives it an
    // input slot (its follow camera's own orbit is switched off: CourseCamera steers the yaw).
    s.entity(
        "e_ball",
        "Ball",
        json!({
            "Transform": {"position": c.start.to_array()},
            "MeshRenderer": ball_look(),
            "RigidBody": {"motion": "dynamic", "continuous": true, "allowSleep": false},
            "Collider": {"kind": "sphere", "radius": 0.5, "friction": 0.8},
            "PlayerControl": {"camera": "e_camera"},
            "Ball": {"spawn": c.start.to_array()},
            "BallController": {},
        }),
    );
    s.entity(
        "e_shadow",
        "Ball contact shadow",
        json!({
            "Transform": {"position": [0, 0.02, -2.0], "scale": [1.3, 1, 1.3]},
            "MeshRenderer": models.get("glint_blob").renderer,
            "BallShadow": {},
        }),
    );
    s.entity(
        "e_fx",
        "Ball sparks",
        json!({"Transform": {"position": c.start.to_array()}, "VisualEffect": {"effect": "sparks", "scale": 0.8, "intensity": 0.0}, "BallFx": {}}),
    );
    s.entity(
        "e_camera",
        "Camera",
        json!({
            "Transform": {"position": [0, 4.0, -12.0]},
            "Camera": {"fov": 56},
            "FollowCamera": {"target": "e_ball", "distance": 5.4, "height": 2.2, "pitch": 18, "sensitivity": 0},
            "CourseCamera": {},
        }),
    );
    // An establishing shot for previews; a higher priority number keeps the follow camera active.
    s.entity(
        "e_camwide",
        "Establishing camera",
        json!({"Transform": {"position": [0.0, 7.0, -17.0]}, "Camera": {"fov": 62, "priority": 10}}),
    );
    s.commands.push((
        "transform.lookAt",
        json!({"entity": "e_camwide", "point": [-30.0, 1.0, 60.0]}),
    ));
    // Review cameras: the whole route from the side, the turn, the cannon's shot, the goal.
    let (cannon, goal) = (c.cannon.as_ref().expect("cannon"), c.goal);
    let centre = (c.start + goal) * 0.5;
    for (id, name, at, look, fov) in [
        (
            "e_camover",
            "Overview",
            centre + Vec3::new(260.0, 190.0, -120.0),
            centre,
            58.0,
        ),
        (
            "e_camturn",
            "Turn view",
            c.waypoints[c.waypoints.len() / 2] + Vec3::new(30.0, 22.0, -40.0),
            c.waypoints[c.waypoints.len() / 2],
            60.0,
        ),
        (
            "e_camcannon",
            "Cannon view",
            cannon.breech + Vec3::new(18.0, 6.0, -12.0),
            cannon.muzzle + Vec3::new(-50.0, 6.0, 0.0),
            60.0,
        ),
        (
            "e_camgoal",
            "Goal view",
            goal + Vec3::new(34.0, 8.0, -24.0),
            goal + Vec3::new(0.0, 4.0, 0.0),
            60.0,
        ),
    ] {
        s.entity(
            id,
            name,
            json!({"Transform": {"position": round3(at)}, "Camera": {"fov": fov, "priority": 10}}),
        );
        s.commands.push((
            "transform.lookAt",
            json!({"entity": id, "point": round3(look)}),
        ));
    }
    s.entity(
        "e_run",
        "Run",
        json!({"GlintRun": {"start": c.start.to_array(), "goldTicks": MEDALS[0] * 60, "silverTicks": MEDALS[1] * 60, "bronzeTicks": MEDALS[2] * 60,
            "orbTotal": c.pickups.len()}}),
    );
    let calls = std::mem::take(&mut s.commands);
    ctx.apply(
        SCENE.0,
        "Glint: Skyward, the ball, the course and the run director",
        calls,
    )?;
    ctx.set_main(SCENE.0, SCENE.1)
}

/// The chrome ball's renderer fields.
fn ball_look() -> Value {
    json!({"shape": "sphere", "size": [1, 1, 1], "color": [0.97, 0.97, 0.98, 1], "roughness": 0.03, "metallic": 1})
}

/// The menu backdrop: the same art, the ball resting at the start, and the fly-over eye and
/// target the shell's camera uses.
fn menu(ctx: &Ctx, c: &Course) -> Result<()> {
    ctx.new_document(MENU.0, "scene", MENU.1)?;
    let models = models(ctx, c)?;
    let mut s = Scene::new(&models);
    art(&mut s, c, false);
    let mover = models.get("glint_mover").renderer.clone();
    for m in &c.sliders {
        s.entity(
            &format!("{}m", m.id),
            "Moving platform",
            json!({"Transform": {"position": round3(m.origin + m.travel * 0.4), "rotation": round3(Vec3::from_array(euler(m.rot)))}, "MeshRenderer": mover}),
        );
    }
    let bar = models.get("glint_bar").renderer.clone();
    for b in &c.bars {
        s.entity(
            &format!("{}m", b.id),
            "Spinner bar",
            json!({"Transform": {"position": round3(b.pivot), "rotation": [0, b.yaw + 35.0, 0]}, "MeshRenderer": bar}),
        );
    }
    s.entity(
        "e_menuball",
        "Ball",
        json!({"Transform": {"position": [0.0, 0.5, 9.0]}, "MeshRenderer": ball_look()}),
    );
    // The title view: low and a little to the side of the launch deck, the long straight ahead.
    s.entity(
        "e_gamemenueye",
        "Menu vista camera",
        json!({"Transform": {"position": [-7.0, 3.4, -16.0]}}),
    );
    s.entity(
        "e_gamemenutarget",
        "Menu vista focus",
        json!({"Transform": {"position": [3.0, 2.0, 40.0]}}),
    );
    let calls = std::mem::take(&mut s.commands);
    ctx.apply(MENU.0, "Glint: native menu backdrop", calls)
}

/// Builds the Glint project: icons, UI data, the game scene and the menu backdrop.
pub fn build(ctx: &Ctx) -> Result<()> {
    clean(ctx, &["levels", "kit"])?;
    let c = course::build();
    icons::write(&ctx.path("ui/icons"))?;
    ui::write(&ctx.path("ui"), c.pickups.len(), c.checkpoints.len())?;
    game(ctx, &c)?;
    menu(ctx, &c)?;
    ctx.check()
}
