// Generated hit.slang concatenates this prefix, the authoritative helper, and the probe.
struct Root {uint scene; uint table; uint output; uint d; uint e; uint f; uint g; uint h;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(1) RWByteAddressBuffer buffers[] : register(u0,space2);
BIND(4) RaytracingAccelerationStructure scenes[16] : register(t0,space4);

// Retained position fetch, shared unchanged by Slang and the native MSL adapter.
#ifndef ARA_HIT_GEOMETRY
#define ARA_HIT_GEOMETRY
#ifndef ARA_HIT_CONTEXT
#define ARA_HIT_CONTEXT
#endif
#ifndef ARA_HIT_LOAD
#define ARA_HIT_LOAD(slot, offset) buffers[NonUniformResourceIndex(slot)].Load(offset)
#endif
#ifndef ARA_HIT_FLOAT3
#define ARA_HIT_FLOAT3(a, b, c) asfloat(uint3(a, b, c))
#endif
struct AraHitTriangle { float3 a; float3 b; float3 c; uint valid; };
// `instance` is the query's instance ordinal, `primitive` its local triangle index.
// Table offsets and all mesh offsets are bytes. Query status must be a triangle hit.
AraHitTriangle araHitTriangle(ARA_HIT_CONTEXT uint table, uint tableOffset,
                             uint instanceCount, uint instance, uint primitive) {
    AraHitTriangle result;
    result.a = float3(0); result.b = float3(0); result.c = float3(0); result.valid = 0;
    if (instance >= instanceCount) return result;
    uint base = tableOffset + instance * 32;
    uint vb = ARA_HIT_LOAD(table, base);
    uint ib = ARA_HIT_LOAD(table, base + 4);
    uint vo = ARA_HIT_LOAD(table, base + 8);
    uint io = ARA_HIT_LOAD(table, base + 12);
    uint stride = ARA_HIT_LOAD(table, base + 16);
    uint vertices = ARA_HIT_LOAD(table, base + 20);
    uint indices = ARA_HIT_LOAD(table, base + 24);
    if (primitive >= indices / 3) return result;
    uint at = io + primitive * 12;
    uint a = ARA_HIT_LOAD(ib, at), b = ARA_HIT_LOAD(ib, at + 4), c = ARA_HIT_LOAD(ib, at + 8);
    if (a >= vertices || b >= vertices || c >= vertices) return result;
    a = vo + a * stride; b = vo + b * stride; c = vo + c * stride;
    result.a = ARA_HIT_FLOAT3(ARA_HIT_LOAD(vb,a), ARA_HIT_LOAD(vb,a+4), ARA_HIT_LOAD(vb,a+8));
    result.b = ARA_HIT_FLOAT3(ARA_HIT_LOAD(vb,b), ARA_HIT_LOAD(vb,b+4), ARA_HIT_LOAD(vb,b+8));
    result.c = ARA_HIT_FLOAT3(ARA_HIT_LOAD(vb,c), ARA_HIT_LOAD(vb,c+4), ARA_HIT_LOAD(vb,c+8));
    result.valid = 1;
    return result;
}
#endif

[shader("compute")][numthreads(1,1,1)]
void hitMain() {
    RayDesc ray; ray.Origin=float3(1.5,0,-2); ray.Direction=float3(0,0,1); ray.TMin=0.001; ray.TMax=100;
    RayQuery<RAY_FLAG_FORCE_OPAQUE> query;
    query.TraceRayInline(scenes[root.scene], RAY_FLAG_NONE,255,ray);
    while(query.Proceed()) {}
    if(query.CommittedStatus()!=COMMITTED_TRIANGLE_HIT) {buffers[root.output].Store(0,0xffffffff);return;}
    uint instance=query.CommittedInstanceIndex(), primitive=query.CommittedPrimitiveIndex();
    AraHitTriangle tri=araHitTriangle(root.table,0,2,instance,primitive);
    // Keep this flat word contract explicit: Slang's Vulkan Store3 lowering uses
    // a std430 uint3 array (16-byte stride), rather than the packed 12-byte rows.
    buffers[root.output].Store(0,instance);
    buffers[root.output].Store(4,primitive);
    buffers[root.output].Store(8,tri.valid);
    for(uint i=0;i<3;i++) {
        buffers[root.output].Store(12+i*4,asuint(tri.a[i]));
        buffers[root.output].Store(24+i*4,asuint(tri.b[i]));
        buffers[root.output].Store(36+i*4,asuint(tri.c[i]));
    }
    buffers[root.output].Store(48,araHitTriangle(root.table,0,2,2,0).valid);
    buffers[root.output].Store(52,araHitTriangle(root.table,0,2,1,2).valid);
}
