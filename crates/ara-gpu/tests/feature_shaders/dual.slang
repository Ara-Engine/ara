struct O { float4 p : SV_Position; };
[shader("vertex")] O fullscreenVertex(uint id : SV_VertexID) {
    O o; o.p=float4(float2((id<<1)&2,id&2)*2-1,0.4,1); return o;
}
struct C { float4 a : SV_Target0; float4 b : SV_Target1; };
[shader("fragment")] float4 colorOnly() : SV_Target { return float4(1,0,0,1); }
[shader("fragment")] float depthOnly() : SV_Depth { return 0.5; }
[shader("fragment")] C dualColor() {
    C c; c.a=float4(1,0,0,1); c.b=c.a; return c;
}
