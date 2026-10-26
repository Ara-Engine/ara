// Private symbol namespace; both vendored files are byte-for-byte upstream.
#define MESHOPTIMIZER_NO_WRAPPERS
#define meshopt ara_lod_meshopt
#define meshopt_Allocator ara_lod_allocator
#define meshopt_simplifyEdge ara_lod_upstream_meshopt_simplifyEdge
#define meshopt_simplify ara_lod_upstream_meshopt_simplify
#define meshopt_simplifyWithAttributes ara_lod_upstream_meshopt_simplifyWithAttributes
#define meshopt_simplifyWithUpdate ara_lod_upstream_meshopt_simplifyWithUpdate
#define meshopt_simplifySloppy ara_lod_upstream_meshopt_simplifySloppy
#define meshopt_simplifyPrune ara_lod_upstream_meshopt_simplifyPrune
#define meshopt_simplifyPoints ara_lod_upstream_meshopt_simplifyPoints
#define meshopt_simplifyScale ara_lod_upstream_meshopt_simplifyScale
#include "../vendor/simplifier.cpp"
#undef meshopt
#undef meshopt_Allocator
#undef meshopt_simplifyEdge
#undef meshopt_simplify
#undef meshopt_simplifyWithAttributes
#undef meshopt_simplifyWithUpdate
#undef meshopt_simplifySloppy
#undef meshopt_simplifyPrune
#undef meshopt_simplifyPoints
#undef meshopt_simplifyScale
#include "state.h"
#include "prepare.inc"
#include "run.inc"

static_assert(sizeof(unsigned int)==4, "Rust u32 ABI requires 32-bit unsigned int");

// Only these three owned-context calls cross the Rust ABI. No input pointer
// survives construction; no exception crosses the boundary.
extern "C" void* ara_lod_begin(const unsigned int* indices, size_t index_count,
    const float* positions, size_t vertex_count, const float* attributes,
    const float* weights, size_t attribute_count, const unsigned char* locks)
{
    try {
        std::unique_ptr<ara_lod::State> p(new ara_lod::State);
        p->original.assign(indices, indices+index_count);
        p->vertices=vertex_count;
        p->attributes=attribute_count;
        ara_lod::prepare(*p,p->original.data(),p->original.data(),index_count,
            positions,vertex_count,12,attributes,attribute_count*sizeof(float),
            weights,attribute_count,locks,0,0,97,NULL);
        return p.release();
    } catch (...) { return NULL; }
}
extern "C" size_t ara_lod_reset(void* context, unsigned int* destination,
    size_t target_count, float error, float* result_error)
{
    try {
        ara_lod::State& p=*static_cast<ara_lod::State*>(context);
        if(target_count>p.original.size()) return size_t(-1);
        if(p.dirty) p.restore();
        p.dirty=true;
        memcpy(destination,p.original.data(),p.original.size()*sizeof(unsigned int));
        return ara_lod::run(p,destination,p.original.data(),p.original.size(),NULL,
            p.vertices,12,NULL,p.attributes*sizeof(float),NULL,p.attributes,NULL,
            target_count,error,97,result_error);
    } catch (...) { return size_t(-1); }
}
extern "C" void ara_lod_end(void* context) noexcept
{
    delete static_cast<ara_lod::State*>(context);
}
