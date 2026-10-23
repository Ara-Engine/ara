// Ara's thin C ABI over Jolt Physics (see ara_jolt.h). Compiled by jolt-sys/build.rs
// together with the vendored Jolt sources, with identical defines, so the ABI between
// this file and Jolt can never drift.
#include "ara_jolt.h"

#include <Jolt/Jolt.h>

#include <Jolt/Core/Factory.h>
#include <Jolt/Core/JobSystemSingleThreaded.h>
#include <Jolt/Core/JobSystemThreadPool.h>
#include <Jolt/Core/TempAllocator.h>
#include <Jolt/Physics/Body/BodyCreationSettings.h>
#include <Jolt/Physics/Character/CharacterVirtual.h>
#include <Jolt/Physics/Collision/BroadPhase/BroadPhaseLayerInterfaceTable.h>
#include <Jolt/Physics/Collision/BroadPhase/ObjectVsBroadPhaseLayerFilterTable.h>
#include <Jolt/Physics/Collision/CastResult.h>
#include <Jolt/Physics/Collision/CollisionCollectorImpl.h>
#include <Jolt/Physics/Collision/ObjectLayerPairFilterTable.h>
#include <Jolt/Physics/Collision/RayCast.h>
#include <Jolt/Physics/Collision/Shape/BoxShape.h>
#include <Jolt/Physics/Collision/Shape/CapsuleShape.h>
#include <Jolt/Physics/Collision/Shape/MeshShape.h>
#include <Jolt/Physics/Collision/Shape/OffsetCenterOfMassShape.h>
#include <Jolt/Physics/Collision/Shape/RotatedTranslatedShape.h>
#include <Jolt/Physics/Collision/Shape/SphereShape.h>
#include <Jolt/Physics/Collision/ShapeCast.h>
#include <Jolt/Physics/PhysicsSystem.h>
#include <Jolt/Physics/StateRecorder.h>
#include <Jolt/Physics/Vehicle/VehicleCollisionTester.h>
#include <Jolt/Physics/Vehicle/VehicleConstraint.h>
#include <Jolt/Physics/Vehicle/WheeledVehicleController.h>
#include <Jolt/RegisterTypes.h>

#include <cstring>
#include <memory>
#include <mutex>
#include <vector>

using namespace JPH;

namespace {

constexpr uint32_t kInvalid = 0xFFFFFFFFu;

void global_init() {
    static std::once_flag once;
    std::call_once(once, [] {
        RegisterDefaultAllocator();
        Factory::sInstance = new Factory();
        RegisterTypes();
    });
}

inline Vec3 v3(AraVec3 v) { return Vec3(v.x, v.y, v.z); }
inline AraVec3 a3(Vec3Arg v) { return AraVec3{v.GetX(), v.GetY(), v.GetZ()}; }
inline Quat q4(AraQuat q) { return Quat(q.x, q.y, q.z, q.w); }
inline AraQuat aq(QuatArg q) { return AraQuat{q.GetX(), q.GetY(), q.GetZ(), q.GetW()}; }

// StateRecorder over a growable byte vector (Jolt's StateRecorderImpl uses a stringstream).
class VecWriter final : public StateRecorder {
public:
    std::vector<uint8_t> bytes;
    void WriteBytes(const void* data, size_t n) override {
        const uint8_t* p = static_cast<const uint8_t*>(data);
        bytes.insert(bytes.end(), p, p + n);
    }
    void ReadBytes(void* out, size_t n) override {
        std::memset(out, 0, n);
        failed = true;
    }
    bool IsEOF() const override { return true; }
    bool IsFailed() const override { return failed; }

private:
    bool failed = false;
};

// Read-only StateRecorder over borrowed bytes.
class SpanReader final : public StateRecorder {
public:
    SpanReader(const uint8_t* data, size_t len) : data(data), len(len) {}
    void WriteBytes(const void*, size_t) override { failed = true; }
    void ReadBytes(void* out, size_t n) override {
        if (pos + n > len) {
            std::memset(out, 0, n);
            failed = true;
            return;
        }
        std::memcpy(out, data + pos, n);
        pos += n;
    }
    bool IsEOF() const override { return pos >= len; }
    bool IsFailed() const override { return failed; }
    bool fully_consumed() const { return pos == len; }

private:
    const uint8_t* data;
    size_t len;
    size_t pos = 0;
    bool failed = false;
};

// Contact policy shared by all characters of a world. Runs inside Jolt (no FFI callback).
class CharacterListener final : public CharacterContactListener {
public:
    std::vector<uint8_t> allow_sliding; // indexed by character user data

