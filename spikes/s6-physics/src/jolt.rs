//! Jolt backend: Ara's own C shim (`ara-jolt-sys`) over vendored Jolt 5.6.0.

use crate::common::{DT, GRAVITY, Quat, Vec3};
use crate::engine::*;
use ara_jolt_sys as sys;
use std::collections::HashMap;
use std::ptr::NonNull;

fn v(a: Vec3) -> sys::AraVec3 {
    sys::AraVec3 {
        x: a.x,
        y: a.y,
        z: a.z,
    }
}
fn q(a: Quat) -> sys::AraQuat {
    sys::AraQuat {
        x: a.x,
        y: a.y,
        z: a.z,
        w: a.w,
    }
}
fn gv(a: sys::AraVec3) -> Vec3 {
    Vec3::new(a.x, a.y, a.z)
}
fn gq(a: sys::AraQuat) -> Quat {
    Quat::from_xyzw(a.x, a.y, a.z, a.w)
}

struct Character {
    step_up: f32,
    stick_down: f32,
}

/// Owns one `AraWorld`. `BodyId` is Jolt's raw BodyID, so reads pass ids straight through.
pub struct JoltEngine {
    world: NonNull<sys::AraWorld>,
    box_shapes: HashMap<[u32; 3], u32>,
    sphere_shapes: HashMap<u32, u32>,
    characters: Vec<Character>,
    vehicles: Vec<u32>,
    /// OR of every step's EPhysicsUpdateError bits (capacity overflows drop contacts).
    step_errors: u32,
}

impl Drop for JoltEngine {
    fn drop(&mut self) {
        // SAFETY: the world was created by ara_world_create and is destroyed exactly once.
        unsafe { sys::ara_world_destroy(self.world.as_ptr()) }
    }
}

impl JoltEngine {
    fn w(&self) -> *mut sys::AraWorld {
        self.world.as_ptr()
    }

    fn box_shape(&mut self, half: Vec3) -> u32 {
        let key = [half.x.to_bits(), half.y.to_bits(), half.z.to_bits()];
        let w = self.w();
        // SAFETY: live world.
        *self
            .box_shapes
            .entry(key)
            .or_insert_with(|| unsafe { sys::ara_shape_box(w, v(half)) })
    }

    fn sphere_shape(&mut self, radius: f32) -> u32 {
        let w = self.w();
        // SAFETY: live world.
        *self
            .sphere_shapes
            .entry(radius.to_bits())
            .or_insert_with(|| unsafe { sys::ara_shape_sphere(w, radius) })
    }

    fn add(&mut self, descs: &[sys::AraBodyDesc]) -> Vec<BodyId> {
        let mut ids = vec![0u32; descs.len()];
        // SAFETY: `ids` has room for every descriptor.
        let n = unsafe {
            sys::ara_bodies_add(
                self.w(),
                descs.as_ptr(),
                descs.len() as u32,
                ids.as_mut_ptr(),
            )
        };
        assert_eq!(n as usize, descs.len(), "Jolt ran out of bodies");
        ids
    }

    /// Raw bulk read into the C struct: the zero-conversion ECS extraction path.
    pub fn read_raw(&self, ids: &[BodyId], out: &mut [sys::AraBodyState]) {
        assert!(out.len() >= ids.len());
        // SAFETY: `out` holds at least `ids.len()` entries.
        unsafe { sys::ara_bodies_read(self.w(), ids.as_ptr(), ids.len() as u32, out.as_mut_ptr()) }
    }

    /// Empty FFI round trip, for overhead measurement.
    pub fn noop(&self, x: u32) -> u32 {
        // SAFETY: pure function.
        unsafe { sys::ara_noop(x) }
    }
}

fn body_desc(
    shape: u32,
    position: Vec3,
    rotation: Quat,
    motion: u8,
    friction: f32,
) -> sys::AraBodyDesc {
    sys::AraBodyDesc {
        shape,
        position: v(position),
        rotation: q(rotation),
        motion,
        layer: if motion == sys::ARA_MOTION_STATIC {
            sys::ARA_LAYER_STATIC
        } else {
            sys::ARA_LAYER_MOVING
        },
        allow_sleep: 1,
        _pad: 0,
        friction,
        restitution: 0.0,
        mass: 0.0,
        linear_velocity: sys::AraVec3::default(),
    }
}

impl Engine for JoltEngine {
    const NAME: &'static str = "jolt";

    fn new(cfg: WorldConfig) -> Self {
        let desc = sys::AraWorldDesc {
            max_bodies: cfg.max_bodies,
            max_body_pairs: (cfg.max_bodies * 8).max(4096),
            max_contact_constraints: (cfg.max_bodies * 8).max(4096),
            threads: cfg.threads as u32,
            temp_allocator_mb: 64 + cfg.max_bodies / 100,
            gravity: v(GRAVITY),
        };
        // SAFETY: desc is a valid, fully initialised struct.
        let world = NonNull::new(unsafe { sys::ara_world_create(&desc) })
            .expect("ara_world_create returned null");
        Self {
            world,
            box_shapes: HashMap::new(),
            sphere_shapes: HashMap::new(),
            characters: Vec::new(),
            vehicles: Vec::new(),
            step_errors: 0,
        }
    }

