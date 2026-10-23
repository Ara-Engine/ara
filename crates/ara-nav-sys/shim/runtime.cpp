#include "nav.h"
#include "DetourNavMesh.h"
#include "DetourNavMeshQuery.h"
#include "DetourCrowd.h"
#include "DetourAlloc.h"
#include "DetourCommon.h"
#include <algorithm>
#include <cmath>
#include <cstring>
#include <memory>
#include <vector>

namespace {
struct Query {
    dtNavMeshQuery* query = dtAllocNavMeshQuery();
    std::vector<dtPolyRef> polys, refs;
    std::vector<float> points;
    std::vector<unsigned char> flags;
    ~Query() { dtFreeNavMeshQuery(query); }
};
// Area filter plus runtime-blocked off-mesh links (closed doors, absent elevators),
// so gameplay can gate connections without rebaking. Needs DT_VIRTUAL_QUERYFILTER.
struct LinkFilter : dtQueryFilter {
    const unsigned int* blocked = nullptr; int count = 0;
    bool passFilter(const dtPolyRef ref, const dtMeshTile* tile, const dtPoly* poly) const override {
        if (!dtQueryFilter::passFilter(ref, tile, poly)) return false;
        if (count == 0 || poly->getType() != DT_POLYTYPE_OFFMESH_CONNECTION) return true;
        const int index = static_cast<int>(poly - tile->polys) - tile->header->offMeshBase;
        if (index < 0 || index >= tile->header->offMeshConCount) return true;
        return !std::binary_search(blocked, blocked + count, tile->offMeshCons[index].userId);
    }
};
LinkFilter filter(const AnFilter& f) {
    LinkFilter result; result.setIncludeFlags(f.include); result.setExcludeFlags(f.exclude);
    for (int i=0;i<16;++i) result.setAreaCost(i,f.costs[i]);
    result.blocked = f.blocked; result.count = f.blocked_count > 0 ? f.blocked_count : 0;
    return result;
}
// The byte format is private, local-endian, pinned to this Detour revision.
// Check every index/count used by addTile/query before trusting a cached blob.
bool valid(const unsigned char* bytes, int size) {
    if (size < static_cast<int>(sizeof(dtMeshHeader))) return false;
    dtMeshHeader h; std::memcpy(&h,bytes,sizeof(h));
    if (h.magic != DT_NAVMESH_MAGIC || h.version != DT_NAVMESH_VERSION || h.layer != 0) return false;
    if(h.x < -1000000 || h.x > 1000000 || h.y < -1000000 || h.y > 1000000 ||
        !std::isfinite(h.bvQuantFactor) || h.bvQuantFactor<=0 ||
        !std::isfinite(h.walkableHeight) || h.walkableHeight<=0 ||
        !std::isfinite(h.walkableRadius) || h.walkableRadius<0 ||
        !std::isfinite(h.walkableClimb) || h.walkableClimb<0) return false;
    for(int i=0;i<3;++i) if(!std::isfinite(h.bmin[i]) || !std::isfinite(h.bmax[i]) ||
        std::abs(h.bmin[i])>1000000 || std::abs(h.bmax[i])>1000000 || h.bmin[i]>h.bmax[i]) return false;
    for (int n : {h.polyCount,h.vertCount,h.maxLinkCount,h.detailMeshCount,h.detailVertCount,h.detailTriCount,h.bvNodeCount,h.offMeshConCount})
        if (n < 0 || n > 1000000) return false;
    if (h.vertCount >= 65535 || h.polyCount > 32768 || h.offMeshBase != h.detailMeshCount ||
        h.offMeshBase+h.offMeshConCount != h.polyCount || h.bvNodeCount > h.detailMeshCount*2) return false;
    size_t offset = dtAlign4(sizeof(h));
    const size_t vertexOffset=offset; offset += dtAlign4(sizeof(float)*3*h.vertCount);
    const size_t polyOffset=offset; offset += dtAlign4(sizeof(dtPoly)*h.polyCount);
    offset += dtAlign4(sizeof(dtLink)*h.maxLinkCount);
    const size_t detailOffset=offset; offset += dtAlign4(sizeof(dtPolyDetail)*h.detailMeshCount);
    const size_t detailVertOffset=offset; offset += dtAlign4(sizeof(float)*3*h.detailVertCount);
    const size_t triangleOffset=offset; offset += dtAlign4(4*h.detailTriCount);
    const size_t bvOffset=offset; offset += dtAlign4(sizeof(dtBVNode)*h.bvNodeCount);
    const size_t linkOffset=offset; offset += dtAlign4(sizeof(dtOffMeshConnection)*h.offMeshConCount);
    if (offset != static_cast<size_t>(size)) return false;
    for (int i=0;i<h.vertCount*3;++i) { float v; std::memcpy(&v,bytes+vertexOffset+sizeof(float)*i,sizeof(v)); if (!std::isfinite(v)) return false; }
    for (int i=0;i<h.polyCount;++i) {
        dtPoly p; std::memcpy(&p,bytes+polyOffset+sizeof(p)*i,sizeof(p));
        if (p.vertCount < (i<h.offMeshBase ? 3 : 2) || p.vertCount > DT_VERTS_PER_POLYGON ||
            (i>=h.offMeshBase && p.vertCount!=2) || p.getArea() >= 16 || p.getType() != (i<h.offMeshBase ? DT_POLYTYPE_GROUND : DT_POLYTYPE_OFFMESH_CONNECTION)) return false;
        for(int j=0;j<p.vertCount;++j) {
            if (p.verts[j] >= h.vertCount) return false;
            if (!(p.neis[j]&DT_EXT_LINK) && p.neis[j] > h.polyCount) return false;
            if ((p.neis[j]&DT_EXT_LINK) && (p.neis[j]&0xff) > 7) return false;
        }
        if (i<h.detailMeshCount) {
            dtPolyDetail d; std::memcpy(&d,bytes+detailOffset+sizeof(d)*i,sizeof(d));
            if (static_cast<uint64_t>(d.vertBase)+d.vertCount>static_cast<unsigned>(h.detailVertCount) || static_cast<uint64_t>(d.triBase)+d.triCount>static_cast<unsigned>(h.detailTriCount)) return false;
            for(unsigned j=0;j<d.triCount;++j) for(int k=0;k<3;++k)
                if (bytes[triangleOffset+4*(d.triBase+j)+k] >= p.vertCount+d.vertCount) return false;
        }
    }
    for (int i=0;i<h.detailVertCount*3;++i) { float v; std::memcpy(&v,bytes+detailVertOffset+sizeof(float)*i,sizeof(v)); if(!std::isfinite(v)) return false; }
    for (int i=0;i<h.bvNodeCount;++i) {
        dtBVNode n; std::memcpy(&n,bytes+bvOffset+sizeof(n)*i,sizeof(n));
        if(n.i>=h.offMeshBase || (n.i<0 && (n.i == (-2147483647-1) || -n.i>h.bvNodeCount-i))) return false;
    }
    for(int i=0;i<h.offMeshConCount;++i) {
        dtOffMeshConnection c; std::memcpy(&c,bytes+linkOffset+sizeof(c)*i,sizeof(c));
        if(c.poly!=h.offMeshBase+i || !std::isfinite(c.rad) || c.rad<=0 || (c.side!=0xff && c.side>7)) return false;
        for(float v:c.pos) if(!std::isfinite(v)) return false;
    }
    return h.polyCount>0;
}
}
extern "C" void* an_mesh_new(const float* origin, float width, int count) {
    auto* mesh=dtAllocNavMesh(); if(!mesh) return nullptr;
    dtNavMeshParams p{}; std::memcpy(p.orig,origin,12); p.tileWidth=width; p.tileHeight=width;
    p.maxTiles=count; p.maxPolys=1 << std::min(15, 22-static_cast<int>(dtIlog2(dtNextPow2(count))));
    if(dtStatusFailed(mesh->init(&p))) { dtFreeNavMesh(mesh); return nullptr; } return mesh;
}
extern "C" void an_mesh_free(void* mesh) { dtFreeNavMesh(static_cast<dtNavMesh*>(mesh)); }
extern "C" int an_mesh_add(void* mesh,const unsigned char* bytes,int size) {
    if(!valid(bytes,size)) return 1;
    dtMeshHeader header; std::memcpy(&header,bytes,sizeof(header));
    if(header.polyCount > static_cast<dtNavMesh*>(mesh)->getParams()->maxPolys) return 4;
    auto* data=static_cast<unsigned char*>(dtAlloc(size,DT_ALLOC_PERM)); if(!data) return 2;
    std::memcpy(data,bytes,size);
    if(dtStatusFailed(static_cast<dtNavMesh*>(mesh)->addTile(data,size,DT_TILE_FREE_DATA,0,nullptr))) { dtFree(data); return 3; }
    return 0;
}
extern "C" void* an_query_new(const void* mesh,int nodes) {
    try {
        auto q=std::make_unique<Query>(); if(!q->query) return nullptr;
        if(dtStatusFailed(q->query->init(static_cast<const dtNavMesh*>(mesh),nodes))) return nullptr;
        return q.release();
    } catch(...) { return nullptr; }
}
extern "C" void an_query_free(void* query) { delete static_cast<Query*>(query); }
extern "C" unsigned int an_nearest(void* query,const float* point,const float* ext,const AnFilter* f,float* out) {
    auto ft=filter(*f); dtPolyRef ref=0;
    static_cast<Query*>(query)->query->findNearestPoly(point,ext,&ft,&ref,out); return ref;
}
extern "C" unsigned int an_path(void* query,const float* start,const float* end,const float* ext,
    const AnFilter* f,AnCorner* corners,int cap,int* count,unsigned char* complete,float* reachable) {
    try {
        *count=0; *complete=0;
        auto* scratch=static_cast<Query*>(query);auto* q=scratch->query; auto ft=filter(*f);
        float a[3],b[3]; dtPolyRef ar=0,br=0;
        auto status=q->findNearestPoly(start,ext,&ft,&ar,a);
        if(dtStatusFailed(status)||!ar) return DT_FAILURE|DT_INVALID_PARAM;
        status=q->findNearestPoly(end,ext,&ft,&br,b);
        if(dtStatusFailed(status)||!br) return DT_FAILURE|DT_INVALID_PARAM;
        auto& polys=scratch->polys;polys.resize(cap); int np=0;
        status=q->findPath(ar,br,a,b,&ft,polys.data(),&np,cap);
        if(dtStatusFailed(status)||!np) return status;
        std::memcpy(reachable,b,12);
        *complete=polys[np-1]==br && !dtStatusDetail(status,DT_BUFFER_TOO_SMALL) && !dtStatusDetail(status,DT_OUT_OF_NODES);
        if(!*complete) q->closestPointOnPoly(polys[np-1],b,reachable,nullptr);
        auto& points=scratch->points;points.resize(cap*3);
        auto& flags=scratch->flags;flags.resize(cap);
        auto& refs=scratch->refs;refs.resize(cap);
        auto straight=q->findStraightPath(a,reachable,polys.data(),np,points.data(),flags.data(),refs.data(),count,cap,DT_STRAIGHTPATH_ALL_CROSSINGS);
        if(dtStatusFailed(straight)) return straight;
        if(dtStatusDetail(straight,DT_BUFFER_TOO_SMALL)) *complete=0;
        for(int i=0;i<*count;++i) { std::memcpy(corners[i].position,&points[i*3],12); corners[i].polygon=refs[i]; corners[i].flags=flags[i]; }
        return status| (straight&DT_STATUS_DETAIL_MASK);
    } catch(...) { return DT_FAILURE|DT_OUT_OF_MEMORY; }
}
extern "C" unsigned int an_raycast(void* query,const float* start,const float* end,const float* ext,const AnFilter* f,float* t,float* normal) {
    auto* q=static_cast<Query*>(query)->query; auto ft=filter(*f); dtPolyRef ref=0; float a[3];
    q->findNearestPoly(start,ext,&ft,&ref,a); if(!ref) return DT_FAILURE|DT_INVALID_PARAM;
    return q->raycast(ref,a,end,&ft,t,normal,nullptr,nullptr,0);
}
extern "C" int an_polygon_area(void* query,unsigned int ref) {
    unsigned char area=0; auto* mesh=static_cast<Query*>(query)->query->getAttachedNavMesh();
    return dtStatusFailed(mesh->getPolyArea(ref,&area)) ? -1 : area;
}
extern "C" int an_visible(void* query,const float* start,const float* end,const float* ext,const AnFilter* f) {
    try {
        auto* scratch=static_cast<Query*>(query);auto* q=scratch->query;auto ft=filter(*f);
        dtPolyRef a=0,b=0;float ap[3],bp[3];
        q->findNearestPoly(start,ext,&ft,&a,ap);q->findNearestPoly(end,ext,&ft,&b,bp);
        if(!a || !b) return 0;
        scratch->polys.resize(4096);dtRaycastHit hit{};hit.path=scratch->polys.data();hit.maxPath=4096;
        const auto status=q->raycast(a,ap,bp,&ft,0,&hit);
        // Verify the actual ending polygon: XZ-only raycast visibility must never
        // shortcut between overlapping floors of a bridge or stacked room.
        if(dtStatusFailed(status) || dtStatusDetail(status,DT_BUFFER_TOO_SMALL) || hit.t<1.0f || hit.pathCount<=0) return 0;
        // An endpoint on a shared edge may belong to either polygon. Compare its
        // projected surface position instead of requiring the same polygon ID.
        float projected[3];
        if(dtStatusFailed(q->closestPointOnPoly(scratch->polys[hit.pathCount-1],bp,projected,nullptr))) return 0;
        return dtVdistSqr(projected,bp)<0.0025f;
    } catch(...) { return 0; }
}
extern "C" int an_debug_count(const void* nav) {
    auto* mesh=static_cast<const dtNavMesh*>(nav); int count=0;
    for(int i=0;i<mesh->getMaxTiles();++i) { const auto* t=mesh->getTile(i); if(t->header) for(int j=0;j<t->header->offMeshBase;++j) count+=t->polys[j].vertCount-2; } return count;
}
extern "C" int an_debug(const void* nav,AnDebug* out,int cap) {
    auto* mesh=static_cast<const dtNavMesh*>(nav); int count=0;
    for(int i=0;i<mesh->getMaxTiles();++i) { const auto* t=mesh->getTile(i); if(!t->header) continue;
        for(int j=0;j<t->header->offMeshBase;++j) { const auto& p=t->polys[j];
            for(int k=2;k<p.vertCount && count<cap;++k) { auto& tri=out[count++];
                std::memcpy(tri.a,&t->verts[3*p.verts[0]],12); std::memcpy(tri.b,&t->verts[3*p.verts[k-1]],12); std::memcpy(tri.c,&t->verts[3*p.verts[k]],12); tri.area=p.getArea(); }
        }
    } return count;
}
extern "C" int an_crowd_step(const void* nav,AnAgent* agents,int count,float dt) {
    try {
        std::unique_ptr<dtCrowd,decltype(&dtFreeCrowd)> crowd(dtAllocCrowd(),dtFreeCrowd);
        float radius=0.01f; for(int i=0;i<count;++i) radius=std::max(radius,agents[i].radius);
        if(!crowd || !crowd->init(count,radius,const_cast<dtNavMesh*>(static_cast<const dtNavMesh*>(nav)))) return 1;
        std::vector<int> indices(count);
        // Filters are per-agent (maximum 16 distinct include/exclude combinations).
        std::vector<std::pair<unsigned short,unsigned short>> masks;
        for(int i=0;i<count;++i) {
            auto& a=agents[i]; auto mask=std::make_pair(a.include,a.exclude);
            auto it=std::find(masks.begin(),masks.end(),mask); int fi=static_cast<int>(it-masks.begin());
            if(it==masks.end()) { if(masks.size()==16) return 2; masks.push_back(mask); auto* f=crowd->getEditableFilter(fi); f->setIncludeFlags(a.include); f->setExcludeFlags(a.exclude); }
            dtCrowdAgentParams p{}; p.radius=a.radius; p.height=a.height; p.maxSpeed=a.speed; p.maxAcceleration=a.acceleration;
            p.collisionQueryRange=a.radius*12; p.pathOptimizationRange=a.radius*30; p.separationWeight=2;
            p.updateFlags=DT_CROWD_OBSTACLE_AVOIDANCE|DT_CROWD_SEPARATION; p.queryFilterType=static_cast<unsigned char>(fi);
            int idx=crowd->addAgent(a.position,&p); if(idx<0) return 3; indices[i]=idx;
            auto* agent=crowd->getEditableAgent(idx); if(agent->state==DT_CROWDAGENT_STATE_INVALID) return 4;
            dtVcopy(agent->vel,a.velocity); crowd->requestMoveVelocity(idx,a.desired);
        }
        crowd->update(dt,nullptr);
        for(int i=0;i<count;++i) { const auto* a=crowd->getAgent(indices[i]); dtVcopy(agents[i].position,a->npos); dtVcopy(agents[i].velocity,a->vel); }
        return 0;
    } catch(...) { return 5; }
}
namespace {
// True when edge `k` of ground polygon `p` has a link (internal neighbour or a
// stitched neighbouring tile). Unlinked edges bound the walkable surface.
bool linked(const dtMeshTile* t, const dtPoly& p, int k) {
    if (!(p.neis[k] & DT_EXT_LINK)) return p.neis[k] != 0;
    for (unsigned int l = p.firstLink; l != DT_NULL_LINK; l = t->links[l].next)
        if (t->links[l].edge == k) return true;
    return false;
}
}
// Boundary edges of every loaded tile, in tile slot then polygon order. Returns the
// total; writes at most cap entries.
extern "C" int an_walls(const void* nav, AnWall* out, int cap) {
    auto* mesh = static_cast<const dtNavMesh*>(nav); int count = 0;
    for (int i = 0; i < mesh->getMaxTiles(); ++i) {
        const dtMeshTile* t = mesh->getTile(i); if (!t->header) continue;
        for (int j = 0; j < t->header->offMeshBase; ++j) {
            const dtPoly& p = t->polys[j];
            for (int k = 0; k < p.vertCount; ++k) {
                if (linked(t, p, k)) continue;
                if (count < cap) {
                    std::memcpy(out[count].a, &t->verts[3 * p.verts[k]], 12);
                    std::memcpy(out[count].b, &t->verts[3 * p.verts[(k + 1) % p.vertCount]], 12);
                }
                ++count;
            }
        }
    }
    return count;
}
// Polygon edges lying on tile borders (Detour portal edges), linked or not.
extern "C" int an_borders(const void* nav, AnBorder* out, int cap) {
    auto* mesh = static_cast<const dtNavMesh*>(nav); int count = 0;
    for (int i = 0; i < mesh->getMaxTiles(); ++i) {
        const dtMeshTile* t = mesh->getTile(i); if (!t->header) continue;
        for (int j = 0; j < t->header->offMeshBase; ++j) {
            const dtPoly& p = t->polys[j];
            for (int k = 0; k < p.vertCount; ++k) {
                if (!(p.neis[k] & DT_EXT_LINK)) continue;
                if (count < cap) {
                    auto& b = out[count];
                    std::memcpy(b.a, &t->verts[3 * p.verts[k]], 12);
                    std::memcpy(b.b, &t->verts[3 * p.verts[(k + 1) % p.vertCount]], 12);
                    b.tile_x = t->header->x; b.tile_z = t->header->y;
                    b.side = static_cast<unsigned char>(p.neis[k] & 0xff);
                    b.linked = linked(t, p, k) ? 1 : 0; b.area = p.getArea();
                }
                ++count;
            }
        }
    }
    return count;
}
