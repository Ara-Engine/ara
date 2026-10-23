// Merge native landscape outputs with the scene before transparent particles.
struct Root { uint scene; uint depth; uint landscape; uint landscapeDepth; uint hdr; uint mergedDepth; uint exposure; uint unused; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(0) Texture2D<float4> textures[128] : register(t0,space1);
BIND(3) RWTexture2D<float4> images[64] : register(u0,space3);
// Landscape publishes gamma-2.2 display color. Undo its ACES fitted curve so the
// shared exposure/post pass and transparent blending operate in linear HDR.
// The landscape's display encoding already includes exposure; undo it here too.
float3 landscapeHdr(float3 c) {
    float3 y = min(pow(saturate(c), 2.2), 0.999);
    float3 a = 2.51 - 2.43 * y;
    float3 b = 0.03 - 0.59 * y;
    return (-b + sqrt(b*b + 0.56*a*y)) / (2*a);
}
[shader("compute")]
[numthreads(8,8,1)]
void compositeMain(uint3 id : SV_DispatchThreadID) {
    uint width, height;
    textures[NonUniformResourceIndex(root.depth)].GetDimensions(width, height);
    if (id.x >= width || id.y >= height) return;
    int3 p = int3(id.xy, 0);
    float depth = textures[NonUniformResourceIndex(root.depth)].Load(p).r;
    float landscapeDepth = textures[NonUniformResourceIndex(root.landscapeDepth)].Load(p).r;
    float4 color = textures[NonUniformResourceIndex(root.scene)].Load(p);
    if (landscapeDepth > depth) {
        color = float4(landscapeHdr(textures[NonUniformResourceIndex(root.landscape)].Load(p).rgb) / max(asfloat(root.exposure), 0.0000001), 1);
        depth = landscapeDepth;
    }
    images[NonUniformResourceIndex(root.hdr)][id.xy] = color;
    images[NonUniformResourceIndex(root.mergedDepth)][id.xy] = float4(depth, 0, 0, 0);
}
