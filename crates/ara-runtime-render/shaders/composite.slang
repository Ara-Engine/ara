// Merge native landscape outputs with the scene before transparent particles.
struct Root { uint scene; uint depth; uint landscape; uint landscapeDepth; uint hdr; uint mergedDepth; uint exposure; uint exposureState; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(5) Texture2D<float4> textures[] : register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[] : register(u0,space2);
BIND(3) RWTexture2D<float4> images[64] : register(u0,space3);
// Landscape publishes gamma-2.2 display color. Undo its ACES fitted curve and its
// manual exposure, then pre-expose like the scene HDR (ara_render::HdrExposure:
// the exposure state's applied multiplier, plus one in `exposureState`; without
// one, or while it is unset, the manual exposure).
float3 landscapeHdr(float3 c) {
    float3 y = min(pow(saturate(c), 2.2), 0.999);
    float3 a = 2.51 - 2.43 * y;
    float3 b = 0.03 - 0.59 * y;
    return (-b + sqrt(b*b + 0.56*a*y)) / (2*a);
}
float preExposure() {
    float manual = max(asfloat(root.exposure), 0.0000001);
    if (root.exposureState == 0) return manual;
    float x = buffers[NonUniformResourceIndex(root.exposureState - 1)].Load<float4>(0).x;
    return x > 0 && x < 1e30 ? x : manual;
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
        color = float4(min(landscapeHdr(textures[NonUniformResourceIndex(root.landscape)].Load(p).rgb) / max(asfloat(root.exposure), 0.0000001) * preExposure(), 60000), 1);
        depth = landscapeDepth;
    }
    images[NonUniformResourceIndex(root.hdr)][id.xy] = color;
    images[NonUniformResourceIndex(root.mergedDepth)][id.xy] = float4(depth, 0, 0, 0);
}
