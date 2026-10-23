use ara_schema::Schema;
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// Four-wheel engine/automatic-gearbox vehicle, using S6's stable defaults.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Vehicle {
    /// Chassis half extents.
    #[ara(default = [0.9,0.2,2],unit = Meters,range(min = 0.001))]
    pub half_extents: [f32; 3],
    /// Chassis mass.
    #[ara(default = 1500,unit = Kilograms,range(min = 0.001))]
    pub mass: f32,
    /// All four wheel radii.
    #[ara(default = 0.35,unit = Meters,range(min = 0.001))]
    pub wheel_radius: f32,
    /// All four wheel widths.
    #[ara(default = 0.2,unit = Meters,range(min = 0.001))]
    pub wheel_width: f32,
    /// Minimum suspension length.
    #[ara(default = 0.2,unit = Meters,range(min = 0))]
    pub suspension_min: f32,
    /// Maximum suspension length.
    #[ara(default = 0.5,unit = Meters,range(min = 0.001))]
    pub suspension_max: f32,
    /// Suspension spring frequency.
    #[ara(default = 1.5,unit = Hertz,range(min = 0.001))]
    pub suspension_frequency: f32,
    /// Suspension damping ratio.
    #[ara(default = 0.5, range(min = 0))]
    pub suspension_damping: f32,
    /// Front wheel steering lock.
    #[ara(default = 30,unit = Degrees,range(min = 0,max = 89))]
    pub max_steer_deg: f32,
    /// Engine peak torque; Jolt's gearbox/engine curves supply the remaining defaults.
    #[ara(default = 500,unit = NewtonMeters,range(min = 0.001))]
    pub engine_max_torque: f32,
    /// Rear-wheel drive; false selects front-wheel drive.
    #[ara(default = true)]
    pub rear_wheel_drive: bool,
    /// Chassis collision layer.
    #[ara(default = 1, range(min = 1, max = 15))]
    pub layer: u32,
}
impl Default for Vehicle {
    fn default() -> Self {
        Self {
            half_extents: [0.9, 0.2, 2.0],
            mass: 1500.0,
            wheel_radius: 0.35,
            wheel_width: 0.2,
            suspension_min: 0.2,
            suspension_max: 0.5,
            suspension_frequency: 1.5,
            suspension_damping: 0.5,
            max_steer_deg: 30.0,
            engine_max_torque: 500.0,
            rear_wheel_drive: true,
            layer: 1,
        }
    }
}

/// Replayable driver intent consumed every fixed tick.
#[derive(Debug, Default, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "physics")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct VehicleInput {
    /// Signed accelerator, with negative input selecting reverse.
    #[ara(range(min = -1,max = 1),replicate(audience = Owner,rollback))]
    pub throttle: f32,
    /// Signed steering, positive turns right.
    #[ara(range(min = -1,max = 1),replicate(audience = Owner,rollback))]
    pub steer: f32,
    /// Service brake.
    #[ara(range(min = 0,max = 1),replicate(audience = Owner,rollback))]
    pub brake: f32,
    /// Rear-wheel handbrake.
    #[ara(range(min = 0,max = 1),replicate(audience = Owner,rollback))]
    pub hand_brake: f32,
}