    void OnContactSolve(const CharacterVirtual* character, const BodyID&, const SubShapeID&, RVec3Arg,
                        Vec3Arg normal, Vec3Arg contact_velocity, const PhysicsMaterial*, Vec3Arg,
                        Vec3& new_velocity) override {
        size_t i = size_t(character->GetUserData());
        bool sliding = i < allow_sliding.size() && allow_sliding[i] != 0;
        if (!sliding && contact_velocity.IsNearZero() && !character->IsSlopeTooSteep(normal))
            new_velocity = Vec3::sZero();
    }
};

} // namespace

struct AraWorld {
    BroadPhaseLayerInterfaceTable bp_layers{2, 2};
    ObjectLayerPairFilterTable pair_filter{2};
    std::unique_ptr<ObjectVsBroadPhaseLayerFilterTable> obj_vs_bp;
    std::unique_ptr<TempAllocatorImpl> temp;
    std::unique_ptr<JobSystem> jobs;
    PhysicsSystem system;
    std::vector<RefConst<Shape>> shapes;
    std::vector<Ref<CharacterVirtual>> characters;
    CharacterListener character_listener;
    std::vector<Ref<VehicleConstraint>> vehicles;
    std::vector<BodyID> vehicle_chassis;
    RefConst<VehicleCollisionTester> vehicle_tester;
    VecWriter snapshot;

    AraWorld(const AraWorldDesc& d) {
        bp_layers.MapObjectToBroadPhaseLayer(ARA_LAYER_STATIC, BroadPhaseLayer(0));
        bp_layers.MapObjectToBroadPhaseLayer(ARA_LAYER_MOVING, BroadPhaseLayer(1));
        pair_filter.EnableCollision(ARA_LAYER_MOVING, ARA_LAYER_STATIC);
        pair_filter.EnableCollision(ARA_LAYER_MOVING, ARA_LAYER_MOVING);
        obj_vs_bp = std::make_unique<ObjectVsBroadPhaseLayerFilterTable>(bp_layers, 2, pair_filter, 2);
        temp = std::make_unique<TempAllocatorImpl>(size_t(d.temp_allocator_mb) * 1024 * 1024);
        if (d.threads <= 1)
            jobs = std::make_unique<JobSystemSingleThreaded>(cMaxPhysicsJobs);
        else
            jobs = std::make_unique<JobSystemThreadPool>(cMaxPhysicsJobs, cMaxPhysicsBarriers, int(d.threads) - 1);
        system.Init(d.max_bodies, 0, d.max_body_pairs, d.max_contact_constraints, bp_layers, *obj_vs_bp, pair_filter);
        system.SetGravity(v3(d.gravity));
        vehicle_tester = new VehicleCollisionTesterRay(ARA_LAYER_MOVING);
    }

    ~AraWorld() {
        for (auto& v : vehicles) {
            system.RemoveStepListener(v);
            system.RemoveConstraint(v);
        }
        vehicles.clear();
        characters.clear();
    }

    const Body* body(uint32_t id) const {
        return system.GetBodyLockInterfaceNoLock().TryGetBody(BodyID(id));
    }
};

