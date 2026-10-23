//! Reflected gameplay state; backend handles stay out of persistent ECS values.
use ara_schema::Schema;
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// Character support and stance mirrored at every fixed boundary.
#[derive(Debug, Default, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CharacterMotion {
    /// Walkable ground support.
    #[ara(replicate(audience=Owner,rollback))]
    pub grounded: bool,
    /// Actual stance; a ceiling can prevent standing.
    #[ara(replicate(audience=Owner,rollback))]
    pub crouched: bool,
    /// Platform contact velocity.
    #[ara(unit=MetersPerSecond,replicate(audience=Owner,rollback))]
    pub ground_velocity: [f32; 3],
    /// Supporting surface normal.
    #[ara(replicate(audience=Owner,rollback))]
    pub ground_normal: [f32; 3],
}

/// Vehicle drivetrain and suspension state mirrored into ECS after each tick.
#[derive(Debug, Default, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct VehicleTelemetry {
    /// Grounded wheel count.
    #[ara(range(min=0,max=4),replicate(audience=Relevant,rollback))]
    pub wheels_in_contact: u32,
    /// Current engine speed in revolutions per minute.
    #[ara(range(min=0),replicate(audience=Relevant,rollback))]
    pub engine_rpm: f32,
    /// Gear index (negative is reverse).
    #[ara(replicate(audience=Relevant,rollback))]
    pub gear: i32,
}
