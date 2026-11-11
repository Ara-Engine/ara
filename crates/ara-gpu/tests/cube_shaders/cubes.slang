// Samples face `id.z % 6` of cube `id.z / 6` of a cube-array view by direction and
// stores it in layer `id.z` of a 2D-array storage view (tests/cube_arrays.rs).
struct Root { uint cubes; uint destination; uint samplerIndex; uint d; uint e; uint f; uint g; uint h; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(5) TextureCubeArray<float4> images[] : register(t0,space1);
BIND(3) RWTexture2DArray<float4> targets[128] : register(u0,space3);
BIND(2) SamplerState samplers[64] : register(s0,space0);
static const float3 toward[6] = {
    float3(1,0,0), float3(-1,0,0), float3(0,1,0), float3(0,-1,0), float3(0,0,1), float3(0,0,-1)
};
[shader("compute")][numthreads(1,1,1)]
void cubesMain(uint3 id : SV_DispatchThreadID) {
    float4 value = images[root.cubes].SampleLevel(
        samplers[root.samplerIndex], float4(toward[id.z % 6], float(id.z / 6)), 0);
    targets[root.destination][uint3(0, 0, id.z)] = value;
}
