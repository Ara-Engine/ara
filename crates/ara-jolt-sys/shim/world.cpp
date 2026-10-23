#include "internal.h"
#include <Jolt/RegisterTypes.h>
#include <cstdio>

namespace {
thread_local char last_error[512] = {};
void global_init() {
    static std::once_flag once;
    std::call_once(once, [] {
        RegisterDefaultAllocator();
        Factory::sInstance = new Factory();
        RegisterTypes();
    });
}
uint64_t checksum(const uint8_t* data, size_t len) {
    // Trusted native snapshots need accidental-corruption detection, not a wire
    // signature. Hash whole words to avoid one serial multiply for every byte.
    uint64_t h = 14695981039346656037ull;
    size_t i=0;
    for (; len-i>=sizeof(uint64_t); i+=sizeof(uint64_t)) {
        uint64_t word;std::memcpy(&word,data+i,sizeof(word));h^=word;h*=1099511628211ull;
    }
    for (;i<len;++i) {h^=data[i];h*=1099511628211ull;}
    return h^uint64_t(len);
}
}
void set_error(const char* message) noexcept { std::snprintf(last_error, sizeof(last_error), "%s", message); }

AraWorld::AraWorld(const AraWorldDesc& d) : contacts(d.event_capacity) {
    require(d.max_bodies > 0 && d.max_bodies <= BodyID::cMaxBodyIndex && d.threads <= 64, "invalid world capacity/thread count");
    require(d.scratch_mb > 0 && d.scratch_mb <= 1024 && finite(d.gravity), "invalid scratch/gravity");
    require(d.velocity_steps > 0 && d.velocity_steps <= 255 && d.position_steps > 0 && d.position_steps <= 255, "invalid solver iterations");
    require(d.max_pairs > 0 && d.max_contacts > 0, "invalid collision capacity");
    for (uint32_t i = 0; i < 16; ++i) {
        bp_layers.MapObjectToBroadPhaseLayer(ObjectLayer(i), BroadPhaseLayer(i == 0 ? 0 : 1));
        for (uint32_t j = 0; j < 16; ++j) {
            require(((d.layer_masks[i] >> j) & 1) == ((d.layer_masks[j] >> i) & 1), "asymmetric collision matrix");
            if ((d.layer_masks[i] >> j) & 1) pairs.EnableCollision(ObjectLayer(i), ObjectLayer(j));
        }
    }
    object_bp = std::make_unique<ObjectVsBroadPhaseLayerFilterTable>(bp_layers, 2, pairs, 16);
    require(finite(d.speculative_contact_distance) && d.speculative_contact_distance >= 0
        && finite(d.time_before_sleep) && d.time_before_sleep > 0
        && finite(d.sleep_velocity_threshold) && d.sleep_velocity_threshold > 0
        && d.settings_flags < 8, "invalid physics tuning");
    temp = std::make_unique<MeasuredTemp>(d.scratch_mb * 1024 * 1024);
    if (d.threads <= 1) jobs = std::make_unique<JobSystemSingleThreaded>(cMaxPhysicsJobs);
    else if (d.settings_flags & 4) {
        auto host = std::make_unique<HostJobs>(d.threads);
        host_jobs = host.get(); jobs = std::move(host);
    }
    else jobs = std::make_unique<JobSystemThreadPool>(cMaxPhysicsJobs, cMaxPhysicsBarriers, int(d.threads) - 1);
    system.Init(d.max_bodies, 0, d.max_pairs, d.max_contacts, bp_layers, *object_bp, pairs);
    system.SetGravity(v3(d.gravity));
    auto s = system.GetPhysicsSettings();
    s.mNumVelocitySteps = d.velocity_steps;
    s.mNumPositionSteps = d.position_steps;
    s.mDeterministicSimulation = (d.settings_flags & 1) != 0;
    s.mUseLargeIslandSplitter = (d.settings_flags & 2) != 0;
    s.mSpeculativeContactDistance = d.speculative_contact_distance;
    s.mTimeBeforeSleep = d.time_before_sleep;
    s.mPointVelocitySleepThreshold = d.sleep_velocity_threshold;
    system.SetPhysicsSettings(s);
    if (d.event_capacity) system.SetContactListener(&contacts);
}
AraWorld::~AraWorld() {
    system.SetContactListener(nullptr);
    for (auto& v : vehicles) if (v.constraint != nullptr) {
        system.RemoveStepListener(v.constraint);
        system.RemoveConstraint(v.constraint);
    }
    for (auto& j : joints) if (j.constraint != nullptr) system.RemoveConstraint(j.constraint);
    vehicles.clear(); joints.clear(); characters.clear();
}

