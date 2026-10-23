//! Rapier backend: `rapier3d` 0.36.1 used directly (pure Rust, no FFI).

use crate::common::{DT, GRAVITY, Quat, Vec3};
use crate::engine::*;
use rapier3d::control::{
    CharacterAutostep, CharacterLength, DynamicRayCastVehicleController,
    KinematicCharacterController, WheelTuning,
};
use rapier3d::math::Pose;
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::parry::shape::{Capsule, TriMeshFlags};
use rapier3d::prelude::*;
use serde::{Deserialize, Serialize};

/// Game-side character state. Rapier's KinematicCharacterController is stateless, so the
/// state a rollback must restore lives here (in Ara it would be an ECS component).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
struct CharacterBody {
    position: [f32; 3],
    vertical_velocity: f32,
    grounded: bool,
}

struct CharacterConfig {
    controller: KinematicCharacterController,
    shape: Capsule,
    center_offset: f32,
}

pub struct RapierEngine {
    pub world: PhysicsWorld,
    handles: Vec<RigidBodyHandle>,
    configs: Vec<CharacterConfig>,
    characters: Vec<CharacterBody>,
    vehicles: Vec<DynamicRayCastVehicleController>,
    vehicle_chassis: Vec<BodyId>,
}

fn pose(position: Vec3, rotation: Quat) -> Pose {
    Pose::from_parts(position, rotation)
}

impl RapierEngine {
    fn insert(&mut self, body: RigidBodyBuilder, collider: ColliderBuilder) -> BodyId {
        let (h, _) = self.world.insert(body, collider);
        self.handles.push(h);
        (self.handles.len() - 1) as BodyId
    }

    /// Bulk read straight from the RigidBodySet (no FFI): the ECS extraction path.
    pub fn read_into(&self, ids: &[BodyId], out: &mut [BodyState]) {
        for (o, id) in out.iter_mut().zip(ids) {
            let rb = &self.world.bodies[self.handles[*id as usize]];
            *o = BodyState {
                position: rb.translation(),
                rotation: *rb.rotation(),
                linear_velocity: rb.linvel(),
                angular_velocity: rb.angvel(),
                active: !rb.is_sleeping(),
            };
        }
    }
}

type SnapshotRef<'a> = (
    &'a PhysicsWorld,
    &'a [CharacterBody],
    &'a [DynamicRayCastVehicleController],
);
type SnapshotOwned = (
    PhysicsWorld,
    Vec<CharacterBody>,
    Vec<DynamicRayCastVehicleController>,
);

impl Engine for RapierEngine {
    const NAME: &'static str = "rapier";

    fn new(cfg: WorldConfig) -> Self {
        #[allow(unused_mut)]
        let mut world = PhysicsWorld::new();
        world.gravity = GRAVITY;
        world.integration_parameters.dt = DT;
        #[cfg(feature = "parallel")]
        world
            .configure_thread_pool(cfg.threads.max(1))
            .expect("rayon pool");
        #[cfg(not(feature = "parallel"))]
        assert!(
            cfg.threads <= 1,
            "multi-threaded Rapier needs `--features parallel`"
        );
        Self {
            world,
            handles: Vec::new(),
            configs: Vec::new(),
            characters: Vec::new(),
            vehicles: Vec::new(),
            vehicle_chassis: Vec::new(),
        }
    }

    fn add_static_box(&mut self, position: Vec3, rotation: Quat, half: Vec3) -> BodyId {
        self.insert(
            RigidBodyBuilder::fixed().pose(pose(position, rotation)),
            ColliderBuilder::cuboid(half.x, half.y, half.z).friction(0.6),
        )
    }

    fn add_static_mesh(&mut self, vertices: &[Vec3], triangles: &[[u32; 3]]) -> BodyId {
        let collider = ColliderBuilder::trimesh_with_flags(
            vertices.to_vec(),
            triangles.to_vec(),
            TriMeshFlags::FIX_INTERNAL_EDGES,
        )
        .expect("valid trimesh")
        .friction(0.6);
        self.insert(RigidBodyBuilder::fixed(), collider)
    }

    fn add_kinematic_box(&mut self, position: Vec3, half: Vec3) -> BodyId {
        self.insert(
            RigidBodyBuilder::kinematic_velocity_based().translation(position),
            ColliderBuilder::cuboid(half.x, half.y, half.z).friction(0.6),
        )
    }

    fn add_dynamic(&mut self, bodies: &[DynamicDesc]) -> Vec<BodyId> {
        bodies
            .iter()
            .map(|b| {
                let collider = match b.shape {
                    ShapeDesc::Box { half } => ColliderBuilder::cuboid(half.x, half.y, half.z),
                    ShapeDesc::Sphere { radius } => ColliderBuilder::ball(radius),
                };
                self.insert(
                    RigidBodyBuilder::dynamic().pose(pose(b.position, b.rotation)),
                    collider.density(1000.0).friction(b.friction),
                )
            })
            .collect()
    }

