struct Root { uint table; uint output; uint count; uint lod; uint first; uint normal; uint scale; uint reserved; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(5) Texture2D<float4> textures[] : register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[64] : register(u0,space2);
BIND(2) SamplerState samplers[3] : register(s0,space0);
#include "native_sample.slang"
[shader("compute")][numthreads(64,1,1)]
void nativeProbe(uint3 id : SV_DispatchThreadID) {
    if(id.x >= root.count) return;
    uint textureId = root.first + id.x;
    float4 value = root.normal == 1
        ? float4(ntNormal(root.table, textureId, float2(0.5), asfloat(root.lod), asfloat(root.scale)), 1.0)
        : ntSampleColorLevel(root.table, textureId, float2(0.5), asfloat(root.lod), root.normal == 2);
    buffers[NonUniformResourceIndex(root.output)].Store4(id.x * 16, asuint(value));
}