extern "C" {
void ara_jobs_begin(AraWorld* w, uint32_t workers) { w->host_jobs->Begin(workers); }
void ara_jobs_end(AraWorld* w) { w->host_jobs->End(); }
void ara_jobs_run(AraWorld* w) { w->host_jobs->Run(); }
size_t ara_last_error(char* out, size_t capacity) {
    size_t len = std::strlen(last_error) + 1;
    if (out && capacity) { size_t n = std::min(len, capacity); std::memcpy(out, last_error, n); out[n-1] = 0; }
    return len;
}
uint32_t ara_abi_layout(uint32_t* out, uint32_t capacity) {
    const uint32_t values[] = {
#include "abi_layout.inc"
    };
    uint32_t count = uint32_t(sizeof(values) / sizeof(values[0]));
    if (out) std::copy_n(values, std::min(count, capacity), out);
    return count;
}
uint32_t ara_deterministic() {
#ifdef JPH_CROSS_PLATFORM_DETERMINISTIC
    return 1;
#else
    return 0;
#endif
}
AraWorld* ara_world_create(const AraWorldDesc* d) {
    try { require(d != nullptr, "null world settings"); global_init(); return new AraWorld(*d); } ARA_CATCH(nullptr)
}
void ara_world_destroy(AraWorld* world) { try { delete world; } catch (...) { set_error("world destructor failed"); } }
uint32_t ara_world_step(AraWorld* world, float dt) {
    try {
        require(world && finite(dt) && dt > 0, "invalid step");
        auto error = uint32_t(world->system.Update(dt, 1, world->temp.get(), world->jobs.get()));
        if (!world->contacts.events.empty()) {
            // Derive character effects from restorable active contacts. Jolt's
            // callback bookkeeping is not saved by CharacterVirtual::SaveState.
            for (uint32_t i=0;i<world->characters.size();++i) {
                const auto& slot=world->characters[i];
                if (slot.character==nullptr) continue;
                for (const auto& c:slot.character->GetActiveContacts()) if (c.mHadCollision) {
                    bool other_character=!c.mCharacterIDB.IsInvalid();
                    uint32_t other=other_character ? c.mCharacterIDB.GetValue()-1 : c.mBodyB.GetIndexAndSequenceNumber();
                    world->contacts.push({i,other,0,c.mSubShapeIDB.GetValue(),1,uint32_t(other_character),1,uint32_t(c.mIsSensorB),
                        a3(Vec3(c.mPosition)),a3(-c.mContactNormal)});
                }
            }
        }
        if (world->contacts.count.load() > world->contacts.events.size()) {
            set_error("contact event capacity exhausted"); return error | 0x80000000u;
        }
        return error;
    } ARA_CATCH(0x80000000u)
}
uint32_t ara_world_optimize(AraWorld* w) { try { require(w, "null world"); w->system.OptimizeBroadPhase(); return 1; } ARA_CATCH(0) }
uint32_t ara_world_active(const AraWorld* w) { return w ? w->system.GetNumActiveBodies(EBodyType::RigidBody) : 0; }
uint32_t ara_contacts(AraWorld* w, AraContact* out, uint32_t capacity) {
    try {
        require(w, "null world");
        uint32_t count = w->contacts.count.load();
        require(count <= w->contacts.events.size() && count <= capacity, "contact output capacity exhausted");
        require(out || count == 0, "null contact output");
        if (count) std::copy_n(w->contacts.events.data(), count, out);
        w->contacts.count.store(0);
        return count;
    } ARA_CATCH(kInvalid)
}
const uint8_t* ara_world_save(AraWorld* w, size_t* len) {
    try {
        require(w && len, "null snapshot output");
        auto& s = w->snapshot;
        s.bytes.clear();
        s.Write(w->structure);
        w->system.SaveState(s, EStateRecorderState::All);
        for (const auto& c : w->characters) if (c.character != nullptr) {
            s.Write(c.crouched);
            c.character->SaveState(s);
        }
        // Character input policy is reconstructed each tick. Vehicle driver input is
        // contained in VehicleConstraint::SaveState alongside engine and gearbox.
        auto h = checksum(s.bytes.data(), s.bytes.size());
        s.Write(h);
        *len = s.bytes.size();
        return s.bytes.data();
    } ARA_CATCH(nullptr)
}
uint32_t ara_world_restore(AraWorld* w, const uint8_t* data, size_t len) {
    try {
        require(w && data && len >= 16, "invalid snapshot envelope");
        uint64_t structure, expected;
        std::memcpy(&structure, data, 8);
        std::memcpy(&expected, data + len - 8, 8);
        require(structure == w->structure, "snapshot topology changed; reconstruct before restore");
        require(expected == checksum(data, len - 8), "snapshot checksum mismatch");
        SpanReader r(data + 8, len - 16);
        require(w->system.RestoreState(r), "Jolt snapshot restore failed");
        for (auto& c : w->characters) if (c.character != nullptr) {
            r.Read(c.crouched);
            auto shape = c.crouched ? c.crouching : c.standing;
            auto layer = ObjectLayer(c.desc.layer);
            // This is trusted restore data: restore shape without clearance rejection.
            c.character->SetShape(shape, FLT_MAX, w->system.GetDefaultBroadPhaseLayerFilter(layer),
                                  w->system.GetDefaultLayerFilter(layer), {}, {}, *w->temp);
            c.character->RestoreState(r);
        }
        require(r.pos == r.len, "snapshot trailing data");
        w->character_collision.Invalidate();
        w->contacts.count.store(0);
        return 1;
    } ARA_CATCH(0)
}
}
