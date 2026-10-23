#include "nav.h"
#include "Recast.h"
#include "DetourNavMeshBuilder.h"
#include "DetourNavMesh.h"
#include "DetourAlloc.h"
#include <algorithm>
#include <cmath>
#include <cstring>
#include <memory>
#include <vector>

namespace {
struct Tile { unsigned char* data = nullptr; int size = 0; ~Tile() { dtFree(data); } };
template<class T, void (*Free)(T*)> using Rc = std::unique_ptr<T, decltype(Free)>;
}

extern "C" int an_bake(const AnConfig* input, const float* verts, int nv,
    const int* tris, const unsigned char* areas, int nt, const AnLink* links,
    int nl, const AnBox* boxes, int nb, void** output) {
    *output = nullptr;
    try {
        rcContext ctx(false);
        rcConfig c{};
        c.cs = input->cell_size; c.ch = input->cell_height;
        c.walkableSlopeAngle = input->slope;
        c.walkableHeight = static_cast<int>(std::ceil(input->height / c.ch));
        c.walkableClimb = static_cast<int>(std::floor(input->climb / c.ch));
        c.walkableRadius = static_cast<int>(std::ceil(input->radius / c.cs));
        c.maxEdgeLen = input->tile_size;
        c.maxSimplificationError = input->error;
        c.minRegionArea = 0; c.mergeRegionArea = 0;
        c.maxVertsPerPoly = DT_VERTS_PER_POLYGON;
        c.tileSize = input->tile_size; c.borderSize = c.walkableRadius + 3;
        c.width = c.tileSize + c.borderSize * 2; c.height = c.width;
        std::memcpy(c.bmin, input->min, sizeof(c.bmin));
        std::memcpy(c.bmax, input->max, sizeof(c.bmax));
        for (int axis : {0, 2}) { c.bmin[axis] -= c.borderSize * c.cs; c.bmax[axis] += c.borderSize * c.cs; }
        Rc<rcHeightfield, rcFreeHeightField> hf(rcAllocHeightfield(), rcFreeHeightField);
        Rc<rcCompactHeightfield, rcFreeCompactHeightfield> chf(rcAllocCompactHeightfield(), rcFreeCompactHeightfield);
        Rc<rcContourSet, rcFreeContourSet> contours(rcAllocContourSet(), rcFreeContourSet);
        Rc<rcPolyMesh, rcFreePolyMesh> mesh(rcAllocPolyMesh(), rcFreePolyMesh);
        Rc<rcPolyMeshDetail, rcFreePolyMeshDetail> detail(rcAllocPolyMeshDetail(), rcFreePolyMeshDetail);
        if (!hf || !chf || !contours || !mesh || !detail) return 1;
        if (!rcCreateHeightfield(&ctx, *hf, c.width, c.height, c.bmin, c.bmax, c.cs, c.ch)) return 2;
        std::vector<unsigned char> walkable(nt, 0);
        rcMarkWalkableTriangles(&ctx, c.walkableSlopeAngle, verts, nv, tris, nt, walkable.data());
        for (int i = 0; i < nt; ++i) if (walkable[i]) walkable[i] = areas[i] == 255 ? 0 : areas[i] + 1;
        if (nt && !rcRasterizeTriangles(&ctx, verts, nv, tris, walkable.data(), nt, *hf, c.walkableClimb)) return 3;
        rcFilterLowHangingWalkableObstacles(&ctx, c.walkableClimb, *hf);
        rcFilterLedgeSpans(&ctx, c.walkableHeight, c.walkableClimb, *hf);
        rcFilterWalkableLowHeightSpans(&ctx, c.walkableHeight, *hf);
        if (!rcBuildCompactHeightfield(&ctx, c.walkableHeight, c.walkableClimb, *hf, *chf)) return 4;
        hf.reset();
        if (!rcErodeWalkableArea(&ctx, c.walkableRadius, *chf)) return 5;
        for (int i = 0; i < nb; ++i) {
            float lo[3], hi[3];
            std::memcpy(lo, boxes[i].min, sizeof(lo)); std::memcpy(hi, boxes[i].max, sizeof(hi));
            for (int axis : {0, 2}) { lo[axis] -= input->radius; hi[axis] += input->radius; }
            lo[1] -= input->height;
            // Walkable spans store area + 1 until the final decrement below.
            const unsigned char area = boxes[i].area == 255 ? RC_NULL_AREA : static_cast<unsigned char>(boxes[i].area + 1);
            rcMarkBoxArea(&ctx, lo, hi, area, *chf);
        }
        if (!rcBuildDistanceField(&ctx, *chf) || !rcBuildRegions(&ctx, *chf, c.borderSize, 0, 0)) return 6;
        if (!rcBuildContours(&ctx, *chf, c.maxSimplificationError, c.maxEdgeLen, *contours)) return 7;
        if (!rcBuildPolyMesh(&ctx, *contours, c.maxVertsPerPoly, *mesh)) return 8;
        auto tile = std::make_unique<Tile>();
        if (!mesh->npolys) { *output = tile.release(); return 0; }
        if (mesh->nverts >= 65535) return 9;
        if (!rcBuildPolyMeshDetail(&ctx, *mesh, *chf, c.cs * 6, c.ch, *detail)) return 10;
        for (int i = 0; i < mesh->npolys; ++i) { --mesh->areas[i]; mesh->flags[i] = static_cast<unsigned short>(1u << mesh->areas[i]); }
        std::vector<float> endpoints(nl * 6), radii(nl);
        std::vector<unsigned char> directions(nl), linkAreas(nl);
        std::vector<unsigned short> flags(nl);
        std::vector<unsigned int> ids(nl);
        for (int i = 0; i < nl; ++i) {
            std::memcpy(&endpoints[i*6], links[i].start, 12); std::memcpy(&endpoints[i*6+3], links[i].end, 12);
            radii[i] = links[i].radius; directions[i] = links[i].bidirectional ? DT_OFFMESH_CON_BIDIR : 0;
            linkAreas[i] = links[i].area; flags[i] = static_cast<unsigned short>(1u << links[i].area); ids[i] = links[i].id;
        }
        dtNavMeshCreateParams p{};
        p.verts = mesh->verts; p.vertCount = mesh->nverts; p.polys = mesh->polys;
        p.polyAreas = mesh->areas; p.polyFlags = mesh->flags; p.polyCount = mesh->npolys; p.nvp = mesh->nvp;
        p.detailMeshes = detail->meshes; p.detailVerts = detail->verts; p.detailVertsCount = detail->nverts;
        p.detailTris = detail->tris; p.detailTriCount = detail->ntris;
        p.offMeshConVerts = endpoints.data(); p.offMeshConRad = radii.data(); p.offMeshConDir = directions.data();
        p.offMeshConAreas = linkAreas.data(); p.offMeshConFlags = flags.data(); p.offMeshConUserID = ids.data(); p.offMeshConCount = nl;
        p.walkableHeight = input->height; p.walkableRadius = input->radius; p.walkableClimb = input->climb;
        p.tileX = input->tile_x; p.tileY = input->tile_z;
        std::memcpy(p.bmin, mesh->bmin, 12); std::memcpy(p.bmax, mesh->bmax, 12);
        p.cs = c.cs; p.ch = c.ch; p.buildBvTree = true;
        if (!dtCreateNavMeshData(&p, &tile->data, &tile->size)) return 11;
        *output = tile.release(); return 0;
    } catch (...) { return 12; }
}
extern "C" const unsigned char* an_tile_bytes(const void* tile, int* size) {
    const auto* t = static_cast<const Tile*>(tile); *size = t->size; return t->data;
}
extern "C" void an_tile_free(void* tile) { delete static_cast<Tile*>(tile); }
