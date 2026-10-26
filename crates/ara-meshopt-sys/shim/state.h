// Ara's owned context; upstream native source/header remain unchanged in vendor/.
#include <vector>
#include <memory>
#include <stdint.h>
namespace ara_lod {
using namespace ara_lod_meshopt;
struct Snapshot {
    void* destination;
    std::vector<unsigned char> bytes;
    Snapshot(void* p, size_t n): destination(p), bytes(static_cast<unsigned char*>(p),static_cast<unsigned char*>(p)+n) {}
    void restore() const { memcpy(destination,bytes.data(),bytes.size()); }
};
struct State {
    ara_lod_allocator allocator;
    EdgeAdjacency adjacency={};
    unsigned int *sparse_remap=NULL,*remap=NULL,*wedge=NULL,*loop=NULL,*loopback=NULL;
    unsigned char *vertex_kind=NULL,*collapse_locked=NULL;
    Vector3* vertex_positions=NULL;
    float vertex_offset[3]={},vertex_scale=0,*vertex_attributes=NULL;
    unsigned int attribute_remap[kMaxAttributes]={};
    Quadric *vertex_quadrics=NULL,*attribute_quadrics=NULL;
    QuadricGrad *attribute_gradients=NULL,*volume_gradients=NULL;
    unsigned int *components=NULL,*collapse_order=NULL,*collapse_remap=NULL;
    float *component_errors=NULL,component_nexterror=0;
    size_t component_count=0,collapse_capacity=0,first_count=0,used_attributes=0;
    Collapse* edge_collapses=NULL;
    std::vector<Snapshot> snapshots;
    std::vector<unsigned int> original;
    size_t vertices=0,attributes=0;
    bool dirty=false;
    void save(void* p,size_t n) { if(n) snapshots.push_back(Snapshot(p,n)); }
    void restore() { for(size_t i=0;i<snapshots.size();++i) snapshots[i].restore(); }
};
}
