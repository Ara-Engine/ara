struct Root { uint source; uint destination; uint c; uint d; uint e; uint f; uint g; uint h; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(5) Texture2DArray<uint4> images[] : register(t0,space1);
BIND(3) RWTexture2DArray<uint4> targets[128] : register(u0,space3);
[shader("compute")][numthreads(1,1,1)]
void viewsMain(uint3 id : SV_DispatchThreadID) {
    targets[root.destination][id] = images[root.source].Load(int4(id,0));
}
