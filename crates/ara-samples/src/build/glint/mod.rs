//! "Glint": a small rolling-ball game, one level so far ("Foundry Steps"): a catwalk of
//! slabs, a sliding platform, two spinner bars, two checkpoints and a goal, floating over a
//! foundry yard at dusk. The ball is a dynamic sphere steered by C#
//! (`samples/glint/content/scripts/Glint`).
//!
//! Two scenes share one art pass: `levels/glint.scene` (the game: colliders, `Surface`
//! faces, movers, the ball, the run director) and `levels/menu.scene` (the native menu's
//! backdrop: the same art, no gameplay components, a fly-over camera marked by two points).
//! Walkable faces carry a `Surface` component with the same half extents as their collider,
//! because scripts have no contact events or ray casts to ask the physics engine.
//! The look reuses Atelier's vocabulary: a numeric gradient sky, one sun, a fixed exposure,
//! plain PBR materials from CC0 sets, lights with explicit source radii, one accent colour.
mod kits;

use super::{
    Ctx, clean,
    outpost::light,
    place::{Models, Prop, Scene, round3, short},
};
use anyhow::Result;
use glam::Vec3;
use kits::{BAR, CHIMNEYS, FURNACES, MOVER_SIZE, SLAB, SLABS, YARD_FLOOR};
use serde_json::{Value, json};

/// Gameplay scene.
pub const SCENE: (&str, &str) = ("levels/glint.scene", "sc_glint000001");
/// The menu backdrop the native shell requires at this path.
const MENU: (&str, &str) = ("levels/menu.scene", "sc_glint000002");
/// Fixed exposure (EV100): no auto-exposure anywhere.
const EV100: f32 = 11.0;
/// Where the ball starts.
const START: Vec3 = Vec3::new(0.0, 0.6, -2.0);
/// Direction the sun's light travels: about 13 degrees above the horizon, raking across the
/// course from its right and ahead, so slab edges and bars throw long shadows left and back.
const SUN: Vec3 = Vec3::new(-0.85, -0.22, -0.47);

/// The moving platform's path: centre, travel, period in ticks.
const MOVER_ORIGIN: Vec3 = Vec3::new(0.0, 0.8 - SLAB * 0.5, 32.5);
const MOVER_TRAVEL: Vec3 = Vec3::new(3.0, 0.0, 0.0);
const MOVER_PERIOD: u32 = 360;

/// Spinner bars over the beam: id, pivot Z, start yaw, degrees a second.
const BARS: [(&str, f32, f32, f32); 2] = [("e_b1", 48.0, 0.0, 80.0), ("e_b2", 58.0, 90.0, -95.0)];

fn lamp(id: &str, name: &str, at: Vec3, color: [f32; 3], lumens: f32, radius: f32, range: f32) -> Vec<(&'static str, Value)> {
    light(
        id,
        name,
        at,
        json!({"kind": "point", "color": [color[0], color[1], color[2], 1], "intensity": lumens,
            "units": "lumens", "range": range, "sourceRadius": radius}),
        None,
    )
}

/// Registers the kits as models and returns them.
fn models(ctx: &Ctx) -> Result<Models> {
    let mut models = Models::default();
    let root = &ctx.root;
    let lock = &ctx.lock;
    for (name, path) in [
        ("glint_course", kits::course(root, lock)?),
        ("glint_mover", kits::mover(root, lock)?),
        ("glint_bar", kits::bar(root, lock)?),
        ("glint_pad", kits::pad(root, lock)?),
        ("glint_goal", kits::goal(root, lock)?),
        ("glint_yard", kits::yard(root, lock)?),
    ] {
        ctx.model(&mut models, name, &path, &format!("as_kit{}", short(name)))?;
    }
    Ok(models)
}

