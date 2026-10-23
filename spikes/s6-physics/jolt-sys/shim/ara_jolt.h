// Ara's thin C ABI over Jolt Physics. Hand-written; mirrored by jolt-sys/src/lib.rs.
//
// Shape of the API: ECS-friendly batches (add/read many bodies per call), plain-old-data
// structs, u32 handles. No callbacks cross the boundary; no C++ exceptions (Jolt is built
// without them). One AraWorld owns everything created through it.
#pragma once
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AraWorld AraWorld;

typedef struct { float x, y, z; } AraVec3;
typedef struct { float x, y, z, w; } AraQuat;

enum { ARA_LAYER_STATIC = 0, ARA_LAYER_MOVING = 1 };
enum { ARA_MOTION_STATIC = 0, ARA_MOTION_KINEMATIC = 1, ARA_MOTION_DYNAMIC = 2 };

typedef struct {
    uint32_t max_bodies;
    uint32_t max_body_pairs;
    uint32_t max_contact_constraints;
    // 0 = JobSystemSingleThreaded; N >= 1 = thread pool where N-1 workers plus the
    // calling thread run jobs (Jolt's caller helps while waiting on barriers).
    uint32_t threads;
    uint32_t temp_allocator_mb;
    AraVec3 gravity;
} AraWorldDesc;

typedef struct {
    uint32_t shape;
    AraVec3 position;
    AraQuat rotation;
    uint8_t motion;   // ARA_MOTION_*
    uint8_t layer;    // ARA_LAYER_*
    uint8_t allow_sleep;
    uint8_t _pad;
    float friction;
    float restitution;
    float mass;       // <= 0: computed from shape density
    AraVec3 linear_velocity;
} AraBodyDesc;

typedef struct {
    AraVec3 position;
    AraQuat rotation;
    AraVec3 linear_velocity;
    AraVec3 angular_velocity;
    uint32_t active;
} AraBodyState;

typedef struct {
    uint32_t body;   // 0xFFFFFFFF when nothing was hit
    float fraction;  // along the cast direction, 0..1
    AraVec3 point;
    AraVec3 normal;
} AraHit;

typedef struct {
    float radius;
    float half_height;   // cylinder half height of the capsule
    float max_slope_deg;
    float max_strength;
    float padding;
    AraVec3 position;    // feet
} AraCharacterDesc;

typedef struct {
    AraVec3 position;    // feet
    AraVec3 linear_velocity;
    AraVec3 ground_velocity;
    AraVec3 ground_normal;
    uint32_t ground_body; // 0xFFFFFFFF when none
    uint32_t ground_state; // Jolt EGroundState: 0 OnGround, 1 OnSteepGround, 2 NotSupported, 3 InAir
    uint32_t supported;
} AraCharacterState;

typedef struct {
    AraVec3 position;
    AraVec3 half_extents;
    float mass;
    float wheel_radius;
    float wheel_width;
    AraVec3 wheel_offsets[4]; // FL, FR, RL, RR in chassis space; forward is +Z
    float suspension_min;
    float suspension_max;
    float suspension_frequency;
    float suspension_damping;
    float max_steer_deg;
    float engine_max_torque;
    uint32_t rear_wheel_drive;
} AraVehicleDesc;

typedef struct {
    uint32_t chassis;
    uint32_t wheels_in_contact;
    float engine_rpm;
    int32_t gear;
} AraVehicleState;

// --- world ---------------------------------------------------------------------------
AraWorld* ara_world_create(const AraWorldDesc* desc);
void ara_world_destroy(AraWorld* world);
// Returns Jolt's EPhysicsUpdateError bits (0 = ok).
uint32_t ara_world_step(AraWorld* world, float dt, int32_t collision_steps);
void ara_world_optimize_broadphase(AraWorld* world);
// Solver iteration counts (Jolt defaults: 10 velocity, 2 position).
void ara_world_set_solver_steps(AraWorld* world, uint32_t velocity_steps, uint32_t position_steps);
uint32_t ara_world_active_body_count(const AraWorld* world);
uint32_t ara_world_body_count(const AraWorld* world);
// Returns 1 if this library was compiled with JPH_CROSS_PLATFORM_DETERMINISTIC.
uint32_t ara_cross_platform_deterministic(void);
// Empty call; used to measure raw FFI overhead.
uint32_t ara_noop(uint32_t value);
// Writes sizeof() of every ABI struct (in header order) so Rust can assert its layouts.
uint32_t ara_abi_sizes(uint32_t* out, uint32_t capacity);

