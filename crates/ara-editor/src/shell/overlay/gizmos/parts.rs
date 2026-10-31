//! What gizmos an entity record has: one part per gizmo-bearing component and the one
//! icon it shows. Components read through their JSON with the schema defaults, so a
//! partial record draws what the engine runs.
use super::*;
use ara_physics::{Collider, ColliderKind};
use ara_runtime::components::{Camera as CameraComponent, Light, LightKind};
use serde::Deserialize;
use serde_json::Value;

/// What an icon is tinted with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Tint {
    /// The light's own colour (display RGB, brightest channel 1).
    Light([f32; 3]),
    Camera,
    Audio,
    Probe,
    Volume,
    Effect,
}
/// The billboard an entity shows.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Icon {
    /// Gizmo type key (`TYPES`).
    pub kind: &'static str,
    /// Lucide glyph name.
    pub glyph: &'static str,
    pub tint: Tint,
}
/// One component's wire gizmo.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Part {
    /// Gizmo type key (`TYPES`).
    pub kind: &'static str,
    pub shape: Shape,
}
/// Primitive collider volumes, in the collider's local frame around `centre`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Volume {
    Box(Vec3),
    Sphere(f32),
    /// Y-axis capsule: cylinder half height and radius.
    Capsule(f32, f32),
    /// Y-axis cylinder: half height and radius.
    Cylinder(f32, f32),
}
/// Joint primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Constraint {
    Fixed,
    Hinge,
    Slider,
    Distance,
    Point,
    SixDof,
}
/// A gizmo shape in its entity's frame (lights, cameras and portals face local -Z).
/// Angles are half-angles in radians; nav links and surfaces are world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Shape {
    /// Point and spot lights also outline their emitter: a sphere of `radius`, or a
    /// tube `length` long along local X.
    PointLight {
        range: f32,
        radius: f32,
        length: f32,
        color: [f32; 3],
    },
    SpotLight {
        range: f32,
        inner: f32,
        outer: f32,
        radius: f32,
        length: f32,
        color: [f32; 3],
    },
    /// A rect (`width` along local X, `height` along local Y) emitting along -Z, its
    /// barn doors (`doors`: angle from the normal in radians, length) and reach.
    RectLight {
        range: f32,
        width: f32,
        height: f32,
        doors: (f32, f32),
        color: [f32; 3],
    },
    /// A disc of `radius` emitting along -Z, and its reach.
    DiscLight {
        range: f32,
        radius: f32,
        color: [f32; 3],
    },
    DirectionalLight {
        color: [f32; 3],
    },
    Camera {
        fov: f32,
    },
    /// Audio ranges (reference and maximum distance) and an optional cone.
    Audio {
        min: f32,
        max: f32,
        cone: Option<(f32, f32)>,
    },
    ReverbBox {
        half: Vec3,
        fade: f32,
    },
    ReverbSphere {
        radius: f32,
        fade: f32,
    },
    Portal {
        width: f32,
        height: f32,
    },
    Collider {
        volume: Volume,
        centre: Vec3,
        sensor: bool,
    },
    /// Character capsule standing on the entity origin.
    Character {
        radius: f32,
        height: f32,
    },
    Joint {
        kind: Constraint,
        anchor: Vec3,
        axis: Vec3,
        normal: Vec3,
        /// Hinge angles or slider offsets along `axis`.
        limits: (f32, f32),
    },
    NavAgent {
        radius: f32,
        height: f32,
    },
    NavObstacle {
        half: Vec3,
    },
    NavLink {
        start: Vec3,
        end: Vec3,
        radius: f32,
        arc: f32,
        both: bool,
    },
    NavSurface {
        min: Vec3,
        max: Vec3,
    },
    /// Sight wedge and peripheral circle at eye height.
    Perceiver {
        sight: f32,
        fov: f32,
        peripheral: f32,
        eye: f32,
    },
}

fn num(component: &Value, key: &str, default: f32) -> f32 {
    component
        .get(key)
        .and_then(Value::as_f64)
        .map_or(default, |v| v as f32)
        .clamp(0., 1e6)
}
fn signed(component: &Value, key: &str, default: f32) -> f32 {
    component
        .get(key)
        .and_then(Value::as_f64)
        .map_or(default, |v| v as f32)
}
fn vec3(component: &Value, key: &str, default: [f32; 3]) -> Vec3 {
    let v = component.get(key).and_then(Value::as_array);
    let at = |n: usize| {
        v.and_then(|v| v.get(n))
            .and_then(Value::as_f64)
            .map_or(default[n], |v| v as f32)
    };
    Vec3::new(at(0), at(1), at(2))
}
fn text<'a>(component: &'a Value, key: &str) -> Option<&'a str> {
    component.get(key).and_then(Value::as_str)
}
/// A linear light colour as a display tint: brightest channel 1, sRGB-encoded.
pub(super) fn display(linear: [f32; 4]) -> [f32; 3] {
    let rgb = Vec3::new(linear[0], linear[1], linear[2]).max(Vec3::ZERO);
    let peak = rgb.max_element();
    if !peak.is_finite() || peak <= 0. {
        return [1.; 3];
    }
    (rgb / peak).to_array().map(|c| c.powf(1. / 2.2))
}

