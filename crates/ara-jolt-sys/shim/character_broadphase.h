// Spatial candidate pruning around Jolt's exact character collision implementation.
// Stable IDs determine narrowphase order; the tree is a rebuildable service cache.
#pragma once
#include <Jolt/Physics/Collision/ShapeCast.h>
#include <Jolt/Geometry/RayAABox.h>
class SpatialCharacters final : public JPH::CharacterVsCharacterCollision {
    struct Node {
        JPH::AABox bounds = JPH::AABox();
        JPH::CharacterVirtual* character = nullptr;
    };
    std::vector<JPH::CharacterVirtual*> entries;
    mutable std::vector<Node> nodes;
    mutable std::vector<size_t> leaves;
    mutable std::vector<JPH::CharacterVirtual*> sorted;
    mutable JPH::CharacterVsCharacterCollisionSimple candidates;
    JPH::CharacterVsCharacterCollisionSimple reference;
    mutable bool dirty = true;
    bool use_tree = true;
    static JPH::AABox conservative(JPH::AABox box, JPH::Vec3Arg padding) {
        // Narrowphase subtracts a nearby origin before computing bounds. An
        // absolute cache must retain padding smaller than a world-coordinate ULP.
        // Four relative epsilons cover transformed-bound and Minkowski rounding;
        // the small absolute floor also covers coordinates close to zero. Keep
        // this vectorized: casts test many tree nodes per character per tick.
        auto magnitude = JPH::Vec3::sMax(box.mMin.Abs(), box.mMax.Abs()) + padding.Abs();
        auto guard = JPH::Vec3::sMax(magnitude * (4.0f * FLT_EPSILON), JPH::Vec3::sReplicate(1.0e-6f));
        box.ExpandBy(padding + guard);
        return box;
    }
    static JPH::AABox bounds(JPH::CharacterVirtual* c) {
        auto box = c->GetShape()->GetWorldSpaceBounds(c->GetCenterOfMassTransform().ToMat44(), JPH::Vec3::sOne());
        return conservative(box, JPH::Vec3::sReplicate(c->GetCharacterPadding()));
    }
    void rebuild() const {
        if (!dirty) return;
        size_t base = 1;
        while (base < entries.size()) base *= 2;
        nodes.clear(); nodes.resize(base * 2);
        sorted.assign(entries.begin(), entries.end());
        std::sort(sorted.begin(), sorted.end(), [](auto* a, auto* b) {
            auto pa = a->GetPosition(); auto pb = b->GetPosition();
            if (pa.GetX() != pb.GetX()) return pa.GetX() < pb.GetX();
            if (pa.GetZ() != pb.GetZ()) return pa.GetZ() < pb.GetZ();
            return a->GetID().GetValue() < b->GetID().GetValue();
        });
        for (size_t i=0; i<sorted.size(); ++i) {
            auto* c = sorted[i]; size_t id = size_t(c->GetUserData());
            if (leaves.size() <= id) leaves.resize(id + 1);
            leaves[id] = base + i;
            nodes[base + i] = {bounds(c), c};
        }
        for (size_t i=base; i-- > 1;) {
            nodes[i].bounds = nodes[i*2].bounds;
            nodes[i].bounds.Encapsulate(nodes[i*2+1].bounds);
        }
        candidates.mCharacters.reserve(entries.size());
        dirty = false;
    }
    template<class Test> void select(Test&& overlaps) const {
        rebuild(); candidates.mCharacters.clear();
        size_t stack[64]; size_t top = 0;
        stack[top++] = 1;
        while (top) {
            size_t i = stack[--top];
            const auto& n = nodes[i];
            if (!overlaps(n.bounds)) continue;
            if (n.character) candidates.mCharacters.push_back(n.character);
            else if (i * 2 < nodes.size()) { stack[top++] = i*2; stack[top++] = i*2+1; }
        }
        std::sort(candidates.mCharacters.begin(), candidates.mCharacters.end(), [](auto* a, auto* b) {
            return a->GetID().GetValue() < b->GetID().GetValue();
        });
    }
public:
    void SetUseTree(bool value) { use_tree = value; dirty = true; }
    void Add(JPH::CharacterVirtual* c) { entries.push_back(c); reference.Add(c); dirty = true; }
    void Remove(JPH::CharacterVirtual* c) {
        auto found = std::find(entries.begin(), entries.end(), c);
        if (found != entries.end()) entries.erase(found);
        reference.Remove(c);
        dirty = true;
    }
    void Invalidate() { dirty = true; }
    void Update(JPH::CharacterVirtual* c) {
        if (dirty || !use_tree) return;
        size_t i = leaves[size_t(c->GetUserData())];
        nodes[i].bounds = bounds(c);
        for (i /= 2; i; i /= 2) {
            nodes[i].bounds = nodes[i*2].bounds;
            nodes[i].bounds.Encapsulate(nodes[i*2+1].bounds);
        }
    }
    void CollideCharacter(const JPH::CharacterVirtual* c, JPH::RMat44Arg transform, const JPH::CollideShapeSettings& settings, JPH::RVec3Arg offset, JPH::CollideShapeCollector& out) const override {
        if (!use_tree) { reference.CollideCharacter(c, transform, settings, offset, out); return; }
        auto box = c->GetShape()->GetWorldSpaceBounds(transform.ToMat44(), JPH::Vec3::sOne());
        box = conservative(box, JPH::Vec3::sReplicate(settings.mMaxSeparationDistance));
        select([&](const JPH::AABox& other) { return box.Overlaps(other); });
        candidates.CollideCharacter(c, transform, settings, offset, out);
    }
    void CastCharacter(const JPH::CharacterVirtual* c, JPH::RMat44Arg transform, JPH::Vec3Arg direction, const JPH::ShapeCastSettings& settings, JPH::RVec3Arg offset, JPH::CastShapeCollector& out) const override {
        if (!use_tree) { reference.CastCharacter(c, transform, direction, settings, offset, out); return; }
        auto box = c->GetShape()->GetWorldSpaceBounds(transform.ToMat44(), JPH::Vec3::sOne());
        box = conservative(box, JPH::Vec3::sZero());
        auto extent = box.GetExtent() + JPH::Vec3::sReplicate(settings.mExtraConvexRadius);
        auto origin = box.GetCenter();
        JPH::RayInvDirection inverse(direction);
        // Jolt can cast beyond fraction one to account for character padding.
        // Clip to the collector's limit, never to the unpadded displacement.
        select([&](const JPH::AABox& other) {
            auto expanded = conservative(other, extent);
            return JPH::RayAABoxHits(origin, inverse, expanded.mMin, expanded.mMax, out.GetEarlyOutFraction());
        });
        candidates.CastCharacter(c, transform, direction, settings, offset, out);
    }
};