extern "C" {

AraWorld* ara_world_create(const AraWorldDesc* desc) {
    global_init();
    return new AraWorld(*desc);
}

void ara_world_destroy(AraWorld* world) { delete world; }

uint32_t ara_world_step(AraWorld* world, float dt, int32_t collision_steps) {
    return uint32_t(world->system.Update(dt, collision_steps, world->temp.get(), world->jobs.get()));
}

void ara_world_optimize_broadphase(AraWorld* world) { world->system.OptimizeBroadPhase(); }

void ara_world_set_solver_steps(AraWorld* world, uint32_t velocity_steps, uint32_t position_steps) {
    PhysicsSettings s = world->system.GetPhysicsSettings();
    s.mNumVelocitySteps = velocity_steps;
    s.mNumPositionSteps = position_steps;
    world->system.SetPhysicsSettings(s);
}

uint32_t ara_world_active_body_count(const AraWorld* world) {
    return world->system.GetNumActiveBodies(EBodyType::RigidBody);
}

uint32_t ara_world_body_count(const AraWorld* world) { return world->system.GetNumBodies(); }

uint32_t ara_cross_platform_deterministic(void) {
#ifdef JPH_CROSS_PLATFORM_DETERMINISTIC
    return 1;
#else
    return 0;
#endif
}

uint32_t ara_noop(uint32_t value) { return value + 1; }

uint32_t ara_abi_sizes(uint32_t* out, uint32_t capacity) {
    const uint32_t sizes[] = {
        sizeof(AraVec3), sizeof(AraQuat), sizeof(AraWorldDesc), sizeof(AraBodyDesc),
        sizeof(AraBodyState), sizeof(AraHit), sizeof(AraCharacterDesc), sizeof(AraCharacterState),
        sizeof(AraVehicleDesc), sizeof(AraVehicleState),
    };
    const uint32_t n = uint32_t(sizeof(sizes) / sizeof(sizes[0]));
    for (uint32_t i = 0; i < n && i < capacity; ++i)
        out[i] = sizes[i];
    return n;
}

uint32_t ara_shape_box(AraWorld* world, AraVec3 half_extents) {
    Vec3 h = v3(half_extents);
    float radius = std::min(cDefaultConvexRadius, 0.5f * h.ReduceMin());
    world->shapes.push_back(new BoxShape(h, radius));
    return uint32_t(world->shapes.size() - 1);
}

uint32_t ara_shape_sphere(AraWorld* world, float radius) {
    world->shapes.push_back(new SphereShape(radius));
    return uint32_t(world->shapes.size() - 1);
}

uint32_t ara_shape_mesh(AraWorld* world, const AraVec3* vertices, uint32_t vertex_count,
                        const uint32_t* indices, uint32_t triangle_count) {
    VertexList verts;
    verts.reserve(vertex_count);
    for (uint32_t i = 0; i < vertex_count; ++i)
        verts.push_back(Float3(vertices[i].x, vertices[i].y, vertices[i].z));
    IndexedTriangleList tris;
    tris.reserve(triangle_count);
    for (uint32_t i = 0; i < triangle_count; ++i)
        tris.push_back(IndexedTriangle(indices[3 * i], indices[3 * i + 1], indices[3 * i + 2]));
    MeshShapeSettings settings(std::move(verts), std::move(tris));
    ShapeSettings::ShapeResult result = settings.Create();
    if (result.HasError())
        return kInvalid;
    world->shapes.push_back(result.Get());
    return uint32_t(world->shapes.size() - 1);
}

uint32_t ara_bodies_add(AraWorld* world, const AraBodyDesc* descs, uint32_t count, uint32_t* out_ids) {
    BodyInterface& bi = world->system.GetBodyInterfaceNoLock();
    std::vector<BodyID> batch;
    batch.reserve(count);
    for (uint32_t i = 0; i < count; ++i) {
        const AraBodyDesc& d = descs[i];
        EMotionType motion = d.motion == ARA_MOTION_DYNAMIC     ? EMotionType::Dynamic
                             : d.motion == ARA_MOTION_KINEMATIC ? EMotionType::Kinematic
                                                                : EMotionType::Static;
        BodyCreationSettings s(world->shapes[d.shape], RVec3(v3(d.position)), q4(d.rotation), motion, ObjectLayer(d.layer));
        s.mFriction = d.friction;
        s.mRestitution = d.restitution;
        s.mAllowSleeping = d.allow_sleep != 0;
        s.mLinearVelocity = v3(d.linear_velocity);
        if (d.mass > 0.0f) {
            s.mOverrideMassProperties = EOverrideMassProperties::CalculateInertia;
            s.mMassPropertiesOverride.mMass = d.mass;
        }
        Body* body = bi.CreateBody(s);
        if (body == nullptr)
            break;
        out_ids[i] = body->GetID().GetIndexAndSequenceNumber();
        batch.push_back(body->GetID());
    }
    // AddBodiesPrepare may reorder `batch`; out_ids keeps creation order.
    if (!batch.empty()) {
        BodyInterface::AddState state = bi.AddBodiesPrepare(batch.data(), int(batch.size()));
        bi.AddBodiesFinalize(batch.data(), int(batch.size()), state, EActivation::Activate);
    }
    return uint32_t(batch.size());
}

void ara_bodies_read(const AraWorld* world, const uint32_t* ids, uint32_t count, AraBodyState* out) {
    for (uint32_t i = 0; i < count; ++i) {
        const Body* b = world->body(ids[i]);
        if (b == nullptr) {
            std::memset(&out[i], 0, sizeof(AraBodyState));
            continue;
        }
        out[i].position = a3(Vec3(b->GetPosition()));
        out[i].rotation = aq(b->GetRotation());
        out[i].linear_velocity = a3(b->GetLinearVelocity());
        out[i].angular_velocity = a3(b->GetAngularVelocity());
        out[i].active = b->IsActive() ? 1u : 0u;
    }
}

void ara_body_position(const AraWorld* world, uint32_t id, AraVec3* out) {
    const Body* b = world->body(id);
    *out = b ? a3(Vec3(b->GetPosition())) : AraVec3{0, 0, 0};
}

void ara_body_move_kinematic(AraWorld* world, uint32_t id, AraVec3 position, AraQuat rotation, float dt) {
    world->system.GetBodyInterfaceNoLock().MoveKinematic(BodyID(id), RVec3(v3(position)), q4(rotation), dt);
}

void ara_body_set_linear_velocity(AraWorld* world, uint32_t id, AraVec3 velocity) {
    world->system.GetBodyInterfaceNoLock().SetLinearVelocity(BodyID(id), v3(velocity));
}

uint32_t ara_cast_ray(const AraWorld* world, AraVec3 origin, AraVec3 direction, AraHit* out) {
    RRayCast ray{RVec3(v3(origin)), v3(direction)};
    RayCastResult hit;
    out->body = kInvalid;
    if (!world->system.GetNarrowPhaseQueryNoLock().CastRay(ray, hit))
        return 0;
    out->body = hit.mBodyID.GetIndexAndSequenceNumber();
    out->fraction = hit.mFraction;
    RVec3 point = ray.GetPointOnRay(hit.mFraction);
    out->point = a3(Vec3(point));
    const Body* b = world->body(out->body);
    out->normal = b ? a3(b->GetWorldSpaceSurfaceNormal(hit.mSubShapeID2, point)) : AraVec3{0, 1, 0};
    return 1;
}

uint32_t ara_cast_sphere(const AraWorld* world, float radius, AraVec3 origin, AraVec3 direction, AraHit* out) {
    SphereShape sphere(radius);
    sphere.SetEmbedded();
    RShapeCast cast(&sphere, Vec3::sOne(), RMat44::sTranslation(RVec3(v3(origin))), v3(direction));
    ShapeCastSettings settings;
    ClosestHitCollisionCollector<CastShapeCollector> collector;
    world->system.GetNarrowPhaseQueryNoLock().CastShape(cast, settings, RVec3::sZero(), collector);
    out->body = kInvalid;
    if (!collector.HadHit())
        return 0;
    const ShapeCastResult& h = collector.mHit;
    out->body = h.mBodyID2.GetIndexAndSequenceNumber();
    out->fraction = h.mFraction;
    out->point = a3(Vec3(h.mContactPointOn2));
    out->normal = a3(-h.mPenetrationAxis.NormalizedOr(Vec3::sAxisY()));
    return 1;
}

const uint8_t* ara_world_save_state(AraWorld* world, size_t* out_len) {
    VecWriter& w = world->snapshot;
    w.bytes.clear();
    world->system.SaveState(w, EStateRecorderState::All);
    for (const auto& c : world->characters)
        c->SaveState(w);
    *out_len = w.bytes.size();
    return w.bytes.data();
}

uint32_t ara_world_restore_state(AraWorld* world, const uint8_t* data, size_t len) {
    SpanReader r(data, len);
    if (!world->system.RestoreState(r))
        return 0;
    for (auto& c : world->characters)
        c->RestoreState(r);
    return (!r.IsFailed() && r.fully_consumed()) ? 1u : 0u;
}

uint32_t ara_character_create(AraWorld* world, const AraCharacterDesc* d) {
    Ref<CharacterVirtualSettings> s = new CharacterVirtualSettings();
    s->mMaxSlopeAngle = DegreesToRadians(d->max_slope_deg);
    s->mMaxStrength = d->max_strength;
    s->mShape = RotatedTranslatedShapeSettings(Vec3(0, d->half_height + d->radius, 0), Quat::sIdentity(),
                                               new CapsuleShape(d->half_height, d->radius))
                    .Create()
                    .Get();
    s->mCharacterPadding = d->padding;
    s->mPenetrationRecoverySpeed = 1.0f;
    s->mPredictiveContactDistance = 0.1f;
    // Contacts below the centre of the lower hemisphere count as support.
    s->mSupportingVolume = Plane(Vec3::sAxisY(), -d->radius);
    uint64 index = world->characters.size();
    Ref<CharacterVirtual> c = new CharacterVirtual(s, RVec3(v3(d->position)), Quat::sIdentity(), index, &world->system);
    c->SetListener(&world->character_listener);
    world->characters.push_back(c);
    world->character_listener.allow_sliding.push_back(1);
    return uint32_t(world->characters.size() - 1);
}

void ara_character_state(const AraWorld* world, uint32_t character, AraCharacterState* out) {
    const CharacterVirtual& c = *world->characters[character];
    out->position = a3(Vec3(c.GetPosition()));
    out->linear_velocity = a3(c.GetLinearVelocity());
    out->ground_velocity = a3(c.GetGroundVelocity());
    out->ground_normal = a3(c.GetGroundNormal());
    out->ground_body = c.GetGroundBodyID().IsInvalid() ? kInvalid : c.GetGroundBodyID().GetIndexAndSequenceNumber();
    out->ground_state = uint32_t(c.GetGroundState());
    out->supported = c.IsSupported() ? 1u : 0u;
}

void ara_character_update_ground_velocity(AraWorld* world, uint32_t character) {
    world->characters[character]->UpdateGroundVelocity();
}

void ara_character_set_linear_velocity(AraWorld* world, uint32_t character, AraVec3 velocity) {
    world->characters[character]->SetLinearVelocity(v3(velocity));
}

void ara_character_extended_update(AraWorld* world, uint32_t character, float dt, float step_up, float stick_down,
                                   uint32_t allow_sliding) {
    world->character_listener.allow_sliding[character] = allow_sliding != 0 ? 1 : 0;
    CharacterVirtual::ExtendedUpdateSettings us;
    us.mStickToFloorStepDown = Vec3(0, -stick_down, 0);
    us.mWalkStairsStepUp = Vec3(0, step_up, 0);
    PhysicsSystem& sys = world->system;
    world->characters[character]->ExtendedUpdate(dt, sys.GetGravity(), us,
                                                 sys.GetDefaultBroadPhaseLayerFilter(ARA_LAYER_MOVING),
                                                 sys.GetDefaultLayerFilter(ARA_LAYER_MOVING), {}, {}, *world->temp);
}

uint32_t ara_vehicle_create(AraWorld* world, const AraVehicleDesc* d) {
    BodyInterface& bi = world->system.GetBodyInterfaceNoLock();
    Vec3 half = v3(d->half_extents);
    RefConst<Shape> shape = OffsetCenterOfMassShapeSettings(Vec3(0, -half.GetY(), 0), new BoxShape(half)).Create().Get();
    BodyCreationSettings cs(shape, RVec3(v3(d->position)), Quat::sIdentity(), EMotionType::Dynamic, ARA_LAYER_MOVING);
    cs.mOverrideMassProperties = EOverrideMassProperties::CalculateInertia;
    cs.mMassPropertiesOverride.mMass = d->mass;
    Body* chassis = bi.CreateBody(cs);
    bi.AddBody(chassis->GetID(), EActivation::Activate);

    VehicleConstraintSettings vs;
    for (int i = 0; i < 4; ++i) {
        WheelSettingsWV* w = new WheelSettingsWV;
        w->mPosition = v3(d->wheel_offsets[i]);
        w->mSuspensionMinLength = d->suspension_min;
        w->mSuspensionMaxLength = d->suspension_max;
        w->mSuspensionSpring.mFrequency = d->suspension_frequency;
        w->mSuspensionSpring.mDamping = d->suspension_damping;
        w->mRadius = d->wheel_radius;
        w->mWidth = d->wheel_width;
        bool front = i < 2;
        w->mMaxSteerAngle = front ? DegreesToRadians(d->max_steer_deg) : 0.0f;
        if (front)
            w->mMaxHandBrakeTorque = 0.0f;
        vs.mWheels.push_back(w);
    }
    vs.mAntiRollBars.resize(2);
    vs.mAntiRollBars[0].mLeftWheel = 0;
    vs.mAntiRollBars[0].mRightWheel = 1;
    vs.mAntiRollBars[1].mLeftWheel = 2;
    vs.mAntiRollBars[1].mRightWheel = 3;
    WheeledVehicleControllerSettings* ctl = new WheeledVehicleControllerSettings;
    ctl->mEngine.mMaxTorque = d->engine_max_torque;
    ctl->mDifferentials.resize(1);
    ctl->mDifferentials[0].mLeftWheel = d->rear_wheel_drive ? 2 : 0;
    ctl->mDifferentials[0].mRightWheel = d->rear_wheel_drive ? 3 : 1;
    vs.mController = ctl;

    Ref<VehicleConstraint> vc = new VehicleConstraint(*chassis, vs);
    vc->SetVehicleCollisionTester(world->vehicle_tester);
    world->system.AddConstraint(vc);
    world->system.AddStepListener(vc);
    world->vehicles.push_back(vc);
    world->vehicle_chassis.push_back(chassis->GetID());
    return uint32_t(world->vehicles.size() - 1);
}

void ara_vehicle_set_input(AraWorld* world, uint32_t vehicle, float forward, float right, float brake, float hand_brake) {
    VehicleConstraint& vc = *world->vehicles[vehicle];
    if (forward != 0.0f || right != 0.0f || brake != 0.0f || hand_brake != 0.0f)
        world->system.GetBodyInterfaceNoLock().ActivateBody(world->vehicle_chassis[vehicle]);
    static_cast<WheeledVehicleController*>(vc.GetController())->SetDriverInput(forward, right, brake, hand_brake);
}

void ara_vehicle_state(const AraWorld* world, uint32_t vehicle, AraVehicleState* out) {
    const VehicleConstraint& vc = *world->vehicles[vehicle];
    out->chassis = world->vehicle_chassis[vehicle].GetIndexAndSequenceNumber();
    uint32_t contacts = 0;
    for (uint32_t i = 0; i < 4; ++i)
        contacts += vc.GetWheel(i)->HasContact() ? 1u : 0u;
    out->wheels_in_contact = contacts;
    const WheeledVehicleController* ctl = static_cast<const WheeledVehicleController*>(vc.GetController());
    out->engine_rpm = ctl->GetEngine().GetCurrentRPM();
    out->gear = ctl->GetTransmission().GetCurrentGear();
}

} // extern "C"
