use ara_schema::Schema;
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// Predicted capsule movement: slopes, stairs, moving platforms, crouching and pushing.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CharacterController {
    /// Standing capsule radius.
    #[ara(default = 0.3,unit = Meters,range(min = 0.001))]
    pub radius: f32,
    /// Complete standing height; must exceed twice the radius.
    #[ara(default = 1.8,unit = Meters,range(min = 0.001))]
    pub height: f32,
    /// Complete crouched height, between diameter and standing height.
    #[ara(default = 1.0,unit = Meters,range(min = 0.001))]
    pub crouch_height: f32,
    /// Maximum walkable slope angle.
    #[ara(default = 45,unit = Degrees,range(min = 0.1,max = 89.9))]
    pub max_slope_deg: f32,
    /// Maximum stair step height.
    #[ara(default = 0.35,unit = Meters,range(min = 0))]
    pub step_height: f32,
    /// Distance searched below feet to stay on a descending surface.
    #[ara(default = 0.5,unit = Meters,range(min = 0))]
    pub stick_down: f32,
    /// Capsule collision padding.
    #[ara(default = 0.02,unit = Meters,range(min = 0))]
    pub padding: f32,
    /// Maximum force used to push rigid bodies.
    #[ara(default = 1000,unit = Newtons,range(min = 0))]
    pub max_strength: f32,
    /// Character mass used for dynamic body interaction.
    #[ara(default = 70,unit = Kilograms,range(min = 0.001))]
    pub mass: f32,
    /// Collision group used for movement queries.
    #[ara(default = 1, range(min = 1, max = 15))]
    pub layer: u32,
}
impl Default for CharacterController {
    fn default() -> Self {
        Self {
            radius: 0.3,
            height: 1.8,
            crouch_height: 1.0,
            max_slope_deg: 45.0,
            step_height: 0.35,
            stick_down: 0.5,
            padding: 0.02,
            max_strength: 1000.0,
            mass: 70.0,
            layer: 1,
        }
    }
}

/// Per-tick character intent: replay this input exactly once before the rigid-body step.
#[derive(Debug, Default, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CharacterInput {
    /// Desired world-space horizontal velocity; Y must be zero.
    #[ara(unit = MetersPerSecond,replicate(audience = Owner,rollback))]
    pub desired_velocity: [f32; 3],
    /// Jump launch speed; zero means no jump this tick.
    #[ara(unit = MetersPerSecond,range(min = 0),replicate(audience = Owner,rollback))]
    pub jump_speed: f32,
    /// Requested crouch; standing may be blocked by clearance.
    #[ara(replicate(audience = Owner,rollback))]
    pub crouch: bool,
}
