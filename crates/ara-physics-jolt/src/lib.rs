//! Jolt implementation of Ara's physics contract.
//!
//! Owns an optimized, pinned Jolt 5.6 world through the exception-contained C ABI.
//! The default deterministic feature costs about 7–12% in the S6 measurements.
//! Cross-platform bit equality still requires testing on the other target machines.
//! No Rust callbacks, compiler processes, filesystem access or wall clock enter a tick.

mod bodies;
mod character;
mod convert;
mod debug;
mod joints;
mod query;
mod shapes;
mod snapshot;
mod vehicle;

use ara_jolt_sys as sys;
use ara_physics::*;
use std::{collections::BTreeMap, ptr::NonNull};

pub use snapshot::JoltSnapshot;

/// Convenient production world using Ara's Jolt backend.
pub type JoltWorld = PhysicsWorld<JoltBackend>;

#[derive(Clone, Copy)]
struct BodyMeta {
    shape: ShapeHandle,
    user_data: u64,
}
#[derive(Clone, Copy)]
struct CharacterMeta {
    controller: CharacterController,
    user_data: u64,
}
#[derive(Clone, Copy)]
struct VehicleMeta {
    desc: VehicleDesc,
}
type ContactKey = (u32, u32, u32, u32);
type CharacterContactKey = (u32, u32, u32, u32);

/// Owns exactly one native world. Only exclusive Rust access can mutate native state.
pub struct JoltBackend {
    world: NonNull<sys::AraWorld>,
    config: PhysicsConfig,
    shapes: Vec<DebugGeometry>,
    primitive_cache: BTreeMap<Vec<u32>, ShapeHandle>,
    bodies: BTreeMap<BodyHandle, BodyMeta>,
    characters: BTreeMap<CharacterHandle, CharacterMeta>,
    vehicles: BTreeMap<VehicleHandle, VehicleMeta>,
    joints: BTreeMap<JointHandle, JointDesc>,
    contacts: BTreeMap<ContactKey, (bool, ContactEvent)>,
    character_contacts: BTreeMap<CharacterContactKey, CharacterContactEvent>,
    raw_contacts: Vec<sys::AraContact>,
    raw_bodies: Vec<sys::AraBodyState>,
    raw_ids: Vec<u32>,
    raw_commands: Vec<sys::AraBodyCommand>,
    structure: u64,
    fingerprint: u64,
}

// SAFETY: the world owns all native allocations and workers. Native calls are
// synchronous and return only after worker barriers complete. No thread-local
// world ownership or Rust callbacks exist; moving ownership between threads is safe.
unsafe impl Send for JoltBackend {}
// SAFETY: shared methods perform read-only no-lock Jolt queries/extraction, each
// with call-local collectors. All native mutation, scratch use and stepping
// require &mut self, which excludes concurrent readers/writers in safe Rust.
unsafe impl Sync for JoltBackend {}

impl Drop for JoltBackend {
    fn drop(&mut self) {
        // SAFETY: this owner exclusively owns the pointer and destroys it once.
        unsafe { sys::ara_world_destroy(self.world.as_ptr()) }
    }
}

