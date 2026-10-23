#include "internal.h"
#include <Jolt/Physics/Collision/Shape/BoxShape.h>
#include <Jolt/Physics/Collision/Shape/SphereShape.h>
#include <Jolt/Physics/Collision/Shape/CapsuleShape.h>
#include <Jolt/Physics/Collision/Shape/CylinderShape.h>
#include <Jolt/Physics/Collision/Shape/ConvexHullShape.h>
#include <Jolt/Physics/Collision/Shape/MeshShape.h>
#include <Jolt/Physics/Collision/Shape/HeightFieldShape.h>
#include <Jolt/Physics/Collision/Shape/StaticCompoundShape.h>

extern "C" uint32_t ara_shape_create(AraWorld* w, const AraShapeDesc* d, const AraVec3* vertices, uint32_t vertex_count,
    const uint32_t* indices, uint32_t index_count, const float* samples, uint32_t sample_len,
    const AraShapePart* parts, uint32_t part_count) {
    try {
        require(w && d, "null shape settings");
        require(finite(d->size) && finite(d->offset) && finite(d->radius) && finite(d->half_height), "nonfinite shape settings");
        require((vertices || !vertex_count) && (indices || !index_count) && (samples || !sample_len) && (parts || !part_count), "null geometry");
        for (uint32_t i=0; i<vertex_count; ++i) require(finite(vertices[i]), "nonfinite vertex");
        ShapeSettings::ShapeResult r;
        switch (d->kind) {
        case 0: {
            auto h = v3(d->size); require(h.ReduceMin() > 0, "nonpositive box extent");
            r = BoxShapeSettings(h, std::min(cDefaultConvexRadius, 0.5f * h.ReduceMin())).Create(); break;
        }
        case 1: require(d->radius > 0, "nonpositive sphere radius"); r = SphereShapeSettings(d->radius).Create(); break;
        case 2: require(d->radius > 0 && d->half_height >= 0, "invalid capsule"); r = CapsuleShapeSettings(d->half_height, d->radius).Create(); break;
        case 3: require(d->radius > 0 && d->half_height > 0, "invalid cylinder"); r = CylinderShapeSettings(d->half_height, d->radius, std::min(cDefaultConvexRadius, 0.5f * std::min(d->half_height,d->radius))).Create(); break;
        case 4: {
            require(vertex_count >= 4, "convex hull needs four vertices");
            Array<Vec3> points; points.reserve(vertex_count);
            for (uint32_t i=0; i<vertex_count; ++i) points.push_back(v3(vertices[i]));
            r = ConvexHullShapeSettings(points).Create(); break;
        }
        case 5: {
            require(index_count > 0 && index_count % 3 == 0, "mesh indices must contain triangles");
            VertexList verts; verts.reserve(vertex_count);
            for (uint32_t i=0; i<vertex_count; ++i) verts.emplace_back(vertices[i].x,vertices[i].y,vertices[i].z);
            IndexedTriangleList triangles; triangles.reserve(index_count/3);
            for (uint32_t i=0; i<index_count; i+=3) {
                require(indices[i] < vertex_count && indices[i+1] < vertex_count && indices[i+2] < vertex_count, "mesh index out of bounds");
                triangles.emplace_back(indices[i],indices[i+1],indices[i+2]);
            }
            r = MeshShapeSettings(std::move(verts),std::move(triangles)).Create(); break;
        }
        case 6: {
            require(d->sample_count >= 4 && uint64_t(d->sample_count)*d->sample_count == sample_len && v3(d->size).ReduceMin() > 0, "invalid heightfield dimensions");
            for (uint32_t i=0; i<sample_len; ++i) require(finite(samples[i]), "nonfinite height sample");
            r = HeightFieldShapeSettings(samples,v3(d->offset),v3(d->size),d->sample_count).Create(); break;
        }
        case 7: {
            require(part_count > 0, "empty compound");
            StaticCompoundShapeSettings s;
            for (uint32_t i=0; i<part_count; ++i) {
                require(finite(parts[i].position) && unit(parts[i].rotation), "invalid compound pose");
                s.AddShape(v3(parts[i].position),q4(parts[i].rotation),w->shape(parts[i].shape));
            }
            r = s.Create(); break;
        }
        default: throw std::runtime_error("unknown shape kind");
        }
        require(!r.HasError(), r.HasError() ? r.GetError().c_str() : "shape creation failed");
        w->shapes.push_back(r.Get()); ++w->structure;
        return uint32_t(w->shapes.size()-1);
    } ARA_CATCH(kInvalid)
}