    /// Nothing to do. Note: `PhysicsWorld::detect_collisions` would populate the query BVH
    /// before the first step, but in 0.36.1 calling it before the first `step` leaves every
    /// dynamic body frozen (verified in a standalone repro), so queries see an empty world
    /// until tick 1.
    fn finish_setup(&mut self) {}

    /// Rapier `num_solver_iterations` (default 4).
    fn set_solver_iterations(&mut self, n: u32) {
        self.world.integration_parameters.num_solver_iterations = n as usize;
    }

    /// Velocity-based kinematic, like Jolt's MoveKinematic: the velocity needed to reach the
    /// target in one step is set now, so the KCC's platform handling sees this step's velocity
    /// (a position-based body only exposes it after the step, one tick late).
    fn move_kinematic(&mut self, id: BodyId, position: Vec3) {
        let rb = &mut self.world.bodies[self.handles[id as usize]];
        let v = (position - rb.translation()) / DT;
        rb.set_linvel(v, true);
    }

    fn step(&mut self) {
        let w = &mut self.world;
        for v in &mut self.vehicles {
            let filter = QueryFilter::default().exclude_rigid_body(v.chassis);
            let q = w.broad_phase.as_query_pipeline_mut(
                w.narrow_phase.query_dispatcher(),
                &mut w.bodies,
                &mut w.colliders,
                filter,
            );
            v.update_vehicle(DT, q);
        }
        w.step();
    }

    fn read(&self, ids: &[BodyId], out: &mut Vec<BodyState>) {
        out.clear();
        out.resize(ids.len(), BodyState::default());
        self.read_into(ids, out);
    }

    fn active_count(&self) -> usize {
        self.world
            .bodies
            .iter()
            .filter(|(_, rb)| rb.is_dynamic() && !rb.is_sleeping())
            .count()
    }

    fn add_character(&mut self, d: &CharacterDesc) -> u32 {
        let controller = KinematicCharacterController {
            up: Vector::Y,
            offset: CharacterLength::Absolute(d.padding),
            slide: true,
            autostep: Some(CharacterAutostep {
                max_height: CharacterLength::Absolute(d.step_height),
                min_width: CharacterLength::Absolute(0.2),
                include_dynamic_bodies: false,
            }),
            max_slope_climb_angle: d.max_slope_deg.to_radians(),
            min_slope_slide_angle: d.max_slope_deg.to_radians(),
            snap_to_ground: Some(CharacterLength::Absolute(d.stick_down)),
            ..KinematicCharacterController::default()
        };
        self.configs.push(CharacterConfig {
            controller,
            shape: Capsule::new_y(d.half_height, d.radius),
            center_offset: d.half_height + d.radius,
        });
        self.characters.push(CharacterBody {
            position: d.position.to_array(),
            ..Default::default()
        });
        (self.characters.len() - 1) as u32
    }

    /// Gravity plus desired velocity, fed to the KCC as a translation. Riding kinematic
    /// platforms is handled inside `move_shape` ("kinematic friction": the platform's point
    /// velocity is added along and normal to the contact). That friction only applies when the
    /// sweep hits the platform, so a platform moving down and away leaves the rider behind.
    fn update_character(&mut self, c: u32, desired: Vec3) -> CharacterState {
        let cfg = &self.configs[c as usize];
        let ch = &mut self.characters[c as usize];
        let base = if ch.grounded {
            Vec3::ZERO
        } else {
            Vec3::new(0.0, ch.vertical_velocity, 0.0)
        };
        let vel = base + GRAVITY * DT + desired;
        let feet = Vec3::from(ch.position);
        let center = Pose::from_translation(feet + Vec3::Y * cfg.center_offset);
        let qp = self.world.query_pipeline();
        let mv = cfg
            .controller
            .move_shape(DT, &qp, &cfg.shape, &center, vel * DT, |_| {});
        ch.position = (feet + mv.translation).to_array();
        ch.grounded = mv.grounded;
        ch.vertical_velocity = if mv.grounded {
            0.0
        } else {
            mv.translation.y / DT
        };
        self.character_state(c)
    }

    fn character_state(&self, c: u32) -> CharacterState {
        let ch = &self.characters[c as usize];
        CharacterState {
            position: Vec3::from(ch.position),
            velocity: Vec3::new(0.0, ch.vertical_velocity, 0.0),
            grounded: ch.grounded,
            ground_velocity: Vec3::ZERO,
        }
    }

