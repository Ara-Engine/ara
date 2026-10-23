// Bench-only direct Jolt harness. The caller constructs two identical worlds;
// this path bypasses the C shim update and Rust effects/tick bookkeeping.
#include "internal.h"
#include <chrono>
#include <Jolt/Core/Profiler.h>
#include <array>

namespace {
using Clock = std::chrono::steady_clock;
uint64_t nanos(Clock::duration d) { return uint64_t(std::chrono::duration_cast<std::chrono::nanoseconds>(d).count()); }
std::atomic<uint64_t> phases[8]{};
std::atomic<uint64_t> update_ns{0};
}

#ifdef JPH_EXTERNAL_PROFILE
namespace {
struct Sample { Clock::time_point start; Sample* parent; uint64_t children; int phase; };
static_assert(sizeof(Sample) <= 64);
thread_local Sample* current = nullptr;
int phase_for(const char* name) {
    if (std::strstr(name, "AraBroadphase") || std::strstr(name, "BroadPhase")) return 0;
    if (std::strstr(name, "AraNarrowphase") || !std::strcmp(name,"FindCollisions") || !std::strcmp(name,"FindCCDContacts")) return 1;
    if (std::strstr(name, "AraContactSetup") || !std::strcmp(name,"SetupVelocityConstraints")) return 2;
    if (!std::strcmp(name,"SolveVelocityConstraints")) return 3;
    if (!std::strcmp(name,"SolvePositionConstraints")) return 4;
    if (std::strstr(name,"IntegrateVelocity") || !std::strcmp(name,"ApplyGravity") || !std::strcmp(name,"ResolveCCDContacts")) return 5;
    if (std::strstr(name,"Islands") || !std::strcmp(name,"BodySetIslandIndex") || !std::strcmp(name,"DetermineActiveConstraints")) return 6;
    if (!std::strcmp(name,"Build Jobs") || !std::strcmp(name,"Build job barrier") || !std::strcmp(name,"ContactRemovedCallbacks")) return 7;
    return -1;
}
}
namespace JPH {
ExternalProfileMeasurement::ExternalProfileMeasurement(const char* name, uint32) {
    auto* sample = new (mUserData) Sample{Clock::now(), current, 0, phase_for(name)};
    if (sample->phase >= 0) current = sample;
}
ExternalProfileMeasurement::~ExternalProfileMeasurement() {
    auto* sample = reinterpret_cast<Sample*>(mUserData);
    if (sample->phase < 0) return;
    uint64_t duration = nanos(Clock::now() - sample->start);
    phases[sample->phase].fetch_add(duration - sample->children, std::memory_order_relaxed);
    current = sample->parent;
    if (current) current->children += duration;
}
}
#endif

extern "C" {
uint32_t ara_bench_job_probe(AraWorld* w, uint32_t count) {
    try {
        require(w && w->host_jobs && count > 0, "job probe requires host jobs");
        std::atomic<uint32_t> executed{0};
        for (uint32_t first = 0; first < count;) {
            uint32_t batch = std::min(uint32_t(256), count - first);
            auto* barrier = w->jobs->CreateBarrier();
            {
                std::array<JobHandle, 256> handles;
                for (uint32_t i = 0; i < batch; ++i)
                    handles[i] = w->jobs->CreateJob("QueueProbe", Color::sWhite, [&executed] { executed.fetch_add(1, std::memory_order_relaxed); });
                barrier->AddJobs(handles.data(), batch);
                w->jobs->WaitForJobs(barrier);
            }
            w->jobs->DestroyBarrier(barrier);
            first += batch;
        }
        require(executed.load() == count, "host queue lost a job");
        return 0;
    } ARA_CATCH(0x80000000u)
}
void ara_bench_character_tree(AraWorld* w, uint32_t enabled) { w->character_collision.SetUseTree(enabled != 0); }
uint32_t ara_bench_native_step(AraWorld* w, float dt) {
    try {
        require(w && dt > 0 && finite(dt) && w->characters.empty(), "native comparison requires a rigid scene");
        auto start = Clock::now();
        auto status = uint32_t(w->system.Update(dt, 1, w->temp.get(), w->jobs.get()));
        update_ns = nanos(Clock::now() - start);
        bool overflow = w->contacts.count.load() > w->contacts.events.size();
        w->contacts.count.store(0);
        require(!overflow, "native benchmark contact capacity exhausted");
        return status;
    } ARA_CATCH(0x80000000u)
}
void ara_bench_reset(AraWorld* w) {
    for (auto& phase : phases) phase.store(0, std::memory_order_relaxed);
    w->temp->peak = 0;
    w->temp->fallback_count = 0;
    update_ns = 0;
}
void ara_bench_stats(const AraWorld* w, uint64_t* out) {
    for (int i=0; i<8; ++i) out[i] = phases[i].load(std::memory_order_relaxed);
    out[8] = update_ns.load(std::memory_order_relaxed);
    out[9] = w->temp->peak;
    out[10] = w->temp->fallback_count;
}
}
