struct Root { uint image; uint output; uint samplerIndex; uint u; uint e; uint f; uint g; uint h; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(5) Texture2D<float4> images[] : register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[] : register(u0,space2);
BIND(2) SamplerState samplers[64] : register(s0,space0);
[shader("compute")][numthreads(1,1,1)]
void samplerMain() {
    float4 color = images[root.image].SampleLevel(samplers[root.samplerIndex], float2(asfloat(root.u),0.5),0);
    buffers[root.output].Store4(0,uint4(round(saturate(color)*255)));
}
