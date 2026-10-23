// Keep Jolt's robust fallback while exposing evidence for sizing the fixed arena.
#pragma once
class MeasuredTemp final : public JPH::TempAllocator {
    JPH::TempAllocatorImpl arena;
    JPH::TempAllocatorMalloc fallback;
public:
    size_t peak = 0;
    uint64_t fallback_count = 0;
    explicit MeasuredTemp(uint32_t bytes) : arena(bytes) {}
    void* Allocate(JPH::uint bytes) override {
        if (!arena.CanAllocate(bytes)) { ++fallback_count; return fallback.Allocate(bytes); }
        void* p = arena.Allocate(bytes);
        peak = std::max(peak, arena.GetUsage());
        return p;
    }
    void Free(void* p, JPH::uint bytes) override {
        if (!p || arena.OwnsMemory(p)) arena.Free(p, bytes);
        else fallback.Free(p, bytes);
    }
};