/// Sky, sun, lights, effects and the visual course: everything both scenes share. `play`
/// adds nothing here; gameplay components are added by the callers around these entities.
fn art(s: &mut Scene) {
    s.entity(
        "e_env",
        "Dusk sky and fixed exposure",
        json!({
            "Environment": {"sky": [0.03, 0.06, 0.2, 1], "horizon": [0.8, 0.4, 0.2, 1], "skyFalloff": 4.5,
                "glow": [0.7, 0.2, 0.05, 1], "glowFalloff": 14, "ground": [0.03, 0.025, 0.025, 1],
                "intensity": 1200, "skyLight": 1},
            // Dusk haze: warm height fog thickens toward the yard far below.
            "HeightFog": {"density": 0.0001, "falloff": 30, "albedo": [1.0, 0.72, 0.52, 1], "anisotropy": 0.6},
            "PostProcessVolume": {"overrides": [
                {"exposure": "manual", "ev100": EV100},
                {"module": "toneMapping", "toneMapping": "neutral"},
            ]},
        }),
    );
    s.commands.extend(light(
        "e_sun",
        "Sun",
        Vec3::new(0.0, 40.0, 30.0),
        json!({"kind": "directional", "color": [1.0, 0.7, 0.48, 1], "intensity": 25_000, "sourceAngle": 0.5357}),
        Some(SUN),
    ));
    // Furnace glow at the chimneys' feet, far below; checkpoint and goal lights on the course.
    // The two big furnaces beside the route, mouths facing the start: the key warm sources.
    for (i, (x, z)) in FURNACES.iter().enumerate() {
        s.commands.extend(lamp(
            &format!("e_lmouth{i}"),
            "Furnace mouth",
            Vec3::new(*x, YARD_FLOOR + 3.5, z - 11.0),
            [1.0, 0.45, 0.12],
            15_000_000.0,
            2.0,
            90.0,
        ));
    }
    for (i, (x, z, _, _)) in CHIMNEYS.iter().take(4).enumerate() {
        s.commands.extend(lamp(
            &format!("e_lfurn{i}"),
            "Furnace glow",
            Vec3::new(*x, YARD_FLOOR + 3.0, z - 4.0),
            [1.0, 0.4, 0.1],
            6_000_000.0,
            1.6,
            90.0,
        ));
    }
    for (i, z) in [40.0f32, 66.0].into_iter().enumerate() {
        s.commands.extend(lamp(&format!("e_lcp{i}"), "Checkpoint light", Vec3::new(0.0, 2.4, z), [1.0, 0.62, 0.22], 150_000.0, 0.3, 18.0));
    }
    s.commands.extend(lamp("e_lgoal", "Goal light", Vec3::new(0.0, 0.0, 94.0), [0.3, 0.85, 1.0], 300_000.0, 0.5, 30.0));
    s.prop(Prop::new("e_course", "Catwalk", "glint_course", Vec3::ZERO, 0.0).at_origin().decor());
    s.prop(
        Prop::new("e_yard", "Foundry yard", "glint_yard", Vec3::ZERO, 0.0)
            .at_origin()
            .decor()
            .renderer(json!({"emissive": [6000, 6000, 6000]})),
    );
    // Steam and sparks: engine-side extras (they do not export).
    for (i, (x, z, _, h)) in CHIMNEYS.iter().enumerate() {
        s.entity(
            &format!("e_smoke{i}"),
            "Chimney smoke",
            json!({"Transform": {"position": [x, YARD_FLOOR + h, z]}, "VisualEffect": {"effect": "smoke", "scale": 5, "intensity": 0.6}}),
        );
    }
    s.entity(
        "e_sparks",
        "Goal sparks",
        json!({"Transform": {"position": [0, -1.5, 94]}, "VisualEffect": {"effect": "sparks", "scale": 1.2}}),
    );
    s.entity(
        "e_swirl",
        "Goal swirl",
        json!({"Transform": {"position": [0, -1.5, 94]}, "VisualEffect": {"effect": "magic_swirl", "scale": 1.6, "intensity": 0.8}}),
    );
}

/// The gameplay scene.
fn game(ctx: &Ctx) -> Result<()> {
    ctx.new_document(SCENE.0, "scene", SCENE.1)?;
    let models = models(ctx)?;
    let mut s = Scene::new(&models);
    art(&mut s);
    let mover = &models.get("glint_mover").renderer;
    let bar = &models.get("glint_bar").renderer;
    // Invisible collision and walkable faces: the course kit draws the slabs.
    for slab in &SLABS {
        let half = [slab.size.0 * 0.5, SLAB * 0.5, slab.size.1 * 0.5];
        s.entity(
            slab.id,
            slab.name,
            json!({
                "Transform": {"position": round3(Vec3::new(slab.at.0, slab.top - SLAB * 0.5, slab.at.1))},
                "RigidBody": {"motion": "static"},
                "Collider": {"halfExtents": half},
                "Surface": {"half": half},
            }),
        );
    }
    let half = [MOVER_SIZE.0 * 0.5, SLAB * 0.5, MOVER_SIZE.1 * 0.5];
    s.entity(
        "e_m1",
        "Moving platform",
        json!({
            "Transform": {"position": round3(MOVER_ORIGIN)},
            "MeshRenderer": mover,
            "RigidBody": {"motion": "kinematic"},
            "Collider": {"halfExtents": half},
            "Surface": {"half": half},
            "Mover": {"origin": MOVER_ORIGIN.to_array(), "travel": MOVER_TRAVEL.to_array(), "periodTicks": MOVER_PERIOD},
        }),
    );
    for (id, z, yaw, rate) in BARS {
        s.entity(
            id,
            "Spinner bar",
            json!({
                "Transform": {"position": [0, 1.35, z], "rotation": [0, yaw, 0]},
                "MeshRenderer": bar,
                "RigidBody": {"motion": "kinematic"},
                "Collider": {"halfExtents": [BAR.0 * 0.5, BAR.1 * 0.5, BAR.1 * 0.5]},
                "Spinner": {"degreesPerSecond": rate, "startYaw": yaw},
            }),
        );
    }
    // The dock's railing is solid: back and both sides, the runway side open.
    for (id, at, half) in [
        ("e_rail0", [0.0, 0.5, -4.93], [4.0, 0.5, 0.07]),
        ("e_rail1", [-3.93, 0.5, 0.0], [0.07, 0.5, 5.0]),
        ("e_rail2", [3.93, 0.5, 0.0], [0.07, 0.5, 5.0]),
    ] {
        s.entity(id, "Dock railing", json!({"Transform": {"position": at}, "RigidBody": {"motion": "static"}, "Collider": {"halfExtents": half}}));
    }
    pads(&mut s, &models, true);
    // The ball: a chrome dynamic sphere driven by BallController; PlayerControl gives it an
    // input slot and its follow camera's orbit.
    s.entity(
        "e_ball",
        "Ball",
        json!({
            "Transform": {"position": START.to_array()},
            "MeshRenderer": ball_look(),
            "RigidBody": {"motion": "dynamic", "continuous": true, "allowSleep": false},
            "Collider": {"kind": "sphere", "radius": 0.5, "friction": 0.8},
            "PlayerControl": {"camera": "e_camera"},
            "Ball": {"spawn": START.to_array()},
            "BallController": {},
        }),
    );
    s.entity(
        "e_camera",
        "Camera",
        json!({
            "Transform": {"position": [0, 3.5, -9]},
            "Camera": {"fov": 60},
            "FollowCamera": {"target": "e_ball", "distance": 7.5, "height": 2.6, "pitch": 18},
        }),
    );
    // An establishing shot for previews; a higher priority number keeps the follow camera active.
    s.entity(
        "e_camwide",
        "Establishing camera",
        json!({"Transform": {"position": [-26.0, 12.0, -24.0]}, "Camera": {"fov": 52, "priority": 10}}),
    );
    s.commands.push(("transform.lookAt", json!({"entity": "e_camwide", "point": [2.0, -7.0, 52.0]})));
    s.entity("e_run", "Run", json!({"GlintRun": {"checkpointCount": 2, "start": START.to_array()}}));
    let calls = std::mem::take(&mut s.commands);
    ctx.apply(SCENE.0, "Glint: Foundry Steps, ball, course and run director", calls)?;
    ctx.set_main(SCENE.0, SCENE.1)
}

