//! "Atelier": a small, clean cross-engine comparison scene. A white-plaster studio with
//! one terracotta panel, lit by one sun through a doorway, an east window and a roof slot,
//! opening onto a concrete courtyard with a few plants; so bounce light matters. Metal
//! (chrome, brass, steel), glass, walnut, graphite leather, concrete, oak and emissive bulbs;
//! one idling mannequin; six local lights with explicit source sizes.
//!
//! Built to cross engines: one fixed exposure, physical light units, a gradient sky
//! given as numbers, plain metallic-roughness glTF materials, and six camera clips
//! (three comparison clips and three test stills) as poses over time. The numbers live in
//! the tables below and are written once to `compare/atelier.spec.json` and
//! `compare/camera_clips.json`; the scene is built from the same tables, so the documents
//! and the scene cannot drift.
//! Frame: Y up, right-handed, metres; cameras look along their local -Z (glTF).
mod kits;

use super::{
    Ctx, clean,
    outpost::light,
    place::{Models, Prop, Scene, short, transform},
};
use crate::lock::Lock;
use anyhow::Result;
use glam::{Mat3, Quat, Vec3};
use serde_json::{Value, json};
use std::path::Path;

/// Level scene.
pub const SCENE: (&str, &str) = ("levels/atelier.scene", "sc_atelier01");

/// Fixed exposure, set for the room so its shaded white walls read as white (EV100 at
/// ISO 100, ACES-style reference of 1.2·2^EV cd/m² at white in Ara's meter convention),
/// the same in every engine.
const EV100: f32 = 10.0;
/// The exterior test stills' own exposures: they look at the sunlit outside, which the
/// room's exposure washes out. A box volume round each one's camera sets it in the scene,
/// and its clip carries the number for other engines. `shadow`: the wall and gravel in
/// sun. `lantern`: its glass at 5000 nits stays under white, and the plate behind it shows
/// the sunlit paving at about a quarter of it.
const TEST_EV100: [(&str, f32); 2] = [("shadow", 12.0), ("lantern", 13.0)];
/// The night scene: the same tables and kits after dark, a frame of many shadowed local
/// lights: 25 of them. Twenty wall lanterns (`kits::night_lanterns` and the courtyard's
/// own) at `LANTERN_LUMENS` and the room's five lamps, under a dim moon and a near-black
/// sky.
pub const NIGHT_SCENE: (&str, &str) = ("levels/atelier_night.scene", "sc_atelier02");
/// Set so the paving near a lantern sits at mid-grey and the gravel beyond the courtyard
/// is black: the exposure a game would ship a night at.
const NIGHT_EV100: f32 = 5.0;
/// The moon: lux on a surface facing it, colour, and the direction its light travels.
const MOON_LUX: f32 = 0.25;
const MOON_COLOR: [f32; 3] = [0.75, 0.85, 1.0];
const MOON_DIRECTION: Vec3 = Vec3::new(0.3, -0.9, 0.3);
/// The night sky's scale, cd/m² (the day's gradient, far below the night's exposure).
const NIGHT_SKY_SCALE_CD: f32 = 0.03;
/// Each night lantern: a point light of the courtyard lantern's flux, colour and size.
const LANTERN_LUMENS: f32 = 600.0;
const LANTERN_COLOR: [f32; 3] = [1.0, 0.78, 0.5];
const LANTERN_RADIUS: f32 = 0.07;
const LANTERN_RANGE: f32 = 8.0;

/// The sun: illuminance on a surface facing it, lux, and the direction its light
/// travels (unit, world space; it shines in through the doorway and the east window).
const SUN_LUX: f32 = 45_000.0;
const SUN_COLOR: [f32; 3] = [1.0, 0.91, 0.76];
const SUN_DIRECTION: Vec3 = Vec3::new(-0.45, -0.438, -0.78);
/// Gradient sky (linear colour, cd/m² at scale 1): no sun disc; the sun is the light.
const SKY_ZENITH: [f32; 3] = [0.2, 0.36, 0.85];
const SKY_HORIZON: [f32; 3] = [0.72, 0.82, 0.95];
const SKY_GROUND: [f32; 3] = [0.4, 0.4, 0.39];
const SKY_SCALE_CD: f32 = 9000.0;
const SKY_FALLOFF: f32 = 3.0;