/// The record's gizmo parts and its icon (the first iconed component wins).
pub(super) fn classify(record: &ara_doc::Record) -> (Vec<Part>, Option<Icon>) {
    let mut parts = Vec::new();
    let mut icons = Vec::new();
    let mut part = |kind, shape| parts.push(Part { kind, shape });
    if let Some(light) = record.get("Light") {
        let light = Light::deserialize(light).unwrap_or_default();
        let color = display(light.color.0);
        let (glyph, shape) = match light.kind {
            LightKind::Point => (
                "lightbulb",
                Shape::PointLight {
                    range: light.range,
                    radius: light.source_radius,
                    length: light.source_length,
                    color,
                },
            ),
            LightKind::Spot => (
                "flashlight",
                Shape::SpotLight {
                    range: light.range,
                    inner: light.inner_angle.min(light.outer_angle).to_radians(),
                    outer: light.outer_angle.to_radians(),
                    radius: light.source_radius,
                    length: light.source_length,
                    color,
                },
            ),
            LightKind::Directional => ("sun", Shape::DirectionalLight { color }),
            LightKind::Rect => (
                "square",
                Shape::RectLight {
                    range: light.range,
                    width: light.source_width,
                    height: light.source_height,
                    doors: (
                        light.barn_door_angle.clamp(0., 90.).to_radians(),
                        light.barn_door_length.max(0.),
                    ),
                    color,
                },
            ),
            LightKind::Disc => (
                "circle",
                Shape::DiscLight {
                    range: light.range,
                    radius: light.source_radius,
                    color,
                },
            ),
        };
        part("Light", shape);
        icons.push(("Light", glyph, Tint::Light(color)));
    }
    if let Some(camera) = record.get("Camera") {
        let camera = CameraComponent::deserialize(camera).unwrap_or_default();
        part(
            "Camera",
            Shape::Camera {
                fov: (camera.fov.clamp(1., 170.) * 0.5).to_radians(),
            },
        );
        icons.push(("Camera", "video", Tint::Camera));
    } else if record.contains_key("FollowCamera") {
        icons.push(("Camera", "video", Tint::Camera));
    }
    for (component, glyph) in [
        ("AudioSource", "volume-2"),
        ("MusicSource", "audio-lines"),
        ("AudioGraphSource", "volume-2"),
        ("AcousticSource", "volume-2"),
    ] {
        let Some(source) = record.get(component) else {
            continue;
        };
        if component == "AudioSource" {
            let min = num(source, "referenceDistance", 1.);
            let max = num(source, "maxDistance", 100.);
            let (inner, outer) = (
                num(source, "coneInner", 360.),
                num(source, "coneOuter", 360.),
            );
            let cone = (outer < 360.).then(|| {
                (
                    (inner.min(outer) * 0.5).to_radians(),
                    (outer * 0.5).to_radians(),
                )
            });
            if source.get("spatial").and_then(Value::as_bool) != Some(false) {
                part("AudioSource", Shape::Audio { min, max, cone });
            }
        }
        icons.push(("AudioSource", glyph, Tint::Audio));
        break;
    }
    if record.contains_key("AudioListener") {
        icons.push(("AudioListener", "ear", Tint::Audio));
    }
    if let Some(zone) = record.get("ReverbZone") {
        let fade = num(zone, "fade", 1.);
        part(
            "ReverbZone",
            if text(zone, "shape") == Some("sphere") {
                Shape::ReverbSphere {
                    radius: num(zone, "radius", 5.),
                    fade,
                }
            } else {
                Shape::ReverbBox {
                    half: vec3(zone, "halfExtents", [5., 3., 5.]).abs(),
                    fade,
                }
            },
        );
        icons.push(("ReverbZone", "audio-lines", Tint::Audio));
    }
    if let Some(portal) = record.get("AcousticPortal") {
        part(
            "AcousticPortal",
            Shape::Portal {
                width: num(portal, "width", 1.),
                height: num(portal, "height", 2.),
            },
        );
    }
    if record.contains_key("ReflectionProbe") {
        icons.push(("ReflectionProbe", "circle-dot", Tint::Probe));
    }
    if record.contains_key("PostProcessVolume") {
        icons.push(("PostProcessVolume", "layers", Tint::Volume));
    }
    for (component, glyph) in [
        ("HeightFog", "cloud-fog"),
        ("SkyAtmosphere", "sun-dim"),
        ("Environment", "globe"),
    ] {
        if record.contains_key(component) {
            icons.push(("HeightFog", glyph, Tint::Volume));
            break;
        }
    }
    if record.contains_key("VisualEffect") {
        icons.push(("VisualEffect", "sparkles", Tint::Effect));
    }
    if let Some(collider) = record.get("Collider")
        && let Ok(collider) = Collider::deserialize(collider)
    {
        let volume = match collider.kind {
            ColliderKind::Box => Some(Volume::Box(Vec3::from_array(collider.half_extents).abs())),
            ColliderKind::Sphere => Some(Volume::Sphere(collider.radius)),
            ColliderKind::Capsule => Some(Volume::Capsule(collider.half_height, collider.radius)),
            ColliderKind::Cylinder => Some(Volume::Cylinder(collider.half_height, collider.radius)),
            // Cooked and model shapes have no authored primitive to draw.
            ColliderKind::Cooked | ColliderKind::ModelConvex | ColliderKind::ModelMesh => None,
        };
        let centre = Vec3::from_array(collider.center);
        if let Some(volume) = volume
            && centre.is_finite()
        {
            part(
                "Collider",
                Shape::Collider {
                    volume,
                    centre,
                    sensor: collider.sensor,
                },
            );
        }
    }
    if let Some(character) = record.get("CharacterController") {
        part(
            "CharacterController",
            Shape::Character {
                radius: num(character, "radius", 0.3),
                height: num(character, "height", 1.8),
            },
        );
    }
    if let Some(joint) = record.get("Joint") {
        let kind = match text(joint, "kind").unwrap_or("fixed") {
            "hinge" => Constraint::Hinge,
            "slider" => Constraint::Slider,
            "distance" => Constraint::Distance,
            "point" => Constraint::Point,
            "sixDof" => Constraint::SixDof,
            _ => Constraint::Fixed,
        };
        let index = |key: &str| {
            joint
                .get(key)
                .and_then(Value::as_array)
                .and_then(|v| v.first())
                .and_then(Value::as_f64)
                .unwrap_or(0.) as f32
        };
        let limits = match kind {
            Constraint::Hinge => (index("rotationMin"), index("rotationMax")),
            _ => (index("translationMin"), index("translationMax")),
        };
        part(
            "Joint",
            Shape::Joint {
                kind,
                anchor: vec3(joint, "anchorB", [0.; 3]),
                axis: vec3(joint, "axis", [1., 0., 0.]).normalize_or(Vec3::X),
                normal: vec3(joint, "normal", [0., 1., 0.]).normalize_or(Vec3::Y),
                limits,
            },
        );
    }
    if let Some(agent) = record.get("NavAgent") {
        part(
            "NavAgent",
            Shape::NavAgent {
                radius: num(agent, "radius", 0.3),
                height: num(agent, "height", 1.8),
            },
        );
    }
    if let Some(obstacle) = record.get("NavObstacle") {
        part(
            "NavObstacle",
            Shape::NavObstacle {
                half: vec3(obstacle, "halfExtents", [0.5; 3]).abs(),
            },
        );
    }
    if let Some(link) = record.get("NavLink") {
        let (start, end) = (vec3(link, "start", [0.; 3]), vec3(link, "end", [0.; 3]));
        if start != end {
            part(
                "NavLink",
                Shape::NavLink {
                    start,
                    end,
                    radius: num(link, "radius", 0.5),
                    arc: signed(link, "arcHeight", 0.),
                    both: link.get("bidirectional").and_then(Value::as_bool) != Some(false),
                },
            );
        }
    }
    if let Some(surface) = record.get("NavMeshSurface") {
        let (min, max) = (vec3(surface, "min", [0.; 3]), vec3(surface, "max", [0.; 3]));
        if (max - min).min_element() > 0. {
            part("NavMeshSurface", Shape::NavSurface { min, max });
        }
    }
    if let Some(perceiver) = record.get("Perceiver") {
        part(
            "Perceiver",
            Shape::Perceiver {
                sight: num(perceiver, "sightRange", 25.),
                fov: (num(perceiver, "fov", 120.).min(360.) * 0.5).to_radians(),
                peripheral: num(perceiver, "peripheralRange", 2.),
                eye: num(perceiver, "eyeHeight", 1.7),
            },
        );
    }
    let icon = icons
        .into_iter()
        .next()
        .map(|(kind, glyph, tint)| Icon { kind, glyph, tint });
    (parts, icon)
}