// --- shapes (owned by the world, referenced by index) ---------------------------------
uint32_t ara_shape_box(AraWorld* world, AraVec3 half_extents);
uint32_t ara_shape_sphere(AraWorld* world, float radius);
uint32_t ara_shape_mesh(AraWorld* world, const AraVec3* vertices, uint32_t vertex_count,
                        const uint32_t* indices, uint32_t triangle_count);

// --- bodies ----------------------------------------------------------------------------
// Creates bodies in array order (BodyIDs are therefore deterministic) and adds them in one
// batch. Writes the BodyIDs to out_ids. Returns the number created.
uint32_t ara_bodies_add(AraWorld* world, const AraBodyDesc* descs, uint32_t count,
                        uint32_t* out_ids);
// Reads the state of `count` bodies into `out` in one call (the ECS extraction path).
void ara_bodies_read(const AraWorld* world, const uint32_t* ids, uint32_t count,
                     AraBodyState* out);
void ara_body_position(const AraWorld* world, uint32_t id, AraVec3* out);
void ara_body_move_kinematic(AraWorld* world, uint32_t id, AraVec3 position, AraQuat rotation,
                             float dt);
void ara_body_set_linear_velocity(AraWorld* world, uint32_t id, AraVec3 velocity);

// --- queries (narrow phase; results are deterministic, see Jolt Architecture.md) ------
uint32_t ara_cast_ray(const AraWorld* world, AraVec3 origin, AraVec3 direction, AraHit* out);
uint32_t ara_cast_sphere(const AraWorld* world, float radius, AraVec3 origin, AraVec3 direction,
                         AraHit* out);

// --- state (rollback / prediction contract) -------------------------------------------
// Saves the physics system (bodies, contacts, constraints including vehicles) and then every
// character, in creation order. Returns a pointer to bytes owned by the world, valid until
// the next save; *out_len receives the length.
const uint8_t* ara_world_save_state(AraWorld* world, size_t* out_len);
// Restores bytes produced by ara_world_save_state on a world with the same bodies,
// characters and vehicles. Returns 1 on success.
uint32_t ara_world_restore_state(AraWorld* world, const uint8_t* data, size_t len);

// --- CharacterVirtual -----------------------------------------------------------------
uint32_t ara_character_create(AraWorld* world, const AraCharacterDesc* desc);
void ara_character_state(const AraWorld* world, uint32_t character, AraCharacterState* out);
void ara_character_update_ground_velocity(AraWorld* world, uint32_t character);
void ara_character_set_linear_velocity(AraWorld* world, uint32_t character, AraVec3 velocity);
// CharacterVirtual::ExtendedUpdate (move + stick to floor + walk stairs). With
// allow_sliding = 0 the shim's contact listener stops the character sliding down walkable
// static slopes (Jolt sample behaviour for an idle player).
void ara_character_extended_update(AraWorld* world, uint32_t character, float dt,
                                   float step_up, float stick_down, uint32_t allow_sliding);

// --- VehicleConstraint (wheeled, engine + transmission + differential) ----------------
uint32_t ara_vehicle_create(AraWorld* world, const AraVehicleDesc* desc);
void ara_vehicle_set_input(AraWorld* world, uint32_t vehicle, float forward, float right,
                           float brake, float hand_brake);
void ara_vehicle_state(const AraWorld* world, uint32_t vehicle, AraVehicleState* out);

#ifdef __cplusplus
}
#endif