/// One local light. `lumens` is the flux of a point and the flux a spot's cone masks
/// from a point of that flux; `length` makes the source a tube along its local X (every
/// light here is a sphere: Renderlab draws tube lights as spheres, so a tube would differ
/// between the engines compared).
struct Lamp {
    id: &'static str,
    name: &'static str,
    position: Vec3,
    /// Aim point for spots (`None` for points).
    target: Option<Vec3>,
    color: [f32; 3],
    lumens: f32,
    radius: f32,
    length: f32,
    range: f32,
    /// Spot inner/outer half-angles, degrees.
    cone: Option<(f32, f32)>,
    /// Whether it should cast shadows. Ara's `Light` has no per-light shadow switch
    /// (the renderer decides); the choice is recorded for the other engines.
    shadow: bool,
}

const LAMPS: [Lamp; 6] = [
    Lamp {
        id: "e_lpendant",
        name: "Pendant over the table",
        position: Vec3::new(0.2, 2.5, -6.0),
        target: None,
        color: [1.0, 0.82, 0.6],
        lumens: 900.0,
        radius: 0.1,
        length: 0.0,
        range: 9.0,
        cone: None,
        shadow: true,
    },
    Lamp {
        id: "e_lfloor",
        name: "Floor lamp reading spot",
        position: Vec3::new(-3.4, 1.7, -5.7),
        target: Some(Vec3::new(-2.8, 0.4, -6.9)),
        color: [1.0, 0.8, 0.58],
        lumens: 450.0,
        radius: 0.04,
        length: 0.0,
        range: 6.0,
        cone: Some((25.0, 45.0)),
        shadow: true,
    },
    Lamp {
        id: "e_lpicture",
        name: "Picture light",
        position: Vec3::new(1.6, 2.65, -8.7),
        target: Some(Vec3::new(1.6, 1.7, -8.97)),
        color: [1.0, 0.9, 0.75],
        lumens: 350.0,
        radius: 0.02,
        length: 0.0,
        range: 3.5,
        cone: Some((30.0, 55.0)),
        shadow: false,
    },
    Lamp {
        id: "e_lshelf",
        name: "Shelf LED strip",
        position: Vec3::new(-2.4, 1.2, -8.65),
        target: None,
        color: [1.0, 0.93, 0.8],
        lumens: 300.0,
        radius: 0.01,
        length: 0.0,
        range: 3.0,
        cone: None,
        shadow: false,
    },
    Lamp {
        id: "e_llantern",
        name: "Courtyard wall lantern",
        position: Vec3::new(3.3, 2.4, 0.68),
        target: None,
        color: [1.0, 0.78, 0.5],
        lumens: 600.0,
        radius: 0.07,
        length: 0.0,
        range: 8.0,
        cone: None,
        shadow: false,
    },
    Lamp {
        id: "e_lcup",
        name: "Table lamp glow",
        // The centre of the glowing globe on the table (`kits::TABLE + kits::GLOBE`).
        position: Vec3::new(0.65, 0.877, -6.03),
        target: None,
        color: [1.0, 0.8, 0.55],
        lumens: 120.0,
        radius: 0.1,
        length: 0.0,
        range: 3.0,
        cone: None,
        shadow: false,
    },
];

/// A camera pose: where it stands and what it looks at (metres, Y up).
#[derive(Clone, Copy)]
struct Pose {
    t: f32,
    position: Vec3,
    target: Vec3,
    /// Vertical field of view, degrees.
    fov: f32,
    /// A hard cut follows this key: no interpolation to the next key.
    cut: bool,
}

const fn key(t: f32, position: Vec3, target: Vec3, fov: f32, cut: bool) -> Pose {
    Pose {
        t,
        position,
        target,
        fov,
        cut,
    }
}

