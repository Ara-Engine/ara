//! "Glint": a small rolling-ball game, one level so far ("Foundry Steps"). The ball is a
//! dynamic sphere steered by C# (`samples/glint/content/scripts/Glint`); the course is
//! boxes, a moving platform, two spinner bars, two checkpoints and a goal. The look reuses
//! Atelier's: a numeric gradient sky, one sun, a fixed exposure and plain PBR factors.
//! The course tables below are the single source of the scene; walkable faces carry a
//! `Surface` component with the same half extents as their collider, because scripts have
//! no contact events or ray casts to ask the physics engine.
use super::{
    Ctx,
    outpost::light,
    place::{Models, Scene},
};
use anyhow::Result;
use glam::Vec3;
use serde_json::{Value, json};

/// Level scene.
pub const SCENE: (&str, &str) = ("levels/glint.scene", "sc_glint000001");
/// Fixed exposure (EV100), as in Atelier: no auto-exposure anywhere.
const EV100: f32 = 14.0;
/// Where the ball starts.
const START: Vec3 = Vec3::new(0.0, 0.6, -2.0);
/// Thickness of every slab, metres.
const SLAB: f32 = 1.0;

/// A static slab: footprint (X, Z), the height of its top face and its colour.
struct Slab {
    id: &'static str,
    name: &'static str,
    /// Centre X, centre Z.
    at: (f32, f32),
    /// Size X, size Z.
    size: (f32, f32),
    top: f32,
    color: [f32; 3],
    roughness: f32,
    metallic: f32,
}

const fn slab(
    id: &'static str,
    name: &'static str,
    at: (f32, f32),
    size: (f32, f32),
    top: f32,
    color: [f32; 3],
) -> Slab {
    Slab {
        id,
        name,
        at,
        size,
        top,
        color,
        roughness: 0.7,
        metallic: 0.0,
    }
}

const STONE: [f32; 3] = [0.42, 0.4, 0.37];
const STEEL: [f32; 3] = [0.5, 0.52, 0.55];

/// The static course along +Z, from the start dock to the goal island.
fn slabs() -> Vec<Slab> {
    vec![
        slab("e_s0", "Start dock", (0.0, 0.0), (8.0, 10.0), 0.0, STONE),
        slab("e_s1", "Runway", (0.0, 12.0), (4.0, 14.0), 0.0, STEEL),
        slab("e_s2", "Pad B", (0.0, 25.0), (6.0, 6.0), 0.8, STONE),
        slab("e_s3", "Checkpoint pad 1", (0.0, 40.0), (6.0, 6.0), 0.8, STONE),
        slab("e_s4", "Beam", (0.0, 53.0), (1.8, 20.0), 0.8, STEEL),
        slab("e_s5", "Checkpoint pad 2", (0.0, 66.0), (6.0, 6.0), 0.8, STONE),
        slab("e_s6", "Island 1", (0.0, 74.0), (3.4, 3.4), 0.2, STEEL),
        slab("e_s7", "Island 2", (2.0, 80.0), (3.4, 3.4), -0.5, STEEL),
        slab("e_s8", "Island 3", (-1.0, 86.0), (3.4, 3.4), -1.2, STEEL),
        slab("e_s9", "Goal island", (0.0, 94.0), (8.0, 8.0), -1.8, STONE),
    ]
}

/// A platform sliding on a sine: centre of its path, travel and period.
struct Slider {
    id: &'static str,
    name: &'static str,
    origin: Vec3,
    travel: Vec3,
    size: (f32, f32),
    period: u32,
}

const SLIDER: Slider = Slider {
    id: "e_m1",
    name: "Moving platform",
    origin: Vec3::new(0.0, 0.8 - SLAB * 0.5, 32.5),
    travel: Vec3::new(4.0, 0.0, 0.0),
    size: (3.2, 3.2),
    period: 300,
};

/// Spinner bars over the beam: Z of the pivot, start yaw, degrees a second.
const BARS: [(&str, f32, f32, f32); 2] = [
    ("e_b1", 48.0, 0.0, 80.0),
    ("e_b2", 58.0, 90.0, -95.0),
];

fn surface_components(slab: &Slab) -> Value {
    let half = [slab.size.0 * 0.5, SLAB * 0.5, slab.size.1 * 0.5];
    json!({
        "Transform": {"position": super::place::round3(Vec3::new(slab.at.0, slab.top - SLAB * 0.5, slab.at.1))},
        "MeshRenderer": {"shape": "box", "size": [slab.size.0, SLAB, slab.size.1],
            "color": [slab.color[0], slab.color[1], slab.color[2], 1], "roughness": slab.roughness, "metallic": slab.metallic},
        "RigidBody": {"motion": "static"},
        "Collider": {"halfExtents": half},
        "Surface": {"half": half},
    })
}

