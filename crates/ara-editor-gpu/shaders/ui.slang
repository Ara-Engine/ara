// ara-ui-render's 64-byte instance ABI. Rounded ancestors intersect analytic masks.
struct Instance { float4 rect; float4 uv; float4 color; float4 parameters; };
[[vk::binding(0,0)]] StructuredBuffer<Instance> instances;
[[vk::binding(1,0)]] Texture2D<float4> textures[256];
[[vk::binding(2,0)]] SamplerState linearClamp;
// Two float4 records per rounded ancestor: rect and radius. Rect clips also use scissor.
[[vk::binding(3,0)]] StructuredBuffer<float4> masks;
struct Push { uint firstInstance; uint textureIndex; uint mode; uint maskOffset; float2 viewport; uint maskCount; float dpi; };
[[vk::push_constant]] ConstantBuffer<Push> pc;
#define INSTANCE(i) instances[i]
#define MASK(i) masks[i]
#define TEX(slot) textures[NonUniformResourceIndex(slot)]
#define SAMPLER linearClamp
#define NDC(p) (p/pc.viewport*float2(2,2)-1)
#include "ui_body.slang"