/// A camera clip: its name, what it shows, and its keys.
type Clip = (&'static str, &'static str, &'static [Pose]);

/// The comparison clips: a still, a moving stretch, one hard cut, and three test stills
/// (`glow`: an emitter seen in a near-mirror and in chrome; `shadow`: thin shadows more
/// than 40 m from the camera; `lantern`: an emitter seen in a near-mirror in daylight).
/// Positions interpolate linearly and rotations by slerp between keys, at 30 frames a
/// second.
const CLIPS: [Clip; 6] = [
    (
        "still",
        "Interior from the doorway side: sun patches, sculpture, table, armchair.",
        &[key(
            0.0,
            Vec3::new(3.1, 1.5, -0.6),
            Vec3::new(-1.5, 1.6, -5.0),
            46.0,
            false,
        )],
    ),
    (
        "dolly",
        "A slow push from the courtyard through the doorway toward the table.",
        &[
            key(
                0.0,
                Vec3::new(0.2, 1.5, 6.5),
                Vec3::new(-0.4, 1.3, -4.0),
                50.0,
                false,
            ),
            key(
                8.0,
                Vec3::new(-0.2, 1.5, -1.2),
                Vec3::new(-1.0, 1.2, -5.0),
                50.0,
                false,
            ),
        ],
    ),
    (
        "cut",
        "A low courtyard pan, a hard cut at 3 s to the chrome sphere.",
        &[
            key(
                0.0,
                Vec3::new(-5.0, 1.2, 7.5),
                Vec3::new(0.0, 1.6, 0.4),
                46.0,
                false,
            ),
            key(
                3.0,
                Vec3::new(-3.0, 1.2, 7.5),
                Vec3::new(2.0, 1.6, 0.4),
                46.0,
                true,
            ),
            key(
                3.0,
                Vec3::new(-0.4, 1.4, -0.6),
                Vec3::new(-1.6, 1.27, -2.4),
                40.0,
                false,
            ),
            key(
                6.0,
                Vec3::new(-0.9, 1.4, -0.9),
                Vec3::new(-1.6, 1.27, -2.4),
                40.0,
                false,
            ),
        ],
    ),
    (
        "glow",
        "The table's end, in shade: a glowing globe on a polished nickel tray (roughness 0.07) beside a chrome sphere (0.03).",
        &[key(
            0.0,
            Vec3::new(1.85, 1.3, -5.7),
            Vec3::new(0.8, 0.8, -6.13),
            34.0,
            false,
        )],
    ),
    (
        "shadow",
        "From the gravel west of the courtyard: three masts 42 to 44 m away and their shadows on the boundary wall at 50 to 54 m.",
        &[key(
            0.0,
            Vec3::new(-8.5, 1.29, 6.0),
            Vec3::new(-17.5, 5.0, -44.0),
            26.0,
            false,
        )],
    ),
    (
        "lantern",
        "The courtyard wall lantern from above its right: its glowing glass beside its own reflection in the polished nickel plate behind it (roughness 0.07), in daylight.",
        &[key(
            0.0,
            Vec3::new(4.2, 3.3, 1.9),
            Vec3::new(3.42, 2.45, 0.43),
            30.0,
            false,
        )],
    ),
];

/// The night scene's clips: a still along the front wall from the east planter (the
/// lantern beside its reflection in its plate on the right, the doorway and the room's
/// lamps, the west and far walls' lanterns across the courtyard), a three-second push
/// toward the doorway, and the day's `lantern` view of the plate.
const NIGHT_CLIPS: [Clip; 3] = [
    (
        "night",
        "The courtyard at night from its east side, looking west along the front wall: twenty lanterns and the room's lamps.",
        &[key(
            0.0,
            Vec3::new(5.4, 1.75, 3.0),
            Vec3::new(-3.0, 1.5, 1.2),
            62.0,
            false,
        )],
    ),
    (
        "nightdolly",
        "A three-second push across the courtyard toward the doorway at night.",
        &[
            key(
                0.0,
                Vec3::new(0.5, 1.5, 8.0),
                Vec3::new(0.0, 1.6, 0.0),
                55.0,
                false,
            ),
            key(
                3.0,
                Vec3::new(0.3, 1.5, 4.5),
                Vec3::new(0.0, 1.6, 0.0),
                55.0,
                false,
            ),
        ],
    ),
    (
        "nightlantern",
        "The wall lantern beside its reflection in its plate, at night: the brightest thing in the frame.",
        &[key(
            0.0,
            Vec3::new(4.2, 3.3, 1.9),
            Vec3::new(3.42, 2.45, 0.43),
            30.0,
            false,
        )],
    ),
];

/// One time of day: a scene built from the same tables and kits.
struct Time {
    scene: (&'static str, &'static str),
    summary: &'static str,
    /// The scene's fixed exposure.
    ev100: f32,
    /// The directional light: its name, lux, colour and travel direction.
    sun: (&'static str, f32, [f32; 3], Vec3),
    sky_scale_cd: f32,
    clips: &'static [Clip],
    /// The clip whose first key is the scene's main camera.
    main_clip: &'static str,
    /// Exposure boxes round exterior test cameras: clip and EV100.
    test_ev100: &'static [(&'static str, f32)],
    /// The night's extra lanterns: their kit and a light in each.
    lanterns: bool,
}
const DAY: Time = Time {
    scene: SCENE,
    summary: "Atelier: studio interior, courtyard and light table",
    ev100: EV100,
    sun: ("Sun", SUN_LUX, SUN_COLOR, SUN_DIRECTION),
    sky_scale_cd: SKY_SCALE_CD,
    clips: &CLIPS,
    main_clip: "still",
    test_ev100: &TEST_EV100,
    lanterns: false,
};
const NIGHT: Time = Time {
    scene: NIGHT_SCENE,
    summary: "Atelier at night: twenty wall lanterns and the room's lamps",
    ev100: NIGHT_EV100,
    sun: ("Moon", MOON_LUX, MOON_COLOR, MOON_DIRECTION),
    sky_scale_cd: NIGHT_SKY_SCALE_CD,
    clips: &NIGHT_CLIPS,
    main_clip: "night",
    test_ev100: &[],
    lanterns: true,
};