/// Builds the Glint project: one scene, authored through typed commands.
pub fn build(ctx: &Ctx) -> Result<()> {
    ctx.new_document(SCENE.0, "scene", SCENE.1)?;
    let models = Models::default();
    let mut s = Scene::new(&models);
    s.entity(
        "e_env",
        "Sky and fixed exposure",
        json!({
            "Environment": {"sky": [0.2, 0.36, 0.85, 1], "horizon": [0.72, 0.82, 0.95, 1],
                "ground": [0.28, 0.26, 0.23, 1], "skyFalloff": 3, "intensity": 9000, "skyLight": 1},
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
        json!({"kind": "directional", "color": [1.0, 0.94, 0.84, 1], "intensity": 90_000, "sourceAngle": 0.5357}),
        Some(Vec3::new(-0.35, -0.7, -0.62)),
    ));
    for slab in slabs() {
        s.entity(slab.id, slab.name, surface_components(&slab));
    }
    // The moving platform: a kinematic slab moved from the tick number by `Mover`.
    let half = [SLIDER.size.0 * 0.5, SLAB * 0.5, SLIDER.size.1 * 0.5];
    s.entity(
        SLIDER.id,
        SLIDER.name,
        json!({
            "Transform": {"position": super::place::round3(SLIDER.origin)},
            "MeshRenderer": {"shape": "box", "size": [SLIDER.size.0, SLAB, SLIDER.size.1],
                "color": [0.85, 0.45, 0.2, 1], "roughness": 0.5, "metallic": 0.0},
            "RigidBody": {"motion": "kinematic"},
            "Collider": {"halfExtents": half},
            "Surface": {"half": half},
            "Mover": {"origin": SLIDER.origin.to_array(), "travel": SLIDER.travel.to_array(), "periodTicks": SLIDER.period},
        }),
    );
    // Spinner bars across the beam, a hand above its top face.
    for (id, z, yaw, rate) in BARS {
        s.entity(
            id,
            "Spinner bar",
            json!({
                "Transform": {"position": [0, 1.35, z], "rotation": [0, yaw, 0]},
                "MeshRenderer": {"shape": "box", "size": [3.6, 0.3, 0.3], "color": [0.8, 0.2, 0.15, 1], "roughness": 0.4, "metallic": 0.0},
                "RigidBody": {"motion": "kinematic"},
                "Collider": {"halfExtents": [1.8, 0.15, 0.15]},
                "Spinner": {"degreesPerSecond": rate, "startYaw": yaw},
            }),
        );
    }
    // Checkpoint pads and the goal: emissive plates with a light each.
    for (index, id, z) in [(1, "e_c1", 40.0f32), (2, "e_c2", 66.0)] {
        s.entity(
            id,
            &format!("Checkpoint {index}"),
            json!({
                "Transform": {"position": [0, 0.84, z]},
                "MeshRenderer": {"shape": "plane", "size": [4, 0.05, 4], "color": [0.1, 0.1, 0.1, 1],
                    "emissive": [6000, 4200, 1800], "castShadows": false},
                "Checkpoint": {"index": index, "radius": 2.6},
            }),
        );
    }
    s.entity(
        "e_goal",
        "Goal",
        json!({
            "Transform": {"position": [0, -1.76, 94]},
            "MeshRenderer": {"shape": "plane", "size": [6, 0.05, 6], "color": [0.1, 0.1, 0.1, 1],
                "emissive": [1500, 6000, 7000], "castShadows": false},
            "Goal": {"radius": 3.2},
        }),
    );
    // The ball: a chrome dynamic sphere driven by BallController; PlayerControl gives it
    // an input slot and its follow camera's orbit.
    s.entity(
        "e_ball",
        "Ball",
        json!({
            "Transform": {"position": START.to_array()},
            "MeshRenderer": {"shape": "sphere", "size": [1, 1, 1], "color": [0.95, 0.95, 0.95, 1], "roughness": 0.05, "metallic": 1},
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
    s.entity(
        "e_run",
        "Run",
        json!({"GlintRun": {"checkpointCount": 2, "start": START.to_array()}}),
    );
    let calls = std::mem::take(&mut s.commands);
    ctx.apply(SCENE.0, "Glint: Foundry Steps, ball, course and run director", calls)?;
    ctx.set_main(SCENE.0, SCENE.1)?;
    ctx.check()
}
