//! "Glint": a rolling-ball game, one course ("Skyward"): decks, a descent and a kicker, a split
//! (beam or detour), a banked turn, sliders, spinner bars and a cannon that fires the ball 80 m
//! to the goal plaza, on pylons high over a cloud deck in low sun. The ball is a dynamic sphere
//! steered by C# (`samples/glint/content/scripts/Glint`).
//!
//! Everything comes from `course.rs` (the route as data). Two scenes share one art pass:
//! `levels/glint.scene` (the game: colliders, movers, the ball, the director)
//! and `levels/menu.scene` (the native menu's backdrop: the same art, no gameplay components, a
//! fly-over camera marked by two points). The UI's markup, bindings and resources, the sounds and
//! the autopilot's route are generated here too.
mod compare;
mod course;
mod icons;
mod kits;
mod sounds;
mod ui;

use super::{
    Ctx, clean,
    outpost::light,
    place::{Models, Prop, Scene, round3, short},
};
use crate::lock::Lock;
use anyhow::Result;
use course::{Course, Step};
use glam::{Quat, Vec3};
use serde_json::{Value, json};
use std::path::Path;

/// Gameplay scene.
pub const SCENE: (&str, &str) = ("levels/glint.scene", "sc_glint000001");
/// The menu backdrop the native shell requires at this path.
const MENU: (&str, &str) = ("levels/menu.scene", "sc_glint000002");
/// The mixer (master, music, sfx) and the two-stem score.
const MIXER: (&str, &str) = ("audio/glint.mixer", "mx_glint");
const MUSIC: (&str, &str) = ("audio/glint.music", "mu_glint");
/// Fixed exposure (EV100): no auto-exposure anywhere.
const EV100: f32 = 13.5;
/// Direction the sun's light travels: 20 degrees up, ahead and to the left of the start, so the
/// course is side-lit from the front for the first half and from behind after the turn.
const SUN: Vec3 = Vec3::new(-0.163, -0.342, -0.926);
/// Emissive levels in cd/m2 (a sunlit pale deck is about 7,000): the course's chevrons, the
/// pylons' bands, the set pieces (rings, arches, cannon, platforms), the goal, the orbs, the
/// far islands and the sun's disc.
const LIT_COURSE: f32 = 45_000.0;
const LIT_PYLONS: f32 = 30_000.0;
const LIT_SET: f32 = 50_000.0;
const LIT_GOAL: f32 = 70_000.0;
const LIT_ORB: f32 = 90_000.0;
const LIT_ISLE: f32 = 40_000.0;
const LIT_SUN: f32 = 600_000.0;
/// The spinner bar's length and thickness (m).
pub const BAR: (f32, f32) = (6.4, 0.32);
/// Gold, silver and bronze times in seconds (the UI prints the same numbers).
pub const MEDALS: [u32; 3] = [60, 75, 100];

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

/// Writes every kit's glTF and returns each kit's name and path.
fn kits_written(root: &Path, lock: &Lock, c: &Course) -> Result<Vec<(&'static str, String)>> {
    let slider = c
        .sliders
        .first()
        .map_or(Vec3::new(2.2, 0.4, 2.2), |s| s.half);
    let cannon = c.cannon.as_ref().expect("the course has a cannon");
    Ok(vec![
        ("glint_course", kits::course(root, lock, c)?),
        ("glint_supports", kits::supports(root, lock, c)?),
        ("glint_mover", kits::mover(root, lock, slider)?),
        ("glint_bar", kits::bar(root, lock)?),
        ("glint_orb", kits::orb(root, lock)?),
        ("glint_hoop", kits::hoop(root, lock)?),
        ("glint_gate", kits::gate(root, lock)?),
        (
            "glint_cannon",
            kits::cannon(root, lock, cannon.muzzle - cannon.breech)?,
        ),
        ("glint_goal", kits::goal(root, lock)?),
        ("glint_cloud0", kits::cloud(root, lock, 0)?),
        ("glint_cloud1", kits::cloud(root, lock, 1)?),
        ("glint_cloud2", kits::cloud(root, lock, 2)?),
        ("glint_trail", kits::trail(root, lock)?),
        ("glint_span", kits::span(root, lock)?),
        ("glint_gantry", kits::gantry(root, lock)?),
        ("glint_isle", kits::isle(root, lock)?),
        ("glint_sun", kits::sun(root, lock)?),
    ])
}

