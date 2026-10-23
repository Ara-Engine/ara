// Ara native implementation detail. No Jolt types cross ara_jolt.h.
#pragma once
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
#include <Jolt/Physics/Collision/ObjectLayerPairFilterTable.h>
#include <Jolt/Physics/Collision/ContactListener.h>
#include <Jolt/Physics/Constraints/TwoBodyConstraint.h>
#include <Jolt/Physics/PhysicsSystem.h>
#include <Jolt/Physics/StateRecorder.h>
#include <Jolt/Physics/Vehicle/VehicleConstraint.h>
#include <Jolt/Physics/Vehicle/VehicleCollisionTester.h>
#include <Jolt/Physics/Vehicle/WheeledVehicleController.h>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstring>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <vector>

using namespace JPH;
constexpr uint32_t kInvalid = 0xffffffffu;
void set_error(const char* message) noexcept;
inline void require(bool condition, const char* message) { if (!condition) throw std::runtime_error(message); }
#define ARA_CATCH(value) catch (const std::exception& e) { set_error(e.what()); return value; } catch (...) { set_error("unknown C++ exception"); return value; }
inline Vec3 v3(AraVec3 v) { return Vec3(v.x, v.y, v.z); }
inline AraVec3 a3(Vec3Arg v) { return {v.GetX(), v.GetY(), v.GetZ()}; }
inline Quat q4(AraQuat q) { return Quat(q.x, q.y, q.z, q.w); }
inline AraQuat aq(QuatArg q) { return {q.GetX(), q.GetY(), q.GetZ(), q.GetW()}; }
// Bit checks must survive /fp:fast, which may assume floating-point inputs finite.
inline bool finite(float value) {uint32_t bits;std::memcpy(&bits,&value,sizeof(bits));return (bits&0x7f800000u)!=0x7f800000u;}
inline bool finite(AraVec3 v) { return finite(v.x) && finite(v.y) && finite(v.z); }
inline bool unit(AraQuat q) { return finite(q.x) && finite(q.y) && finite(q.z) && finite(q.w) && q4(q).IsNormalized(1.0e-4f); }

// Bounded, allocation-free MPSC storage. Callbacks never call Rust or acquire locks.
class Contacts final : public ContactListener {
public:
    std::vector<AraContact> events;
    std::atomic<uint32_t> count{0};
    explicit Contacts(uint32_t capacity) : events(capacity) {}
    void push(AraContact e) noexcept {
        uint32_t i = count.fetch_add(1, std::memory_order_relaxed);
        if (i < events.size()) events[i] = e;
    }
    void contact(const Body& a, const Body& b, const ContactManifold& m, uint32_t phase) noexcept {
        push({a.GetID().GetIndexAndSequenceNumber(), b.GetID().GetIndexAndSequenceNumber(),
              m.mSubShapeID1.GetValue(), m.mSubShapeID2.GetValue(), 0, 0, phase, uint32_t(a.IsSensor() || b.IsSensor()),
              a3(Vec3(m.GetWorldSpaceContactPointOn1(0))), a3(m.mWorldSpaceNormal)});
    }
    void OnContactAdded(const Body& a, const Body& b, const ContactManifold& m, ContactSettings&) override { contact(a,b,m,0); }
    void OnContactPersisted(const Body& a, const Body& b, const ContactManifold& m, ContactSettings&) override { contact(a,b,m,1); }
    void OnContactRemoved(const SubShapeIDPair& p) override {
        push({p.GetBody1ID().GetIndexAndSequenceNumber(), p.GetBody2ID().GetIndexAndSequenceNumber(),
              p.GetSubShapeID1().GetValue(), p.GetSubShapeID2().GetValue(), 0, 0, 2, 0, {}, {}});
    }
};

