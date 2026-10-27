// Metal backend `Command::ClearDepth` (Metal has no in-pass rectangle clear; Vulkan
// uses vkCmdClearAttachments and never loads this pipeline). A vertex-only
// fullscreen triangle at the clear depth; the backend clips it to the tile with the
// viewport and scissor, tests Always and writes depth. Root word 0 holds the depth.
struct Root { float depth; uint pad0; uint pad1; uint pad2; uint pad3; uint pad4; uint pad5; uint pad6; };
#ifdef VULKAN
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
ConstantBuffer<Root> root : register(b0, space0);
#endif

[shader("vertex")]
float4 clearDepthVertex(uint id : SV_VertexID) : SV_Position
{
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), root.depth, 1);
}