/// The chrome ball's renderer fields.
fn ball_look() -> Value {
    json!({"shape": "sphere", "size": [1, 1, 1], "color": [0.95, 0.95, 0.95, 1], "roughness": 0.04, "metallic": 1})
}

/// Checkpoint pads and the goal: lit plates; `play` adds the components the director reads.
fn pads(s: &mut Scene, models: &Models, play: bool) {
    let pad = &models.get("glint_pad").renderer;
    for (index, id, z) in [(1, "e_c1", 40.0f32), (2, "e_c2", 66.0)] {
        let mut components = json!({
            "Transform": {"position": [0, 0.8, z]},
            "MeshRenderer": pad,
        });
        components["MeshRenderer"]["emissive"] = json!([5000, 5000, 5000]);
        if play {
            components["Checkpoint"] = json!({"index": index, "radius": 2.6});
        }
        s.entity(id, &format!("Checkpoint {index}"), components);
    }
    let mut goal = json!({
        "Transform": {"position": [0, -1.8, 94]},
        "MeshRenderer": models.get("glint_goal").renderer,
    });
    goal["MeshRenderer"]["emissive"] = json!([6000, 6000, 6000]);
    if play {
        goal["Goal"] = json!({"radius": 3.2});
    }
    s.entity("e_goal", "Goal", goal);
}

/// The menu backdrop: the same art, the ball resting at the start, and the fly-over eye and
/// target the shell's camera uses.
fn menu(ctx: &Ctx) -> Result<()> {
    ctx.new_document(MENU.0, "scene", MENU.1)?;
    let models = models(ctx)?;
    let mut s = Scene::new(&models);
    art(&mut s);
    s.entity(
        "e_menumover",
        "Moving platform",
        json!({"Transform": {"position": round3(MOVER_ORIGIN + MOVER_TRAVEL * 0.6)}, "MeshRenderer": models.get("glint_mover").renderer}),
    );
    for (id, z, yaw, _) in BARS {
        s.entity(
            &format!("{id}m"),
            "Spinner bar",
            json!({"Transform": {"position": [0, 1.35, z], "rotation": [0, yaw + 35.0, 0]}, "MeshRenderer": models.get("glint_bar").renderer}),
        );
    }
    pads(&mut s, &models, false);
    s.entity("e_menuball", "Ball", json!({"Transform": {"position": [0.0, 0.5, 6.0]}, "MeshRenderer": ball_look()}));
    s.entity("e_gamemenueye", "Menu vista camera", json!({"Transform": {"position": [-5.5, 3.4, -7.0]}}));
    s.entity("e_gamemenutarget", "Menu vista focus", json!({"Transform": {"position": [0.0, 0.8, 24.0]}}));
    let calls = std::mem::take(&mut s.commands);
    ctx.apply(MENU.0, "Glint: native menu backdrop", calls)
}

/// Builds the Glint project: the game scene and the menu backdrop.
pub fn build(ctx: &Ctx) -> Result<()> {
    clean(ctx, &["levels", "kit"])?;
    game(ctx)?;
    menu(ctx)?;
    ctx.check()
}
