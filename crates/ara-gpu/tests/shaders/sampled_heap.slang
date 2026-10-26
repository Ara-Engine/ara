// Offline probe: nonuniform sampled indices above 127 and native mip filtering.
struct Root { uint indices; uint output; uint count; uint lod; uint a; uint b; uint c; uint d; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(5) Texture2D<float4> textures[] : register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[] : register(u0,space2);
BIND(2) SamplerState samplers[3] : register(s0,space0);
[shader("compute")][numthreads(64,1,1)]
void sampleMain(uint3 id : SV_DispatchThreadID) {
    if(id.x >= root.count) return;
    uint slot = buffers[NonUniformResourceIndex(root.indices)].Load((root.a+id.x)*4);
    float4 color = textures[NonUniformResourceIndex(slot)].SampleLevel(samplers[0], float2(0.5), asfloat(root.lod));
    uint4 bytes = uint4(round(saturate(color)*255));
    buffers[NonUniformResourceIndex(root.output)].Store(id.x*4, bytes.x | (bytes.y<<8) | (bytes.z<<16) | (bytes.w<<24));
}