/// A camera looks along -Z: its rotation as a glTF quaternion `[x, y, z, w]`.
fn look_rotation(position: Vec3, target: Vec3) -> [f32; 4] {
    let forward = (target - position).normalize();
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);
    Quat::from_mat3(&Mat3::from_cols(right, up, -forward))
        .normalize()
        .to_array()
}

fn round3(v: Vec3) -> [f64; 3] {
    super::place::round3(v)
}

fn lamp_light(lamp: &Lamp) -> Value {
    let mut light = json!({
        "kind": if lamp.cone.is_some() { "spot" } else { "point" },
        "color": [lamp.color[0], lamp.color[1], lamp.color[2], 1],
        "intensity": lamp.lumens, "units": "lumens", "range": lamp.range,
        "sourceRadius": lamp.radius,
    });
    if lamp.length > 0.0 {
        light["sourceLength"] = json!(lamp.length);
    }
    if let Some((inner, outer)) = lamp.cone {
        light["innerAngle"] = json!(inner);
        light["outerAngle"] = json!(outer);
    }
    light
}

/// The plain-file sidecar for other engines: conventions, exposure, sky, sun, lights.
fn spec() -> Value {
    json!({
        "scene": "atelier",
        "conventions": {
            "handedness": "right",
            "up": "+Y",
            "units": "metres",
            "cameraForward": "-Z (glTF)",
            "note": "Left-handed engines (Unity): negate X on import, flip triangle winding.",
            "roomForward": "The room runs toward -Z from its front wall at z=0; the courtyard is on +Z.",
        },
        "exposure": {"mode": "fixed", "ev100": EV100, "isoNote": "ISO 100; photometric exposure H = L / (1.2 * 2^EV100)",
            "toneMapping": "AgX (Medium High Contrast look) in Ara; export linear HDR for comparisons"},
        "sky": {"kind": "gradient equirect, no sun disc", "zenith": SKY_ZENITH, "horizon": SKY_HORIZON,
            "ground": SKY_GROUND, "scaleCdPerM2": SKY_SCALE_CD, "falloff": SKY_FALLOFF},
        "sun": {"illuminanceLux": SUN_LUX, "color": SUN_COLOR, "travelDirection": SUN_DIRECTION.normalize().to_array(),
            "angularDiameterDeg": 0.5357, "castsShadows": true},
        "lights": LAMPS.iter().map(|l| json!({
            "name": l.name, "type": if l.cone.is_some() { "spot" } else { "point" },
            "position": l.position.to_array(), "aimAt": l.target.map(|t| t.to_array()),
            "color": l.color, "lumens": l.lumens, "sourceRadius": l.radius,
            "sourceLength": l.length, "range": l.range,
            "coneHalfAnglesDeg": l.cone.map(|(i, o)| [i, o]), "castsShadows": l.shadow,
        })).collect::<Vec<_>>(),
        "engineSideExtras": ["mannequin idle animation", "dust particles in the sun shaft"],
        "fittingsThatCastNoShadow": {
            "why": "Each encloses its own light: a fitting that shadowed it would leave the lamp lighting nothing.",
            "models": ["atelier_lamps (the table lamp's globe, the pendant's shade and bulb, the floor lamp's bulb)",
                "atelier_lantern_glass (the courtyard wall lantern's glass)",
                "atelier_night_glass (the night lanterns' glass, night scene only)"],
        },
        "night": {
            "scene": NIGHT_SCENE.0,
            "note": "The same geometry, materials and six lights, plus the lanterns below; every local light casts shadows.",
            "exposure": {"mode": "fixed", "ev100": NIGHT_EV100},
            "sky": {"scaleCdPerM2": NIGHT_SKY_SCALE_CD},
            "moon": {"illuminanceLux": MOON_LUX, "color": MOON_COLOR,
                "travelDirection": MOON_DIRECTION.normalize().to_array(), "castsShadows": true},
            "lanterns": {"type": "point", "lumens": LANTERN_LUMENS, "color": LANTERN_COLOR,
                "sourceRadius": LANTERN_RADIUS, "range": LANTERN_RANGE, "castsShadows": true,
                "positions": kits::night_lanterns().into_iter().map(round3).collect::<Vec<_>>(),
                "also": "the day's Courtyard wall lantern, unchanged"},
        },
    })
}