    fn add_static_box(&mut self, position: Vec3, rotation: Quat, half: Vec3) -> BodyId {
        let shape = self.box_shape(half);
        self.add(&[body_desc(
            shape,
            position,
            rotation,
            sys::ARA_MOTION_STATIC,
            0.6,
        )])[0]
    }

    fn add_static_mesh(&mut self, vertices: &[Vec3], triangles: &[[u32; 3]]) -> BodyId {
        let verts: Vec<sys::AraVec3> = vertices.iter().map(|p| v(*p)).collect();
        let idx: Vec<u32> = triangles.iter().flatten().copied().collect();
        // SAFETY: the buffers outlive the call; lengths match.
        let shape = unsafe {
            sys::ara_shape_mesh(
                self.w(),
                verts.as_ptr(),
                verts.len() as u32,
                idx.as_ptr(),
                triangles.len() as u32,
            )
        };
        assert_ne!(shape, sys::ARA_INVALID, "Jolt rejected the mesh");
        self.add(&[body_desc(
            shape,
            Vec3::ZERO,
            Quat::IDENTITY,
            sys::ARA_MOTION_STATIC,
            0.6,
        )])[0]
    }

    fn add_kinematic_box(&mut self, position: Vec3, half: Vec3) -> BodyId {
        let shape = self.box_shape(half);
        let mut d = body_desc(
            shape,
            position,
            Quat::IDENTITY,
            sys::ARA_MOTION_KINEMATIC,
            0.6,
        );
        d.allow_sleep = 0;
        self.add(&[d])[0]
    }

    fn add_dynamic(&mut self, bodies: &[DynamicDesc]) -> Vec<BodyId> {
        let descs: Vec<sys::AraBodyDesc> = bodies
            .iter()
            .map(|b| {
                let shape = match b.shape {
                    ShapeDesc::Box { half } => self.box_shape(half),
                    ShapeDesc::Sphere { radius } => self.sphere_shape(radius),
                };
                body_desc(
                    shape,
                    b.position,
                    b.rotation,
                    sys::ARA_MOTION_DYNAMIC,
                    b.friction,
                )
            })
            .collect();
        self.add(&descs)
    }

    fn finish_setup(&mut self) {
        // SAFETY: live world.
        unsafe { sys::ara_world_optimize_broadphase(self.w()) }
    }

    /// Jolt velocity steps (default 10); position steps stay at the default 2.
    fn set_solver_iterations(&mut self, n: u32) {
        // SAFETY: live world.
        unsafe { sys::ara_world_set_solver_steps(self.w(), n, 2) }
    }

    fn move_kinematic(&mut self, id: BodyId, position: Vec3) {
        // SAFETY: id came from this world.
        unsafe { sys::ara_body_move_kinematic(self.w(), id, v(position), q(Quat::IDENTITY), DT) }
    }

    fn step(&mut self) {
        // SAFETY: live world.
        self.step_errors |= unsafe { sys::ara_world_step(self.w(), DT, 1) };
    }

    fn read(&self, ids: &[BodyId], out: &mut Vec<BodyState>) {
        let mut raw = vec![sys::AraBodyState::default(); ids.len()];
        self.read_raw(ids, &mut raw);
        out.clear();
        out.extend(raw.iter().map(|s| BodyState {
            position: gv(s.position),
            rotation: gq(s.rotation),
            linear_velocity: gv(s.linear_velocity),
            angular_velocity: gv(s.angular_velocity),
            active: s.active != 0,
        }));
    }

    fn step_errors(&self) -> u32 {
        self.step_errors
    }

    fn active_count(&self) -> usize {
        // SAFETY: live world.
        unsafe { sys::ara_world_active_body_count(self.w()) as usize }
    }

    fn add_character(&mut self, d: &CharacterDesc) -> u32 {
        let desc = sys::AraCharacterDesc {
            radius: d.radius,
            half_height: d.half_height,
            max_slope_deg: d.max_slope_deg,
            max_strength: 100.0,
            padding: d.padding,
            position: v(d.position),
        };
        self.characters.push(Character {
            step_up: d.step_height,
            stick_down: d.stick_down,
        });
        // SAFETY: live world, valid desc.
        unsafe { sys::ara_character_create(self.w(), &desc) }
    }

