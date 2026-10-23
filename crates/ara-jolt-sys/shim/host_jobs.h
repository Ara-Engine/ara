// Ara's bounded job queue. Workers are borrowed from the host for one update.
#pragma once
#include <Jolt/Core/FixedSizeFreeList.h>
#include <Jolt/Core/JobSystemWithBarrier.h>
#include <thread>

class HostJobs final : public JPH::JobSystemWithBarrier {
    using Job = JPH::JobSystem::Job;
    JPH::FixedSizeFreeList<Job> available;
    // More slots than live jobs: a producer cannot lap an outstanding consumer.
    static constexpr uint32_t capacity = 8192;
    struct Slot { std::atomic<uint64_t> sequence{0}; Job* job = nullptr; };
    Slot queue[capacity];
    alignas(64) std::atomic<uint64_t> head{0};
    alignas(64) std::atomic<uint64_t> tail{0};
    std::atomic<bool> finished{true};
    JPH::Semaphore ready;
    uint32_t concurrency;
    uint32_t participants = 0;
public:
    explicit HostJobs(uint32_t threads) : JobSystemWithBarrier(JPH::cMaxPhysicsBarriers), concurrency(threads) {
        available.Init(JPH::cMaxPhysicsJobs, JPH::cMaxPhysicsJobs);
        for (uint64_t i = 0; i < capacity; ++i) queue[i].sequence.store(i, std::memory_order_relaxed);
    }
    int GetMaxConcurrency() const override { return int(concurrency); }
    JPH::JobHandle CreateJob(const char* name, JPH::ColorArg color, const JobFunction& fn, uint32_t dependencies = 0) override {
        uint32_t index;
        while ((index = available.ConstructObject(name, color, this, fn, dependencies)) == decltype(available)::cInvalidObjectIndex)
            std::this_thread::yield();
        auto* job = &available.Get(index);
        JPH::JobHandle handle(job);
        if (!dependencies) QueueJob(job);
        return handle;
    }
    void Begin(uint32_t workers) { participants = workers; finished.store(false, std::memory_order_release); }
    void End() {
        finished.store(true, std::memory_order_release);
        ready.Release(participants);
    }
    void Run() {
        for (;;) {
            ready.Acquire();
            uint64_t next = head.load(std::memory_order_relaxed);
            while (next < tail.load(std::memory_order_acquire)) {
                if (!head.compare_exchange_weak(next, next + 1, std::memory_order_relaxed)) continue;
                auto& slot = queue[next % capacity];
                while (slot.sequence.load(std::memory_order_acquire) != next + 1) std::this_thread::yield();
                Job* job = slot.job;
                slot.sequence.store(next + capacity, std::memory_order_release);
                job->Execute();
                job->Release();
                break;
            }
            if (next >= tail.load(std::memory_order_acquire) && finished.load(std::memory_order_acquire)) return;
        }
    }
protected:
    void FreeJob(Job* job) override { available.DestructObject(job); }
    void QueueJob(Job* job) override {
        job->AddRef();
        uint64_t position = tail.fetch_add(1, std::memory_order_acq_rel);
        auto& slot = queue[position % capacity];
        while (slot.sequence.load(std::memory_order_acquire) != position) std::this_thread::yield();
        slot.job = job;
        slot.sequence.store(position + 1, std::memory_order_release);
        ready.Release();
    }
    void QueueJobs(Job** jobs, uint32_t count) override {
        for (uint32_t i = 0; i < count; ++i) QueueJob(jobs[i]);
    }
};