/// The clips of both scenes as poses over time (plain JSON, 30 fps), each with its scene
/// and its fixed exposure.
fn clips() -> Value {
    let clips: Vec<Value> = [&DAY, &NIGHT]
        .into_iter()
        .flat_map(|time| time.clips.iter().map(move |clip| (time, clip)))
        .map(|(time, (name, note, keys))| {
            let ev100 = time
                .test_ev100
                .iter()
                .find(|(clip, _)| clip == name)
                .map_or(time.ev100, |(_, ev)| *ev);
            json!({
                "name": name, "note": note, "duration": keys.last().map_or(0.0, |k| k.t),
                "scene": time.scene.0,
                "ev100": ev100,
                "keys": keys.iter().map(|k| json!({
                    "t": k.t, "position": round3(k.position), "rotation": look_rotation(k.position, k.target),
                    "lookAt": round3(k.target), "fovYDeg": k.fov, "cut": k.cut,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "handedness": "right", "up": "+Y", "units": "metres", "cameraForward": "-Z (glTF)",
        "fps": 30, "resolution": [1920, 1080],
        "interpolation": "position: linear; rotation: slerp; a key with cut=true ends its segment",
        "clips": clips,
    })
}

/// Writes every generated, git-ignored kit file (`kit/`) without authoring a document.
pub fn generate(root: &Path, lock: &Lock) -> Result<()> {
    super::outpost::suit(root, lock)?;
    kits_written(root, lock)?;
    Ok(())
}

fn kits_written(root: &Path, lock: &Lock) -> Result<Vec<(String, String)>> {
    Ok(vec![
        ("atelier_shell".into(), kits::shell(root, lock)?),
        ("atelier_furniture".into(), kits::furniture(root, lock)?),
        ("atelier_courtyard".into(), kits::courtyard(root, lock)?),
        ("atelier_lamps".into(), kits::lamps(root, lock)?),
        (
            "atelier_lantern_glass".into(),
            kits::wall_lantern_glass(root, lock)?,
        ),
        ("atelier_night".into(), kits::night(root, lock)?),
        ("atelier_night_glass".into(), kits::night_glass(root, lock)?),
    ])
}

/// Builds the Atelier project.
pub fn build(ctx: &Ctx) -> Result<()> {
    clean(ctx, &["levels", "anim", "prefabs", "materials", "kit"])?;
    let mut models = Models::default();
    for (name, asset) in [
        ("flight_helmet", "khronos_flight_helmet"),
        ("antique_camera", "khronos_antique_camera"),
        ("lantern", "lantern_01"),
        ("radio", "vintage_radio_transceiver"),
        ("shrub", "shrub_04"),
        ("fern", "fern_02"),
        ("pot_a", "potted_plant_01"),
        ("pot_b", "potted_plant_02"),
    ] {
        ctx.third_party(&mut models, name, asset)?;
    }
    // A mid-grey clay mannequin (the suit palette recast): it stands in full sun, where a
    // pale one clips at the room's exposure.
    let palette = crate::kit::recolor(
        &ctx.root,
        &ctx.lock,
        "m2m_human",
        "color-palette",
        [0.3, 0.29, 0.27],
        "human_suit",
    )?;
    super::materials::palette(ctx, &mut models, "human", "m2m_human", &palette)?;
    for (name, path) in kits_written(&ctx.root, &ctx.lock)? {
        ctx.model(
            &mut models,
            &name,
            &path,
            &format!("as_kit{}", short(&name)),
        )?;
    }
    for time in [&DAY, &NIGHT] {
        author(ctx, &models, time)?;
    }
    ctx.set_main(SCENE.0, SCENE.1)?;
    let compare = ctx.root.join("compare");
    std::fs::create_dir_all(&compare)?;
    std::fs::write(
        compare.join("atelier.spec.json"),
        ara_json::to_canonical_string(&spec()),
    )?;
    std::fs::write(
        compare.join("camera_clips.json"),
        ara_json::to_canonical_string(&clips()),
    )?;
    ctx.check()
}

/// Authors one time of day's scene from the shared tables.
fn author(ctx: &Ctx, models: &Models, time: &Time) -> Result<()> {
    ctx.new_document(time.scene.0, "scene", time.scene.1)?;
    let mut s = Scene::new(models);
    s.entity(
        "e_env",
        "Gradient sky and fixed exposure",
        json!({
            "Environment": {
                "sky": [SKY_ZENITH[0], SKY_ZENITH[1], SKY_ZENITH[2], 1],
                "horizon": [SKY_HORIZON[0], SKY_HORIZON[1], SKY_HORIZON[2], 1],
                "ground": [SKY_GROUND[0], SKY_GROUND[1], SKY_GROUND[2], 1],
                "skyFalloff": SKY_FALLOFF, "intensity": time.sky_scale_cd, "skyLight": 1,
            },
            // One fixed exposure and one display transform: nothing else. AgX because the sunlit
            // floor is about ten times the walls in shade, which Neutral clips to flat white.
            "PostProcessVolume": {"overrides": [
                {"exposure": "manual", "ev100": time.ev100},
                {"module": "toneMapping", "toneMapping": "agxMediumHighContrast"},
            ]},
        }),
    );
    // Each exterior test camera stands in its own box; no other camera comes within 4 m
    // of either.
    for &(clip, ev100) in time.test_ev100 {
        let camera = time
            .clips
            .iter()
            .find(|(name, ..)| *name == clip)
            .map_or(Vec3::ZERO, |(_, _, keys)| keys[0].position);
        s.entity(
            &format!("e_{clip}ev"),
            &format!("Exposure for the {clip} still"),
            json!({
                "Transform": {"position": round3(camera), "scale": [2, 2, 2]},
                "PostProcessVolume": {"shape": "box", "priority": 10, "overrides": [
                    {"exposure": "manual", "ev100": ev100},
                ]},
            }),
        );
    }
    let (sun, lux, colour, travel) = time.sun;
    s.commands.extend(light(
        "e_sun",
        sun,
        Vec3::new(0.0, 40.0, 30.0),
        json!({"kind": "directional", "color": [colour[0], colour[1], colour[2], 1],
            "intensity": lux, "sourceAngle": 0.5357}),
        Some(travel),
    ));
    for lamp in &LAMPS {
        let direction = lamp.target.map(|t| t - lamp.position);
        s.commands.extend(light(
            lamp.id,
            lamp.name,
            lamp.position,
            lamp_light(lamp),
            direction,
        ));
    }
    // The architecture and the furniture: one model each, no collision.
    s.prop(
        Prop::new(
            "e_shell",
            "Room and courtyard",
            "atelier_shell",
            Vec3::ZERO,
            0.0,
        )
        .at_origin()
        .decor(),
    );
    s.prop(
        Prop::new(
            "e_furniture",
            "Furniture",
            "atelier_furniture",
            Vec3::ZERO,
            0.0,
        )
        .at_origin()
        .decor(),
    );
    s.prop(
        Prop::new(
            "e_courtyard",
            "Courtyard dressing",
            "atelier_courtyard",
            Vec3::ZERO,
            0.0,
        )
        .at_origin()
        .decor(),
    );
    // The lamp fittings that enclose their own light (`kits::lamps`, the lantern glass):
    // they glow, and they must not shadow the light inside them, which is this switch on
    // the mesh renderer. Renderlab does not honour it yet (FINDINGS 17), so today each of
    // these lamps lights only the inside of its own fitting.
    for (id, name, model, nits) in [
        ("e_lamps", "Lamp shades and bulbs", "atelier_lamps", 6000),
        (
            "e_lanternglass",
            "Wall lantern glass",
            "atelier_lantern_glass",
            5000,
        ),
    ] {
        s.prop(
            Prop::new(id, name, model, Vec3::ZERO, 0.0)
                .at_origin()
                .decor()
                .renderer(json!({"emissive": [nits, nits, nits], "castShadows": false})),
        );
    }
    // The night's extra lanterns: their kit, and a point light at each one's glass.
    if time.lanterns {
        s.prop(
            Prop::new(
                "e_nightkit",
                "Night lanterns",
                "atelier_night",
                Vec3::ZERO,
                0.0,
            )
            .at_origin()
            .decor(),
        );
        s.prop(
            Prop::new(
                "e_nightglass",
                "Night lantern glass",
                "atelier_night_glass",
                Vec3::ZERO,
                0.0,
            )
            .at_origin()
            .decor()
            .renderer(json!({"emissive": [5000, 5000, 5000], "castShadows": false})),
        );
        for (i, position) in kits::night_lanterns().into_iter().enumerate() {
            s.commands.extend(light(
                &format!("e_lnight{i}"),
                &format!("Night lantern {i}"),
                position,
                json!({"kind": "point",
                    "color": [LANTERN_COLOR[0], LANTERN_COLOR[1], LANTERN_COLOR[2], 1],
                    "intensity": LANTERN_LUMENS, "units": "lumens", "range": LANTERN_RANGE,
                    "sourceRadius": LANTERN_RADIUS}),
                None,
            ));
        }
    }
    // Third-party objects: on the table, the shelf and the bench.
    for (id, name, model, at, yaw, scale) in [
        (
            "e_camprop",
            "Antique camera",
            "antique_camera",
            Vec3::new(-0.35, 0.765, -5.95),
            25.0,
            0.11,
        ),
        (
            "e_helmet",
            "Flight helmet",
            "flight_helmet",
            Vec3::new(-2.45, 2.22, -8.7),
            200.0,
            0.9,
        ),
        (
            "e_radio",
            "Radio",
            "radio",
            Vec3::new(-1.7, 1.19, -8.7),
            170.0,
            0.8,
        ),
        (
            "e_lantern",
            "Lantern",
            "lantern",
            Vec3::new(-5.6, 0.48, 4.0),
            20.0,
            1.6,
        ),
    ] {
        s.prop(Prop::new(id, name, model, at, yaw).scaled(scale).decor());
    }
    // Foliage: planters, the doorway and a corner of the room.
    for (i, (model, at, yaw, scale)) in [
        ("shrub", Vec3::new(5.7, 0.62, 2.2), 0.0, 1.0),
        ("fern", Vec3::new(5.7, 0.62, 3.4), 40.0, 1.0),
        ("shrub", Vec3::new(5.7, 0.62, 5.6), 80.0, 1.1),
        ("fern", Vec3::new(5.7, 0.62, 6.8), 10.0, 1.0),
        ("fern", Vec3::new(-2.6, 0.62, 1.3), 120.0, 1.0),
        ("shrub", Vec3::new(2.6, 0.62, 1.3), 0.0, 0.9),
        ("pot_a", Vec3::new(3.3, 0.0, -8.4), 200.0, 1.0),
        ("pot_b", Vec3::new(-3.4, 0.0, -0.9), 30.0, 1.0),
        // Courtyard dressing: a second plant in each doorway planter, terracotta pots at
        // the building's front corners and one by the bench.
        ("shrub", Vec3::new(-3.05, 0.62, 1.3), 60.0, 0.8),
        ("fern", Vec3::new(3.05, 0.62, 1.3), 200.0, 0.9),
        ("pot_a", Vec3::new(4.3, 0.0, 1.2), 140.0, 1.1),
        ("pot_b", Vec3::new(-4.3, 0.0, 1.2), 310.0, 1.0),
        ("pot_a", Vec3::new(-6.0, 0.0, 5.3), 20.0, 1.0),
    ]
    .into_iter()
    .enumerate()
    {
        s.prop(
            Prop::new(&format!("e_plant{i}"), "Plant", model, at, yaw)
                .scaled(scale)
                .decor(),
        );
    }
    // The mannequin stands by the courtyard bench, turned toward the doorway: in view of the
    // dolly and the cut clip but not the interior still, because none of the library's idle
    // clips is a relaxed standing pose (wide stance).
    s.entity(
        "e_mannequin",
        "Mannequin (idle)",
        json!({
            "Transform": transform(Vec3::new(-4.7, 0.0, 3.3), 105.0, 1.0),
            "MeshRenderer": models.get("human").renderer,
            "Animator": {"idle": "Idle_Talking"},
        }),
    );
    s.entity(
        "e_dust",
        "Dust motes in the sun shaft",
        json!({"Transform": {"position": [-1.0, 0.8, -1.6]}, "VisualEffect": {"effect": "dust", "scale": 1.4}}),
    );
    // One camera entity per clip key, so `ara capture --camera` can render each.
    for &(name, _, keys) in time.clips {
        for (i, k) in keys.iter().enumerate() {
            let id = format!("e_cam{name}{i}");
            s.entity(
                &id,
                &format!("Camera {name} {i}"),
                // The still's first key is the scene's main camera (priority 0); the rest are
                // preferred last, so an export or a Play of the scene frames the still.
                json!({"Transform": {"position": round3(k.position)},
                    "Camera": {"fov": k.fov, "priority": if name == time.main_clip { 0 } else { 10 }}}),
            );
            s.commands.push((
                "transform.lookAt",
                json!({"entity": id, "point": round3(k.target)}),
            ));
        }
    }
    let calls = std::mem::take(&mut s.commands);
    ctx.apply(time.scene.0, time.summary, calls)
}
