struct Root { uint depth; uint b; uint c; uint d; uint e; uint f; uint g; uint h; };
#ifdef VULKAN
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
ConstantBuffer<Root> root : register(b0,space0);
#endif
struct V { float2 p : POSITION; float4 c : COLOR; };
struct O { float4 p : SV_Position; float4 c : COLOR; };
[shader("vertex")] O streams(V v) { O o; o.p=float4(v.p,0.4,1); o.c=v.c; return o; }
[shader("vertex")] O fullscreen(uint id : SV_VertexID) { O o; float2 p=float2((id<<1)&2,id&2)*2-1; o.p=float4(p,asfloat(root.depth)+p.x*asfloat(root.b),1); o.c=float4(1,0,0,1); return o; }
[shader("fragment")] float4 color(O v) : SV_Target { return v.c; }