/// Writes the generated, git-ignored files (kit geometry and the synthesised sounds) without
/// authoring a document: what a fresh clone needs before it can play.
pub fn generate(root: &Path, lock: &Lock) -> Result<()> {
    kits_written(root, lock, &course::build())?;
    sounds::write(&root.join("content/audio"))
}

/// Registers the kits as models.
fn models(ctx: &Ctx, c: &Course) -> Result<Models> {
    let mut models = Models::default();
    for (name, path) in kits_written(&ctx.root, &ctx.lock, c)? {
        ctx.model(&mut models, name, &path, &format!("as_kit{}", short(name)))?;
    }
    Ok(models)
}

/// A repeatable number in 0..1 from an index and a salt (placement only; never gameplay).
fn scatter(index: u32, salt: u32) -> f32 {
    let mut x = index.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

/// A decoration: a kit model at a place, with its own rotation (scene Euler degrees), scale
/// and emissive level (cd/m2).
#[allow(clippy::too_many_arguments)]
fn decor(
    s: &mut Scene,
    id: &str,
    name: &str,
    model: &str,
    at: Vec3,
    rotation: [f32; 3],
    scale: Vec3,
    emissive: f32,
) {
    let mut renderer = s.models.get(model).renderer.clone();
    if emissive > 0.0 {
        renderer["emissive"] = json!([emissive, emissive, emissive]);
    }
    s.entity(
        id,
        name,
        json!({
            "Transform": {"position": round3(at), "rotation": rotation, "scale": scale.to_array()},
            "MeshRenderer": renderer,
        }),
    );
}

/// The sky: a deep blue zenith held low (high falloff), a pale band at the horizon with a thin
/// warm glow (kept weak: it lights the clouds' shadow sides, which should stay cool), and a hazy blue below the horizon where the cloud opens.
fn environment() -> Value {
    json!({"sky": [0.04, 0.17, 0.62, 1], "horizon": [0.62, 0.8, 1.0, 1], "skyFalloff": 5.5,
        "glow": [0.5, 0.3, 0.2, 1], "glowFalloff": 34, "ground": [0.72, 0.84, 1.0, 1],
        "intensity": 7000, "skyLight": 1})
}

/// The haze: thick at the cloud deck (it softens the banks into one sea), light at the course,
/// so the pylons fade as they go down.
fn fog(deck: f32) -> Value {
    json!({"density": 0.012, "baseHeight": deck, "falloff": 15, "albedo": [0.78, 0.86, 1.0, 1], "anisotropy": 0.5})
}

/// The cloud deck's level: well under the lowest piece.
fn deck_level(c: &Course) -> f32 {
    c.pieces.iter().map(|p| p.center.y).fold(f32::MAX, f32::min) - 50.0
}

/// A local point light.
struct Lamp {
    id: String,
    name: &'static str,
    at: Vec3,
    color: [f32; 3],
    lumens: f32,
    /// Source radius and range, metres.
    radius: f32,
    range: f32,
}

/// The local lights: one over each checkpoint and one over the goal.
fn lamps(c: &Course) -> Vec<Lamp> {
    let mut all: Vec<Lamp> = c
        .checkpoints
        .iter()
        .enumerate()
        .map(|(i, (at, _))| Lamp {
            id: format!("e_lcp{i}"),
            name: "Gate light",
            at: *at + Vec3::new(0.0, 4.0, 0.0),
            color: [1.0, 0.45, 0.25],
            lumens: 90_000.0,
            radius: 0.3,
            range: 16.0,
        })
        .collect();
    all.push(Lamp {
        id: "e_lgoal".into(),
        name: "Goal light",
        at: c.goal + Vec3::new(0.0, 9.0, 0.0),
        color: [1.0, 0.6, 0.36],
        lumens: 160_000.0,
        radius: 0.8,
        range: 40.0,
    });
    all
}

/// Sky, sun, haze, lights, the visual course and the world around it: everything both scenes
/// share. With `play` the gameplay components the director and the cannon read are added too.
fn art(s: &mut Scene, c: &Course, play: bool) {
    let models = s.models;
    let centre = (c.start + c.goal) * 0.5;
    let deck = deck_level(c);
    s.entity(
        "e_env",
        "Clear sky and fixed exposure",
        json!({
            "Environment": environment(),
            "HeightFog": fog(deck),
            "PostProcessVolume": {"overrides": [
                {"exposure": "manual", "ev100": EV100},
                {"module": "toneMapping", "toneMapping": "neutral"},
                {"module": "bloom", "bloom": 0.16},
            ]},
        }),
    );
    s.commands.extend(light(
        "e_sun",
        "Sun",
        Vec3::new(0.0, 60.0, 30.0),
        json!({"kind": "directional", "color": [1.0, 0.85, 0.66, 1], "intensity": 95_000, "sourceAngle": 0.5357}),
        Some(SUN),
    ));
    // The sun's disc, far along its direction (the sky has no disc of its own). It must not
    // cast: an opaque sphere between a directional light and the course shades the course.
    // The traced path ignored `castShadows: false`, so the disc also sits 2.6 degrees high:
    // its shadow then passes 90 m over the course and lands far behind it.
    let mut disc = models.get("glint_sun").renderer.clone();
    disc["emissive"] = json!([LIT_SUN, LIT_SUN, LIT_SUN]);
    disc["castShadows"] = json!(false);
    s.entity(
        "e_sundisc",
        "Sun disc",
        json!({"Transform": {"position": round3(centre + Vec3::Y * 110.0 - SUN.normalize() * 2400.0), "scale": [52, 52, 52]}, "MeshRenderer": disc}),
    );
    for l in lamps(c) {
        s.commands.extend(lamp(
            &l.id, l.name, l.at, l.color, l.lumens, l.radius, l.range,
        ));
    }
    decor(
        s,
        "e_course",
        "Course",
        "glint_course",
        Vec3::ZERO,
        [0.0; 3],
        Vec3::ONE,
        LIT_COURSE,
    );
    decor(
        s,
        "e_supports",
        "Pylons",
        "glint_supports",
        Vec3::ZERO,
        [0.0; 3],
        Vec3::ONE,
        LIT_PYLONS,
    );
    for (i, r) in c.rings.iter().enumerate() {
        decor(
            s,
            &format!("e_hoop{i}"),
            "Ring",
            "glint_hoop",
            r.at,
            course::euler(r.rot),
            Vec3::splat(r.radius),
            LIT_SET,
        );
    }
    // Checkpoint arches, the cannon, the goal.
    for (i, (at, heading)) in c.checkpoints.iter().enumerate() {
        s.prop(
            Prop::new(
                &format!("e_gate{i}"),
                "Checkpoint arch",
                "glint_gate",
                *at,
                *heading,
            )
            .at_origin()
            .decor()
            .renderer(json!({"emissive": [LIT_SET, LIT_SET, LIT_SET]})),
        );
        if play {
            s.commands.push((
                "component.add",
                json!({"entity": format!("e_gate{i}"), "component": "Checkpoint", "value": {"index": i + 1, "radius": 4.0}}),
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
        .renderer(json!({"emissive": [LIT_SET, LIT_SET, LIT_SET]})),
    );
    s.prop(
        Prop::new("e_goal", "Goal", "glint_goal", c.goal, cannon.heading)
            .at_origin()
            .decor()
            .renderer(json!({"emissive": [LIT_GOAL, LIT_GOAL, LIT_GOAL]})),
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
        let mut components = json!({
            "Transform": {"position": round3(*p)},
            "MeshRenderer": {"model": orb, "emissive": [LIT_ORB, LIT_ORB, LIT_ORB]},
        });
        if play {
            components["Pickup"] = json!({"size": 1.0, "home": round3(*p)});
            components["Spinner"] = json!({"degreesPerSecond": 140, "startYaw": (i * 37 % 360)});
        }
        s.entity(&format!("e_orb{i}"), "Orb", components);
    }
    // Two landmarks on the far half, large enough to read from the launch deck: the great arch
    // over the middle of the banked turn and the gantry over the cannon.
    let bank: Vec<&course::Piece> = c
        .pieces
        .iter()
        .filter(|p| p.kind == course::Kind::Bank)
        .collect();
    let middle = bank[bank.len() / 2];
    let across = middle.rot * Vec3::Z;
    decor(
        s,
        "e_span",
        "Great arch",
        "glint_span",
        middle.center + Vec3::Y * middle.half.y,
        [0.0, across.x.atan2(across.z).to_degrees(), 0.0],
        Vec3::ONE,
        LIT_SET,
    );
    decor(
        s,
        "e_gantry",
        "Cannon gantry",
        "glint_gantry",
        cannon.breech - Vec3::Y * 0.5,
        [0.0, cannon.heading, 0.0],
        Vec3::ONE,
        LIT_SET,
    );
    // The cloud deck: banks spread on a spiral around the course, flattened, each a little
    // different; a few high, thin ones far off.
    let banks = 96u32;
    for i in 0..banks {
        let radius = 760.0 * ((i as f32 + 0.5) / banks as f32).sqrt();
        let angle = i as f32 * 2.399_963;
        let size = 1.7 + scatter(i, 1) * 2.0;
        let at = centre + Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius);
        decor(
            s,
            &format!("e_cloud{i}"),
            "Cloud bank",
            ["glint_cloud0", "glint_cloud1", "glint_cloud2"][i as usize % 3],
            Vec3::new(at.x, deck - 4.0 - scatter(i, 2) * 8.0, at.z),
            [0.0, scatter(i, 3) * 360.0, 0.0],
            // Wide and low: the tops stay within about 20 m of the deck's level.
            Vec3::new(
                size,
                0.6 + scatter(i, 4) * 0.6,
                size * (0.8 + scatter(i, 5) * 0.5),
            ),
            0.0,
        );
    }
    // Nothing far away may put the course in shade: a blocker is dropped if any waypoint's ray
    // to the sun passes through it.
    let to_sun = -SUN.normalize();
    let shades = |at: Vec3, radius: f32| {
        c.waypoints.iter().any(|w| {
            let along = (at - *w).dot(to_sun);
            along > 0.0 && (*w + to_sun * along - at).length() < radius
        })
    };
    let mut high = 0;
    for i in 0..24u32 {
        let angle = i as f32 * 0.628 + 0.3;
        let radius = 620.0 + scatter(i, 6) * 420.0;
        let size = 1.6 + scatter(i, 7) * 1.8;
        let at = centre
            + Vec3::new(
                angle.cos() * radius,
                70.0 + scatter(i, 8) * 120.0,
                angle.sin() * radius,
            );
        if high >= 10 || shades(at, 50.0 * size) {
            continue;
        }
        decor(
            s,
            &format!("e_cirrus{high}"),
            "High cloud",
            ["glint_cloud0", "glint_cloud1", "glint_cloud2"][i as usize % 3],
            at,
            [0.0, scatter(i, 9) * 360.0, 0.0],
            Vec3::new(size * 1.2, size * 0.8, size),
            0.0,
        );
        high += 1;
    }
    // Islands in the distance, kept clear of the route.
    let mut placed = 0;
    for i in 0..40u32 {
        let angle = i as f32 * 2.399_963 + 1.0;
        let radius = 230.0 + scatter(i, 10) * 420.0;
        let at = centre
            + Vec3::new(
                angle.cos() * radius,
                -24.0 + scatter(i, 11) * 70.0,
                angle.sin() * radius,
            );
        let size = 0.8 + scatter(i, 12) * 1.5;
        let clear = c
            .waypoints
            .iter()
            .all(|w| Vec3::new(w.x - at.x, 0.0, w.z - at.z).length() > 110.0 + 40.0 * size);
        if !clear || placed >= 11 || shades(at + Vec3::Y * (20.0 * size), 55.0 * size) {
            continue;
        }
        decor(
            s,
            &format!("e_isle{placed}"),
            "Island",
            "glint_isle",
            at,
            [0.0, scatter(i, 13) * 360.0, 0.0],
            Vec3::splat(size),
            LIT_ISLE,
        );
        placed += 1;
    }
}

/// A review camera: where it stands, what it looks at, its vertical field of view.
pub struct View {
    pub id: &'static str,
    pub name: &'static str,
    pub at: Vec3,
    pub look: Vec3,
    pub fov: f32,
}

/// The review cameras, in route order: the world from behind the start, the turn, the sliders,
/// the cannon's shot, the goal. The comparison's stills and fly-over use the same list.
fn views(c: &Course) -> Vec<View> {
    let (cannon, goal) = (c.cannon.as_ref().expect("cannon"), c.goal);
    let ahead = Quat::from_rotation_y(course::START_HEADING.to_radians()) * Vec3::Z;
    let left = Quat::from_rotation_y(course::START_HEADING.to_radians()) * Vec3::X;
    let turn = c.waypoints[c.waypoints.len() * 3 / 5];
    let shot = Quat::from_rotation_y(cannon.heading.to_radians());
    let sliders = (c.sliders[0].origin + c.sliders[1].origin) * 0.5;
    vec![
        View {
            id: "e_camover",
            name: "Overview",
            at: c.start - ahead * 46.0 + left * 34.0 + Vec3::Y * 30.0,
            look: c.start + ahead * 150.0 - left * 30.0 - Vec3::Y * 10.0,
            fov: 62.0,
        },
        View {
            id: "e_camturn",
            name: "Turn view",
            at: turn + Vec3::new(34.0, 20.0, -44.0),
            look: turn,
            fov: 60.0,
        },
        View {
            id: "e_camsliders",
            name: "Slider view",
            at: sliders + shot * Vec3::new(15.0, 9.0, -6.0),
            look: sliders,
            fov: 60.0,
        },
        View {
            id: "e_camcannon",
            name: "Cannon view",
            at: cannon.breech + shot * Vec3::new(9.0, 6.0, -22.0),
            look: cannon.arc(cannon.flight * 0.5),
            fov: 60.0,
        },
        View {
            id: "e_camgoal",
            name: "Goal view",
            at: goal + shot * Vec3::new(26.0, 7.0, -36.0),
            look: goal + Vec3::new(0.0, 9.0, 0.0),
            fov: 60.0,
        },
    ]
}

/// The gameplay scene.
fn game(ctx: &Ctx, c: &Course) -> Result<()> {
    ctx.new_document(SCENE.0, "scene", SCENE.1)?;
    let models = models(ctx, c)?;
    let mut s = Scene::new(&models);
    art(&mut s, c, true);
    // Invisible collision: the course kit draws the pieces, and the ball finds them by ray.
    for p in &c.pieces {
        let half = p.half.to_array();
        s.entity(
            &p.id,
            p.name,
            json!({
                "Transform": {"position": round3(p.center), "rotation": round3(Vec3::from_array(p.euler()))},
                "RigidBody": {"motion": "static"},
                "Collider": {"halfExtents": half},
            }),
        );
    }
    // The glass rails are solid.
    for w in &c.walls {
        s.entity(
            &w.id,
            "Glass rail",
            json!({"Transform": {"position": round3(w.center), "rotation": round3(Vec3::from_array(course::euler(w.rot)))},
                "RigidBody": {"motion": "static"}, "Collider": {"halfExtents": [w.half.x.max(0.06), w.half.y, w.half.z.max(0.06)]}}),
        );
    }
    // Sliders and bars.
    let mover = models.get("glint_mover").renderer.clone();
    for m in &c.sliders {
        let half = m.half.to_array();
        let mut renderer = mover.clone();
        renderer["emissive"] = json!([LIT_SET, LIT_SET, LIT_SET]);
        s.entity(
            &m.id,
            "Moving platform",
            json!({
                "Transform": {"position": round3(m.origin), "rotation": round3(Vec3::from_array(course::euler(m.rot)))},
                "MeshRenderer": renderer,
                "RigidBody": {"motion": "kinematic"},
                "Collider": {"halfExtents": half},
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
            // No damping: a jump and the cannon's shot follow their ballistic arcs.
            "RigidBody": {"motion": "dynamic", "continuous": true, "allowSleep": false, "linearDamping": 0.0},
            "Collider": {"kind": "sphere", "radius": 0.5, "friction": 0.8},
            "PlayerControl": {"camera": "e_camera"},
            "Ball": {"spawn": c.start.to_array()},
            "BallController": {"topSpeed": course::TOP_SPEED},
        }),
    );
    // The spark trail and the roll loop ride the ball (BallFx sets both from its speed).
    let mut trail = models.get("glint_trail").renderer.clone();
    trail["emissive"] = json!([LIT_ORB, LIT_ORB, LIT_ORB]);
    trail["castShadows"] = json!(false);
    s.entity(
        "e_fx",
        "Ball sparks and roll",
        json!({
            "Transform": {"position": c.start.to_array()},
            "VisualEffect": {"effect": "sparks", "scale": 0.8, "intensity": 0.0},
            // The trail: a lit streak BallFx stretches behind the ball at speed (particles do not
            // draw on every backend; this does).
            "MeshRenderer": trail,
            "AudioSource": {"clip": clip(sounds::CLIPS[0].0, sounds::CLIPS[0].1), "bus": "sfx", "looping": true, "spatial": false, "gain": 0.0},
            "BallFx": {},
        }),
    );
    // Feedback clips are also named by a silent source each, which pins them into the cook.
    for (name, id) in &sounds::CLIPS[1..] {
        s.entity(
            &format!("e_snd{name}"),
            "Sound pin",
            json!({"AudioSource": {"clip": clip(name, id), "bus": "sfx", "spatial": false, "gain": 0.0}}),
        );
    }
    s.entity(
        "e_music",
        "Music",
        json!({"MusicSource": {"music": format!("@{}#{}", MUSIC.0, MUSIC.1)}}),
    );
    s.entity(
        "e_camera",
        "Camera",
        json!({
            "Transform": {"position": round3(c.start + Vec3::new(4.0, 3.0, -4.0))},
            "Camera": {"fov": 56},
            "FollowCamera": {"target": "e_ball", "distance": 5.4, "height": 2.2, "yaw": course::START_HEADING, "pitch": 16, "sensitivity": 0},
            "CourseCamera": {"fullSpeed": course::TOP_SPEED, "speedFov": 14.0},
            // The listener; RunFeedback carries the fall and finish counters where they are heard.
            "AudioListener": {},
            "RunFeedback": {},
        }),
    );
    // Review cameras: a higher priority number keeps the follow camera active.
    for view in views(c) {
        s.entity(view.id, view.name, json!({"Transform": {"position": round3(view.at)}, "Camera": {"fov": view.fov, "priority": 10}}));
        s.commands.push((
            "transform.lookAt",
            json!({"entity": view.id, "point": round3(view.look)}),
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
            json!({"Transform": {"position": round3(m.origin + m.travel * 0.4), "rotation": round3(Vec3::from_array(course::euler(m.rot)))}, "MeshRenderer": mover}),
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
    let ahead = Quat::from_rotation_y(course::START_HEADING.to_radians()) * Vec3::Z;
    let left = Quat::from_rotation_y(course::START_HEADING.to_radians()) * Vec3::X;
    s.entity("e_menuball", "Ball", json!({"Transform": {"position": round3(c.start - Vec3::Y * 0.1)}, "MeshRenderer": ball_look()}));
    // The title view: low, behind and to the left of the ball, looking ahead and to the right,
    // so the ball and the course sit in the right half and the title has the left.
    let eye = c.start - ahead * 7.5 + left * 5.0 + Vec3::Y * 2.0;
    let focus = c.start + ahead * 40.0 - left * 22.0 + Vec3::Y * 4.0;
    s.entity(
        "e_gamemenueye",
        "Menu vista camera",
        json!({"Transform": {"position": round3(eye)}}),
    );
    s.entity(
        "e_gamemenutarget",
        "Menu vista focus",
        json!({"Transform": {"position": round3(focus)}}),
    );
    // The same view as a camera, so `ara capture --screen main-menu` frames the title as the
    // shell does (its backdrop camera sits at the eye, looks at the focus, 54 degrees).
    s.entity(
        "e_menucam",
        "Menu camera",
        json!({"Transform": {"position": round3(eye)}, "Camera": {"fov": 54}}),
    );
    s.commands.push((
        "transform.lookAt",
        json!({"entity": "e_menucam", "point": round3(focus)}),
    ));
    let calls = std::mem::take(&mut s.commands);
    ctx.apply(MENU.0, "Glint: native menu backdrop", calls)
}

/// A clip's `@path#id` reference.
fn clip(name: &str, id: &str) -> String {
    format!("@audio/{name}.wav#{id}")
}

/// Synthesises the sounds and authors their import settings, the mixer and the score: a pad
/// that always plays and a pulse that comes in as the run's intensity rises.
fn audio(ctx: &Ctx) -> Result<()> {
    sounds::write(&ctx.path("audio"))?;
    for (name, id) in sounds::CLIPS {
        ctx.sidecar(
            &format!("audio/{name}.wav"),
            id,
            json!({"usage": "shortEffect", "codec": "pcm"}),
        )?;
    }
    for (name, id) in sounds::STEMS {
        ctx.sidecar(&format!("audio/{name}.wav"), id, json!({"usage": "music"}))?;
    }
    // `clean` does not retire mixers, so the document is created once and then updated.
    if !ctx.path(MIXER.0).is_file() {
        ctx.new_document(MIXER.0, "mixer", MIXER.1)?;
    }
    ctx.apply(
        MIXER.0,
        "Glint: master, music and effects buses",
        ["master", "music", "sfx"]
            .into_iter()
            .map(|name| {
                let output = if name == "master" { "" } else { "master" };
                (
                    "mixer.bus.set",
                    json!({"id": format!("b_{name}"), "bus": {"name": name, "output": output}}),
                )
            })
            .collect(),
    )?;
    ctx.new_document(MUSIC.0, "music", MUSIC.1)?;
    let (bars, bpm) = sounds::LOOP;
    ctx.apply(
        MUSIC.0,
        "Glint: a pad and a pulse that follows the run",
        vec![
            ("music.settings.set", json!({"settings": {"initial": "sky", "intensity": 0.1, "seed": 7}})),
            (
                "music.segment.set",
                json!({"id": "sg_sky", "segment": {"name": "sky", "bars": bars, "tempo": [{"bpm": bpm}], "stems": [
                    {"name": "pad", "clip": clip(sounds::STEMS[0].0, sounds::STEMS[0].1), "gainDb": -9, "looping": true},
                    {"name": "pulse", "clip": clip(sounds::STEMS[1].0, sounds::STEMS[1].1), "gainDb": -11, "intensity": [0.3, 0.8], "looping": true},
                ]}}),
            ),
        ],
    )
}

/// The autopilot's plan as JSON (`replay/route.json`): the steps of a plain, perfect run.
fn route(c: &Course) -> Value {
    let steps: Vec<Value> = c
        .plan
        .iter()
        .map(|step| match step {
            Step::Roll { at, speed } => json!({"kind": "roll", "at": round3(*at), "speed": speed}),
            Step::Jump { at } => json!({"kind": "jump", "at": round3(*at)}),
            Step::Board { slider, reach } => {
                json!({"kind": "board", "slider": slider, "reach": reach})
            }
            Step::Leave { at, reach } => {
                json!({"kind": "leave", "at": round3(*at), "reach": reach})
            }
            Step::Cannon { at } => json!({"kind": "cannon", "at": round3(*at)}),
        })
        .collect();
    json!({"scene": SCENE.0, "start": round3(c.start), "goal": round3(c.goal), "orbs": c.pickups.len(), "steps": steps})
}

/// Builds the Glint project: icons, UI data, sounds, the autopilot's route, the game scene and
/// the menu backdrop.
pub fn build(ctx: &Ctx) -> Result<()> {
    clean(ctx, &["levels", "kit", "audio"])?;
    let c = course::build();
    icons::write(&ctx.path("ui/icons"))?;
    ui::write(
        &ctx.path("ui"),
        c.pickups.len(),
        c.checkpoints.len(),
        MEDALS,
    )?;
    audio(ctx)?;
    let replay = ctx.root.join("replay");
    std::fs::create_dir_all(&replay)?;
    std::fs::write(
        replay.join("route.json"),
        serde_json::to_string_pretty(&route(&c))? + "\n",
    )?;
    let lights: Vec<Value> = lamps(&c)
        .into_iter()
        .map(|l| {
            json!({"id": l.id, "name": l.name, "kind": "point", "position": round3(l.at), "color": l.color, "lumens": l.lumens, "sourceRadius": l.radius, "range": l.range})
        })
        .collect();
    compare::write(
        &ctx.root.join("compare"),
        &views(&c),
        &environment(),
        &fog(deck_level(&c)),
        &lights,
        &json!({"routeChevrons": LIT_COURSE, "pylonBands": LIT_PYLONS, "ringsArchesCannonPlatforms": LIT_SET, "goal": LIT_GOAL,
            "orbs": LIT_ORB, "islands": LIT_ISLE, "sunDisc": LIT_SUN}),
    )?;
    game(ctx, &c)?;
    menu(ctx, &c)?;
    ctx.check()
}