class CharacterListener final : public CharacterContactListener {
public:
    std::vector<uint8_t> sliding;
    void OnContactSolve(const CharacterVirtual* c, const BodyID&, const SubShapeID&, RVec3Arg, Vec3Arg normal,
                        Vec3Arg contact_velocity, const PhysicsMaterial*, Vec3Arg, Vec3& velocity) override {
        if (!sliding[size_t(c->GetUserData())] && contact_velocity.IsNearZero() && !c->IsSlopeTooSteep(normal))
            velocity = Vec3::sZero();
    }
};

struct CharacterSlot {
    Ref<CharacterVirtual> character;
    RefConst<Shape> standing;
    RefConst<Shape> crouching;
    AraCharacterDesc desc{};
    bool crouched = false;
};
struct VehicleSlot {
    Ref<VehicleConstraint> constraint;
    BodyID chassis;
};
struct JointSlot {
    Ref<TwoBodyConstraint> constraint;
    uint32_t a, b;
};

class VecWriter final : public StateRecorder {
public:
    std::vector<uint8_t> bytes;
    void WriteBytes(const void* data, size_t len) override {
        const auto* p = static_cast<const uint8_t*>(data);
        if (len) bytes.insert(bytes.end(), p, p + len);
    }
    void ReadBytes(void*, size_t) override { throw std::runtime_error("write-only recorder"); }
    bool IsEOF() const override { return false; }
    bool IsFailed() const override { return false; }
};
class SpanReader final : public StateRecorder {
public:
    const uint8_t* data;
    size_t len, pos = 0;
    SpanReader(const uint8_t* p, size_t n) : data(p), len(n) {}
    void WriteBytes(const void*, size_t) override { throw std::runtime_error("read-only recorder"); }
    void ReadBytes(void* out, size_t n) override {
        require(n <= len - pos, "truncated snapshot");
        if (n) std::memcpy(out, data + pos, n);
        pos += n;
    }
    bool IsEOF() const override { return pos == len; }
    bool IsFailed() const override { return false; }
};

struct AraWorld {
    BroadPhaseLayerInterfaceTable bp_layers{16, 2};
    ObjectLayerPairFilterTable pairs{16};
    std::unique_ptr<ObjectVsBroadPhaseLayerFilterTable> object_bp;
    std::unique_ptr<TempAllocatorImplWithMallocFallback> temp;
    std::unique_ptr<JobSystem> jobs;
    Contacts contacts;
    CharacterListener character_listener;
    CharacterVsCharacterCollisionSimple character_collision;
    PhysicsSystem system;
    std::vector<RefConst<Shape>> shapes;
    std::vector<CharacterSlot> characters;
    std::vector<VehicleSlot> vehicles;
    std::vector<JointSlot> joints;
    VecWriter snapshot;
    uint64_t structure = 0;

    explicit AraWorld(const AraWorldDesc& d);
    ~AraWorld();
    const Body* find(uint32_t id) const {
        if (id == kInvalid) return nullptr;
        return system.GetBodyLockInterfaceNoLock().TryGetBody(BodyID(id));
    }
    Body& body(uint32_t id) {
        auto* b = system.GetBodyLockInterfaceNoLock().TryGetBody(BodyID(id));
        require(b != nullptr, "stale body handle");
        return *b;
    }
    const Shape* shape(uint32_t id) const {
        require(id < shapes.size(), "stale shape handle");
        return shapes[id];
    }
    CharacterSlot& character(uint32_t id) {
        require(id < characters.size() && characters[id].character != nullptr, "stale character handle");
        return characters[id];
    }
    const CharacterSlot& character(uint32_t id) const { return const_cast<AraWorld*>(this)->character(id); }
    VehicleSlot& vehicle(uint32_t id) {
        require(id < vehicles.size() && vehicles[id].constraint != nullptr, "stale vehicle handle");
        return vehicles[id];
    }
    const VehicleSlot& vehicle(uint32_t id) const { return const_cast<AraWorld*>(this)->vehicle(id); }
    bool has_joint(uint32_t body) const {
        for (const auto& j : joints) if (j.constraint != nullptr && (j.a == body || j.b == body)) return true;
        return false;
    }
};
