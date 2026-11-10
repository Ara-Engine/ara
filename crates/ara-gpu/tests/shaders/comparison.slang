struct Root { uint image; uint output; uint samplerIndex; uint a; uint b; uint c; uint d; uint e; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(5) Texture2D<float> textures[] : register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[] : register(u0,space2);
BIND(6) SamplerComparisonState comparisons[64] : register(s0,space5);
[shader("compute")][numthreads(1,1,1)]
void comparisonMain() {
    float result = textures[root.image].SampleCmpLevelZero(comparisons[root.samplerIndex], float2(0.5), 0.25);
    buffers[root.output].Store(0, uint(round(result)));
}

