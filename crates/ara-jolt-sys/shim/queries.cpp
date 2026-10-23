#include "internal.h"
#include <Jolt/Physics/Collision/CastResult.h>
#include <Jolt/Physics/Collision/CollisionCollectorImpl.h>
#include <Jolt/Physics/Collision/RayCast.h>
#include <Jolt/Physics/Collision/ShapeCast.h>

namespace {
class Layers final : public ObjectLayerFilter {
public:
    uint32_t mask;
    explicit Layers(uint32_t m) : mask(m) {}
    bool ShouldCollide(ObjectLayer layer) const override { return layer < 16 && (mask & (1u << layer)); }
};
class Bodies final : public BodyFilter {
public:
    uint32_t ignore; bool sensors;
    Bodies(uint32_t i, bool s) : ignore(i), sensors(s) {}
    bool ShouldCollide(const BodyID& id) const override { return id.GetIndexAndSequenceNumber() != ignore; }
    bool ShouldCollideLocked(const Body& body) const override { return sensors || !body.IsSensor(); }
};

// Keep one result on the stack. Do not lower the early-out fraction: equal-distance
// hits must all reach AddHit so broadphase traversal cannot decide a gameplay tie.
class StableClosestRay final : public CastRayCollector {
public:
    bool found = false;
    RayCastResult best;
    void AddHit(const RayCastResult& hit) override {
        const auto id = hit.mBodyID.GetIndexAndSequenceNumber();
        const auto previous = best.mBodyID.GetIndexAndSequenceNumber();
        if (!found || hit.mFraction < best.mFraction || (hit.mFraction == best.mFraction &&
            (id < previous || (id == previous && hit.mSubShapeID2.GetValue() < best.mSubShapeID2.GetValue())))) {
            best = hit; found = true;
        }
    }
};

void query(const AraWorld* w, const AraQuery& q, std::vector<AraHit>& out) {
    require(q.kind <= 2 && finite(q.position) && finite(q.displacement) && unit(q.rotation), "invalid query");
    Layers layers(q.layer_mask); Bodies bodies(q.ignore,q.sensors != 0);
    const auto& narrow = w->system.GetNarrowPhaseQueryNoLock();
    if (q.kind == 0) {
        RRayCast ray{RVec3(v3(q.position)),v3(q.displacement)};
        AllHitCollisionCollector<CastRayCollector> hits;
        narrow.CastRay(ray,RayCastSettings{},hits,{},layers,bodies);
        for (const auto& h : hits.mHits) {
            auto point = ray.GetPointOnRay(h.mFraction); auto* b = w->find(h.mBodyID.GetIndexAndSequenceNumber());
            if (b) out.push_back({h.mBodyID.GetIndexAndSequenceNumber(),h.mSubShapeID2.GetValue(),h.mFraction,0,
                                a3(Vec3(point)),a3(b->GetWorldSpaceSurfaceNormal(h.mSubShapeID2,point)),b->GetUserData()});
        }
    } else {
        const Shape* shape = w->shape(q.shape);
        require(!shape->MustBeStatic(),"query geometry must be convex or a compound of convex shapes");
        auto transform = RMat44::sRotationTranslation(q4(q.rotation),RVec3(v3(q.position))) * Mat44::sTranslation(shape->GetCenterOfMass());
        if (q.kind == 1) {
            RShapeCast cast(shape,Vec3::sOne(),transform,v3(q.displacement));
            AllHitCollisionCollector<CastShapeCollector> hits;
            narrow.CastShape(cast,ShapeCastSettings{},RVec3::sZero(),hits,{},layers,bodies);
            for (const auto& h : hits.mHits) {
                auto* b = w->find(h.mBodyID2.GetIndexAndSequenceNumber());
                if (b) out.push_back({h.mBodyID2.GetIndexAndSequenceNumber(),h.mSubShapeID2.GetValue(),h.mFraction,h.mPenetrationDepth,
                    a3(Vec3(h.mContactPointOn2)),a3(-h.mPenetrationAxis.NormalizedOr(Vec3::sAxisY())),b->GetUserData()});
            }
        } else {
            AllHitCollisionCollector<CollideShapeCollector> hits;
            narrow.CollideShape(shape,Vec3::sOne(),transform,CollideShapeSettings{},RVec3::sZero(),hits,{},layers,bodies);
            for (const auto& h : hits.mHits) {
                auto* b = w->find(h.mBodyID2.GetIndexAndSequenceNumber());
                if (b) out.push_back({h.mBodyID2.GetIndexAndSequenceNumber(),h.mSubShapeID2.GetValue(),0,h.mPenetrationDepth,
                    a3(Vec3(h.mContactPointOn2)),a3(-h.mPenetrationAxis.NormalizedOr(Vec3::sAxisY())),b->GetUserData()});
            }
        }
    }
}
}

extern "C" uint32_t ara_ray_closest(const AraWorld* w, const AraQuery* q, AraHit* hit) {
    try {
        require(w && q && hit && q->kind == 0 && finite(q->position) && finite(q->displacement), "invalid closest ray");
        Layers layers(q->layer_mask); Bodies bodies(q->ignore, q->sensors != 0);
        RRayCast ray{RVec3(v3(q->position)), v3(q->displacement)};
        StableClosestRay collector;
        w->system.GetNarrowPhaseQueryNoLock().CastRay(ray, RayCastSettings{}, collector, {}, layers, bodies);
        if (!collector.found) return 0;
        const auto& h = collector.best;
        auto point = ray.GetPointOnRay(h.mFraction);
        auto* body = w->find(h.mBodyID.GetIndexAndSequenceNumber());
        require(body != nullptr, "closest ray returned a stale body");
        *hit = {h.mBodyID.GetIndexAndSequenceNumber(), h.mSubShapeID2.GetValue(), h.mFraction, 0,
            a3(Vec3(point)), a3(body->GetWorldSpaceSurfaceNormal(h.mSubShapeID2, point)), body->GetUserData()};
        return 1;
    } ARA_CATCH(kInvalid)
}

extern "C" uint32_t ara_queries(const AraWorld* w, const AraQuery* queries, uint32_t count, AraHit* hits, uint32_t capacity, uint32_t* offsets) {
    try {
        require(w && offsets && (!count || queries) && (!capacity || hits), "invalid query buffers");
        std::vector<AraHit> results;
        uint32_t written = 0; offsets[0] = 0;
        for (uint32_t i=0; i<count; ++i) {
            results.clear(); query(w,queries[i],results);
            // Do not use broadphase traversal order or collector order in gameplay.
            std::sort(results.begin(),results.end(),[](const AraHit& a,const AraHit& b) {
                if (a.fraction != b.fraction) return a.fraction < b.fraction;
                if (a.body != b.body) return a.body < b.body;
                if (a.subshape != b.subshape) return a.subshape < b.subshape;
                if (a.depth != b.depth) return a.depth > b.depth;
                if (a.point.x != b.point.x) return a.point.x < b.point.x;
                if (a.point.y != b.point.y) return a.point.y < b.point.y;
                return a.point.z < b.point.z;
            });
            require(results.size() <= capacity - written, "query output capacity exhausted");
            if (!results.empty()) std::copy(results.begin(),results.end(),hits+written);
            written += uint32_t(results.size()); offsets[i+1] = written;
        }
        return 1;
    } ARA_CATCH(0)
}
