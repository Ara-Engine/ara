use ara_schema::Schema;
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// Joint primitive implemented by the backend.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "physics")]
#[serde(rename_all = "camelCase")]
pub enum JointKind {
    /// Locks all relative motion.
    #[default]
    Fixed,
    /// Rotation around one axis.
    Hinge,
    /// Translation along one axis.
    Slider,
    /// Separation constrained to a distance interval.
    Distance,
    /// Coincident anchors with free rotation.
    Point,
    /// Independent translation and angular limits on three axes each.
    SixDof,
}

/// Joint on a body; connection zero anchors to the world. Anchors/axes are world-space.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Joint {
    /// Constraint primitive.
    pub kind: JointKind,
    /// PhysicsId of the connected body; zero is the world.
    pub connected_body: u64,
    /// Anchor on the connected body, or the world anchor.
    #[ara(unit = Meters)]
    pub anchor_a: [f32; 3],
    /// Anchor on this entity's body.
    #[ara(unit = Meters)]
    pub anchor_b: [f32; 3],
    /// Unit primary axis (hinge direction, slider direction, or frame X).
    #[ara(default = [1,0,0])]
    pub axis: [f32; 3],
    /// Unit perpendicular normal (frame Y).
    #[ara(default = [0,1,0])]
    pub normal: [f32; 3],
    /// Minimum XYZ translation; X is the slider/distance lower bound.
    #[ara(unit = Meters)]
    pub translation_min: [f32; 3],
    /// Maximum XYZ translation; X is the slider/distance upper bound.
    #[ara(unit = Meters)]
    pub translation_max: [f32; 3],
    /// Minimum XYZ angular limits; X is the hinge lower bound.
    #[ara(unit = Radians,range(min = -3.1415928,max = 3.1415928))]
    pub rotation_min: [f32; 3],
    /// Maximum XYZ angular limits; X is the hinge upper bound.
    #[ara(unit = Radians,range(min = -3.1415928,max = 3.1415928))]
    pub rotation_max: [f32; 3],
}
impl Default for Joint {
    fn default() -> Self {
        Self {
            kind: JointKind::Fixed,
            connected_body: 0,
            anchor_a: [0.0; 3],
            anchor_b: [0.0; 3],
            axis: [1.0, 0.0, 0.0],
            normal: [0.0, 1.0, 0.0],
            translation_min: [0.0; 3],
            translation_max: [0.0; 3],
            rotation_min: [0.0; 3],
            rotation_max: [0.0; 3],
        }
    }
}
