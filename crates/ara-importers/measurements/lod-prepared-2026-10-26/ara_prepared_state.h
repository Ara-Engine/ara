// Isolated experiment only. No engine build includes this file.
#define NOMINMAX
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdint.h>
#include <vector>
#include <stdio.h>
#include <stdlib.h>
namespace ara_prepared {
using namespace meshopt;
struct Stamp {
    ULONG64 cycles; LARGE_INTEGER wall;
    Stamp() { if (!QueryThreadCycleTime(GetCurrentThread(), &cycles) || !QueryPerformanceCounter(&wall)) abort(); }
};
struct Time { uint64_t cycles = 0; int64_t ticks = 0; void add(const Stamp& start) { Stamp end; cycles += end.cycles-start.cycles; ticks += end.wall.QuadPart-start.wall.QuadPart; } };
struct Totals {
    Time attributes, positions, prepare, reset, destroy;
    uint64_t scopes=0, builds=0, reuses=0, attr_calls=0, position_calls=0, bytes_copied=0, peak_snapshot_bytes=0;
};
static Totals totals;
struct Snapshot {
    void* destination;
    std::vector<unsigned char> bytes;
    Snapshot(void* p, size_t n): destination(p), bytes(static_cast<unsigned char*>(p),static_cast<unsigned char*>(p)+n) {}
    void restore() const { memcpy(destination,bytes.data(),bytes.size()); }
};
struct State {
    meshopt_Allocator allocator;
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
    const unsigned int* input=NULL;
    const float *input_positions=NULL,*input_attributes=NULL,*input_weights=NULL;
    const unsigned char* input_locks=NULL;
    size_t input_indices=0,input_vertices=0,input_stride=0,input_attribute_stride=0,input_attribute_count=0;
    unsigned input_options=0;
    void save(void* p,size_t n) { if(n) snapshots.push_back(Snapshot(p,n)); }
    void restore() { for(size_t i=0;i<snapshots.size();++i) { snapshots[i].restore(); totals.bytes_copied+=snapshots[i].bytes.size(); } }
};
static thread_local State* state=NULL;
static thread_local bool active=false;
static bool enabled() {
    static bool value=[](){char data[8]={};return GetEnvironmentVariableA("ARA_PREPARED_LOD",data,8)>0 && data[0]=='1';}();
    return value;
}
}
extern "C" void ara_prepared_begin() {
    if(ara_prepared::active || ara_prepared::state) abort();
    ara_prepared::active=true;
    ++ara_prepared::totals.scopes;
}
extern "C" void ara_prepared_end() {
    using namespace ara_prepared;
    if(!active) abort();
    if(state) { Stamp began; delete state; state=NULL; totals.destroy.add(began); }
    active=false;
}
extern "C" void ara_prepared_dump() {
    using namespace ara_prepared;
    if(active || state) abort();
    LARGE_INTEGER frequency; if(!QueryPerformanceFrequency(&frequency)) abort();
    const Time times[]={totals.attributes,totals.positions,totals.prepare,totals.reset,totals.destroy};
    const char* labels[]={"attributes","positions","prepare","reset","destroy"};
    fprintf(stderr,"ARA_PREPARED_COUNTS,%u,%llu,%llu,%llu,%llu,%llu,%llu,%llu\n",enabled()?1:0,totals.scopes,totals.builds,totals.reuses,totals.attr_calls,totals.position_calls,totals.bytes_copied,totals.peak_snapshot_bytes);
    for(size_t i=0;i<5;++i) fprintf(stderr,"ARA_PREPARED_TIME,%s,%llu,%.9f\n",labels[i],times[i].cycles,double(times[i].ticks)*1000.0/double(frequency.QuadPart));
}