    /// Jolt's documented CharacterVirtual loop (Samples/Tests/Character/CharacterVirtualTest):
    /// ground velocity when on walkable ground, otherwise keep vertical velocity; add gravity
    /// and the desired horizontal velocity; then ExtendedUpdate (stairs + stick to floor).
    fn update_character(&mut self, c: u32, desired: Vec3) -> CharacterState {
        let w = self.w();
        let ch = &self.characters[c as usize];
        let mut s = sys::AraCharacterState::default();
        // SAFETY: live world; `c` was returned by ara_character_create.
        unsafe {
            sys::ara_character_update_ground_velocity(w, c);
            sys::ara_character_state(w, c, &mut s);
        }
        let vertical = Vec3::new(0.0, s.linear_velocity.y, 0.0);
        let mut vel = if s.ground_state == 0 {
            gv(s.ground_velocity)
        } else {
            vertical
        };
        vel += GRAVITY * DT + desired;
        // SAFETY: as above.
        unsafe {
            sys::ara_character_set_linear_velocity(w, c, v(vel));
            let allow_sliding = u32::from(desired.length_squared() > 1e-6);
            sys::ara_character_extended_update(w, c, DT, ch.step_up, ch.stick_down, allow_sliding);
        }
        self.character_state(c)
    }

    fn character_state(&self, c: u32) -> CharacterState {
        let mut s = sys::AraCharacterState::default();
        // SAFETY: live world; valid character index.
        unsafe { sys::ara_character_state(self.w(), c, &mut s) };
        CharacterState {
            position: gv(s.position),
            velocity: gv(s.linear_velocity),
            grounded: s.ground_state == 0,
            ground_velocity: gv(s.ground_velocity),
        }
    }

    fn add_vehicle(&mut self, d: &VehicleDesc) -> u32 {
        let desc = sys::AraVehicleDesc {
            position: v(d.position),
            half_extents: v(d.half_extents),
            mass: d.mass,
            wheel_radius: d.wheel_radius,
            wheel_width: d.wheel_width,
            wheel_offsets: d.wheel_offsets.map(v),
            suspension_min: d.suspension_rest - d.suspension_travel,
            suspension_max: d.suspension_rest,
            suspension_frequency: d.suspension_frequency,
            suspension_damping: d.suspension_damping_ratio,
            max_steer_deg: d.max_steer_deg,
            engine_max_torque: 500.0,
            rear_wheel_drive: 1,
        };
        // SAFETY: live world, valid desc.
        let id = unsafe { sys::ara_vehicle_create(self.w(), &desc) };
        self.vehicles.push(id);
        id
    }

    fn drive(&mut self, vehicle: u32, forward: f32, right: f32) {
        // SAFETY: valid vehicle index.
        unsafe { sys::ara_vehicle_set_input(self.w(), vehicle, forward, right, 0.0, 0.0) }
    }

    fn vehicle_state(&self, vehicle: u32) -> VehicleState {
        let mut s = sys::AraVehicleState::default();
        // SAFETY: valid vehicle index.
        unsafe { sys::ara_vehicle_state(self.w(), vehicle, &mut s) };
        let mut out = Vec::with_capacity(1);
        self.read(&[s.chassis], &mut out);
        VehicleState {
            chassis: out[0],
            wheels_in_contact: s.wheels_in_contact,
        }
    }

    fn vehicle_chassis(&self, vehicle: u32) -> BodyId {
        let mut s = sys::AraVehicleState::default();
        // SAFETY: valid vehicle index.
        unsafe { sys::ara_vehicle_state(self.w(), vehicle, &mut s) };
        s.chassis
    }

    fn save(&mut self) -> Vec<u8> {
        let mut len = 0usize;
        // SAFETY: the returned bytes stay valid until the next save; we copy them out now.
        unsafe {
            let ptr = sys::ara_world_save_state(self.w(), &mut len);
            std::slice::from_raw_parts(ptr, len).to_vec()
        }
    }

    fn restore(&mut self, bytes: &[u8]) {
        // SAFETY: bytes came from `save` on an identically built world.
        let ok = unsafe { sys::ara_world_restore_state(self.w(), bytes.as_ptr(), bytes.len()) };
        assert_eq!(ok, 1, "Jolt restore failed");
    }

    fn cast_ray(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<Hit> {
        let mut h = sys::AraHit::default();
        // SAFETY: live world.
        let hit =
            unsafe { sys::ara_cast_ray(self.w(), v(origin), v(direction * max_distance), &mut h) };
        (hit == 1).then(|| Hit {
            distance: h.fraction * max_distance,
            normal: gv(h.normal),
        })
    }

    fn cast_sphere(
        &self,
        radius: f32,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Option<Hit> {
        let mut h = sys::AraHit::default();
        // SAFETY: live world.
        let hit = unsafe {
            sys::ara_cast_sphere(
                self.w(),
                radius,
                v(origin),
                v(direction * max_distance),
                &mut h,
            )
        };
        (hit == 1).then(|| Hit {
            distance: h.fraction * max_distance,
            normal: gv(h.normal),
        })
    }

    fn body_position(&self, id: BodyId) -> Vec3 {
        let mut p = sys::AraVec3::default();
        // SAFETY: live world.
        unsafe { sys::ara_body_position(self.w(), id, &mut p) };
        gv(p)
    }
}
