// The editor UI on the engine's bindless ABI, cooked like every engine pipeline
// (pipelines.json), so the Metal backend draws it through Metal Shader Converter's
// global root signature. The logic is ui_body.slang, shared with the Vulkan editor
// layout (ui.slang). Clip space follows the cook's convention (y up; its SPIR-V lane
// inverts y), mapped into the draw's viewport, which is the draw's clip rect.
struct Instance { float4 rect; float4 uv; float4 color; float4 parameters; };
// Eight root words: the record buffers' slots, then the draw.
struct Root {
    uint instances;   // buffers[] slot of the 64-byte instance records
    uint masks;       // buffers[] slot of the clip-mask records (two float4 each)
    uint firstInstance;
    uint textureMode; // textures[] slot | shader mode << 16
    uint maskRange;   // first mask record | record count << 24
    uint clipOrigin;  // physical x | y << 16
    uint clipSize;    // physical width | height << 16
    float dpi;
};
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

// The fields ui_body.slang reads as `pc`, unpacked from the root words.
struct Push { uint firstInstance; uint textureIndex; uint mode; uint maskOffset; uint maskCount; float dpi; float2 clipOrigin; float2 clipSize; };
Push push() {
    Push p;
    p.firstInstance = root.firstInstance;
    p.textureIndex = root.textureMode & 0xffff;
    p.mode = root.textureMode >> 16;
    p.maskOffset = root.maskRange & 0xffffff;
    p.maskCount = root.maskRange >> 24;
    p.dpi = root.dpi;
    p.clipOrigin = float2(root.clipOrigin & 0xffff, root.clipOrigin >> 16);
    p.clipSize = float2(root.clipSize & 0xffff, root.clipSize >> 16);
    return p;
}
Instance loadInstance(uint index) {
    return buffers[NonUniformResourceIndex(root.instances)].Load<Instance>(index * 64);
}
#define pc push()
#define INSTANCE(i) loadInstance(i)
#define MASK(i) asfloat(buffers[NonUniformResourceIndex(root.masks)].Load4((i) * 16))
#define TEX(slot) textures[NonUniformResourceIndex(slot)]
// The engine's third sampler: linear, clamped to the edge.
#define SAMPLER samplers[2]
#define NDC(p) (((p) * pc.dpi - pc.clipOrigin) / pc.clipSize * float2(2, -2) + float2(-1, 1))
#include "ui_body.slang"