    fn add_vehicle(&mut self, d: &VehicleDesc) -> u32 {
        let h = d.half_extents;
        // Lower the centre of mass to the bottom of the chassis box, like Jolt's
        // OffsetCenterOfMassShape in the shim (parallel-axis term on X and Z).
        let m = d.mass;
        let inertia = Vector::new(
            m / 3.0 * (h.y * h.y + h.z * h.z) + m * h.y * h.y,
            m / 3.0 * (h.x * h.x + h.z * h.z),
            m / 3.0 * (h.x * h.x + h.y * h.y) + m * h.y * h.y,
        );
        let props = MassProperties::new(Vector::new(0.0, -h.y, 0.0), m, inertia);
        let chassis = self.insert(
            RigidBodyBuilder::dynamic()
                .translation(d.position)
                .can_sleep(false),
            ColliderBuilder::cuboid(h.x, h.y, h.z).mass_properties(props),
        );
        let handle = self.handles[chassis as usize];
        // Rapier's spring force is stiffness * compression * chassis mass per wheel, so a
        // per-wheel stiffness of omega^2 / 4 gives the same natural frequency as Jolt's.
        let omega = 2.0 * std::f32::consts::PI * d.suspension_frequency;
        let tuning = WheelTuning {
            suspension_stiffness: omega * omega / 4.0,
            suspension_compression: 2.0 * d.suspension_damping_ratio * omega / 4.0,
            suspension_damping: 2.0 * d.suspension_damping_ratio * omega / 4.0,
            max_suspension_travel: d.suspension_travel,
            max_suspension_force: 3.0 * m * 9.81,
            // Default 10.5 (Bullet's) is ~10 g of grip and rolls the car in any real turn;
            // 1.2 is a road-tyre friction coefficient.
            friction_slip: 1.2,
            ..WheelTuning::default()
        };
        let mut vc = DynamicRayCastVehicleController::new(handle);
        vc.index_up_axis = 1;
        vc.index_forward_axis = 2;
        for offset in d.wheel_offsets {
            vc.add_wheel(
                offset,
                -Vector::Y,
                -Vector::X,
                d.suspension_rest,
                d.wheel_radius,
                &tuning,
            );
        }
        self.vehicles.push(vc);
        self.vehicle_chassis.push(chassis);
        (self.vehicles.len() - 1) as u32
    }

    /// No engine/gearbox model in Rapier: throttle maps to a wheel force on the rear axle,
    /// steering to a wheel angle on the front axle.
    fn drive(&mut self, vehicle: u32, forward: f32, right: f32) {
        const ENGINE_FORCE: f32 = 3000.0;
        const MAX_STEER: f32 = 30.0;
        let wheels = self.vehicles[vehicle as usize].wheels_mut();
        for (i, w) in wheels.iter_mut().enumerate() {
            if i < 2 {
                w.steering = -right * MAX_STEER.to_radians();
            } else {
                w.engine_force = forward * ENGINE_FORCE;
            }
        }
    }

    fn vehicle_state(&self, vehicle: u32) -> VehicleState {
        let vc = &self.vehicles[vehicle as usize];
        let rb = &self.world.bodies[vc.chassis];
        VehicleState {
            chassis: BodyState {
                position: rb.translation(),
                rotation: *rb.rotation(),
                linear_velocity: rb.linvel(),
                angular_velocity: rb.angvel(),
                active: !rb.is_sleeping(),
            },
            wheels_in_contact: vc
                .wheels()
                .iter()
                .filter(|w| w.raycast_info().is_in_contact)
                .count() as u32,
        }
    }

    fn vehicle_chassis(&self, vehicle: u32) -> BodyId {
        self.vehicle_chassis[vehicle as usize]
    }

    fn save(&mut self) -> Vec<u8> {
        let snap: SnapshotRef = (&self.world, &self.characters, &self.vehicles);
        bincode::serialize(&snap).expect("snapshot serialization")
    }

    /// Restores into the live pipeline (keeps the configured thread pool), as Rapier's own
    /// `snapshot_roundtrip` test does with `keep_pipeline`.
    fn restore(&mut self, bytes: &[u8]) {
        let (mut world, characters, vehicles): SnapshotOwned =
            bincode::deserialize(bytes).expect("snapshot deserialization");
        std::mem::swap(
            &mut world.physics_pipeline,
            &mut self.world.physics_pipeline,
        );
        self.world = world;
        self.characters = characters;
        self.vehicles = vehicles;
    }

    fn cast_ray(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<Hit> {
        let ray = Ray::new(origin, direction);
        self.world
            .query_pipeline()
            .cast_ray_and_get_normal(&ray, max_distance, true)
            .map(|(_, hit)| Hit {
                distance: hit.time_of_impact,
                normal: hit.normal,
            })
    }

    fn cast_sphere(
        &self,
        radius: f32,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Option<Hit> {
        let ball = Ball::new(radius);
        let opts = ShapeCastOptions::with_max_time_of_impact(max_distance);
        self.world
            .query_pipeline()
            .cast_shape(&Pose::from_translation(origin), direction, &ball, opts)
            .map(|(_, hit)| Hit {
                distance: hit.time_of_impact,
                normal: hit.normal1,
            })
    }

    fn body_position(&self, id: BodyId) -> Vec3 {
        self.world.bodies[self.handles[id as usize]].translation()
    }
}