impl JoltBackend {
    fn ptr(&self) -> *mut sys::AraWorld {
        self.world.as_ptr()
    }
    fn error() -> PhysicsError {
        let mut text = [0 as std::ffi::c_char; 512];
        // SAFETY: text has 512 writable bytes and is NUL-terminated by the shim.
        unsafe {
            sys::ara_last_error(text.as_mut_ptr(), text.len());
        }
        // SAFETY: the shim always writes a NUL within this nonempty buffer.
        let message = unsafe { std::ffi::CStr::from_ptr(text.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        PhysicsError(message)
    }
    fn ok(status: u32) -> Result<()> {
        if status == 1 {
            Ok(())
        } else {
            Err(Self::error())
        }
    }
    fn count(len: usize) -> Result<u32> {
        u32::try_from(len).map_err(|_| PhysicsError("native slice exceeds u32 capacity".into()))
    }
    fn require_body(&self, id: BodyHandle) -> Result<()> {
        if self.bodies.contains_key(&id) {
            Ok(())
        } else {
            Err(PhysicsError(
                "body handle is stale or belongs to another world".into(),
            ))
        }
    }
    fn require_shape(&self, id: ShapeHandle) -> Result<()> {
        if (id.raw() as usize) < self.shapes.len() {
            Ok(())
        } else {
            Err(PhysicsError(
                "shape handle is stale or belongs to another world".into(),
            ))
        }
    }
    fn hash_words(&mut self, words: impl IntoIterator<Item = u64>) {
        for word in words {
            for byte in word.to_le_bytes() {
                self.fingerprint ^= u64::from(byte);
                self.fingerprint = self.fingerprint.wrapping_mul(1099511628211);
            }
        }
    }
    fn hash_floats(&mut self, values: impl IntoIterator<Item = f32>) {
        self.hash_words(values.into_iter().map(|f| u64::from(f.to_bits())));
    }
    fn changed(&mut self, tag: u64) {
        self.structure = self.structure.wrapping_add(1);
        self.hash_words([tag]);
    }
}

impl PhysicsBackend for JoltBackend {
    type Snapshot = JoltSnapshot;
    fn new(config: &PhysicsConfig) -> Result<Self> {
        #[cfg(target_arch = "x86_64")]
        if !std::is_x86_feature_detected!("avx2")
            || !std::is_x86_feature_detected!("bmi1")
            || !std::is_x86_feature_detected!("lzcnt")
            || !std::is_x86_feature_detected!("f16c")
            || (!cfg!(feature = "cross-platform-deterministic")
                && !std::is_x86_feature_detected!("fma"))
        {
            return Err(PhysicsError(
                "this Jolt build requires AVX2, BMI1, LZCNT, F16C (and FMA in fast mode)".into(),
            ));
        }
        if !config.fixed_dt.is_finite()
            || config.fixed_dt <= 0.0
            || !config.gravity.iter().all(|v| v.is_finite())
        {
            return Err(PhysicsError(
                "nonfinite or nonpositive fixed interval/gravity".into(),
            ));
        }
        let d = sys::AraWorldDesc {
            max_bodies: config.max_bodies,
            max_pairs: config.max_pairs,
            max_contacts: config.max_contacts,
            threads: config.threads,
            scratch_mb: config.scratch_mb,
            velocity_steps: config.velocity_steps,
            position_steps: config.position_steps,
            event_capacity: config.event_capacity,
            layer_masks: config.layer_masks,
            gravity: convert::v(config.gravity),
        };
        // SAFETY: d is fully initialized; the shim validates capacity/filter settings.
        let world = NonNull::new(unsafe { sys::ara_world_create(&d) }).ok_or_else(Self::error)?;
        Ok(Self {
            world,
            config: config.clone(),
            shapes: Vec::new(),
            primitive_cache: BTreeMap::new(),
            bodies: BTreeMap::new(),
            characters: BTreeMap::new(),
            vehicles: BTreeMap::new(),
            joints: BTreeMap::new(),
            contacts: BTreeMap::new(),
            character_contacts: BTreeMap::new(),
            raw_contacts: vec![sys::AraContact::default(); config.event_capacity as usize],
            raw_bodies: Vec::new(),
            raw_ids: Vec::new(),
            raw_commands: Vec::new(),
            structure: 0,
            fingerprint: 14695981039346656037,
        })
    }
    fn supports(&self, feature: Feature) -> bool {
        !matches!(feature, Feature::Physics2d | Feature::TrackedVehicle)
    }
    fn deterministic(&self) -> bool {
        // SAFETY: reads a compile-time constant; no state or pointers.
        unsafe { sys::ara_deterministic() != 0 }
    }
    fn create_shape(&mut self, shape: Shape<'_>) -> Result<ShapeHandle> {
        self.cook(shape)
    }
    fn create_bodies(&mut self, bodies: &[BodyDesc], out: &mut Vec<BodyHandle>) -> Result<()> {
        self.add_bodies(bodies, out)
    }
    fn destroy_body(&mut self, body: BodyHandle) -> Result<()> {
        self.remove_body(body)
    }
    fn apply(&mut self, commands: &[BodyCommand], dt: f32) -> Result<()> {
        self.apply_bodies(commands, dt)
    }
    fn read_bodies(&mut self, bodies: &[BodyHandle], out: &mut Vec<BodyState>) -> Result<()> {
        self.extract_bodies(bodies, out)
    }
    fn body_state(&self, body: BodyHandle) -> Result<BodyState> {
        self.extract_body(body)
    }
    fn active_count(&self) -> u32 {
        // SAFETY: shared live world, with no concurrent native mutation.
        unsafe { sys::ara_world_active(self.ptr()) }
    }
    fn optimize(&mut self) -> Result<()> {
        // SAFETY: exclusive live world, outside any outstanding update.
        Self::ok(unsafe { sys::ara_world_optimize(self.ptr()) })
    }
    fn create_character(&mut self, desc: CharacterDesc) -> Result<CharacterHandle> {
        self.add_character(desc)
    }
    fn destroy_character(&mut self, id: CharacterHandle) -> Result<()> {
        self.remove_character(id)
    }
    fn step_character(
        &mut self,
        id: CharacterHandle,
        input: CharacterInput,
        dt: f32,
    ) -> Result<()> {
        self.move_character(id, input, dt)
    }
    fn character_state(&self, id: CharacterHandle) -> Result<CharacterState> {
        self.extract_character(id)
    }
    fn create_vehicle(&mut self, desc: VehicleDesc) -> Result<VehicleHandle> {
        self.add_vehicle(desc)
    }
    fn destroy_vehicle(&mut self, id: VehicleHandle) -> Result<()> {
        self.remove_vehicle(id)
    }
    fn drive(&mut self, id: VehicleHandle, input: VehicleInput) -> Result<()> {
        self.drive_vehicle(id, input)
    }
    fn vehicle_state(&self, id: VehicleHandle) -> Result<VehicleState> {
        self.extract_vehicle(id)
    }
    fn create_joint(&mut self, desc: JointDesc) -> Result<JointHandle> {
        self.add_joint(desc)
    }
    fn destroy_joint(&mut self, id: JointHandle) -> Result<()> {
        self.remove_joint(id)
    }
    fn step(&mut self, dt: f32, tick: u64, events: &mut PhysicsEvents) -> Result<()> {
        self.update(dt, tick, events)
    }
    fn queries(&self, queries: &[Query], out: &mut QueryResults) -> Result<()> {
        self.query_batch(queries, out)
    }
    fn save(&mut self) -> Result<JoltSnapshot> {
        self.save_snapshot()
    }
    fn restore(&mut self, snapshot: &JoltSnapshot) -> Result<()> {
        self.restore_snapshot(snapshot)
    }
    fn debug_draw(&self, out: &mut Vec<DebugPrimitive>) -> Result<()> {
        self.draw(out)
    }
    fn debug_geometry(&self, id: ShapeHandle) -> Option<&DebugGeometry> {
        self.shapes.get(id.raw() as usize)
    }
}
