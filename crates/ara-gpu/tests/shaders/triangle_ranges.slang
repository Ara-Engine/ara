struct Root { uint scene; uint output; uint a; uint b; uint c; uint d; uint e; uint f; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(1) RWByteAddressBuffer buffers[64] : register(u0,space2);
BIND(4) RaytracingAccelerationStructure acceleration[16] : register(t0,space4);
[shader("compute")][numthreads(4,1,1)]
void rangeProbe(uint3 id : SV_DispatchThreadID) {
    if(id.x >= 3) return;
    RayDesc ray; ray.Origin = float3(id.x*2.0, 0.0, -2.0);
    ray.Direction = float3(0.0, 0.0, 1.0); ray.TMin = 0.001; ray.TMax = 10.0;
    RayQuery<RAY_FLAG_FORCE_OPAQUE> query;
    query.TraceRayInline(acceleration[root.scene], RAY_FLAG_NONE, 255, ray);
    while(query.Proceed()) {}
    buffers[root.output].Store(id.x*4, query.CommittedStatus() == COMMITTED_TRIANGLE_HIT
        ? query.CommittedInstanceID() : 0xffffffff);
}
