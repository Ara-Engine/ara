// Instance ABI: ara_ui_render::gpu::Instance, 64 bytes, 16-byte alignment.
// RGBA8 UNORM UI target, sRGB numeric working space, premultiplied ONE / ONE_MINUS_SRC_ALPHA.
// The host linearizes the completed UI texture before linear/HDR scene composition.
// Rectangular clips use scissor; rounded nested clips use intersected stencil masks.
struct Instance { float4 rect; float4 uv; float4 color; float4 parameters; };
[[vk::binding(0,0)]] StructuredBuffer<Instance> instances;
[[vk::binding(1,0)]] Texture2D<float4> textures[];
[[vk::binding(2,0)]] SamplerState linearClamp;
struct Push { uint firstInstance; uint textureIndex; uint mode; uint reserved; float2 viewport; float2 padding; };
[[vk::push_constant]] ConstantBuffer<Push> pc;
struct Varying { float4 position : SV_Position; float2 local : TEXCOORD0; float2 uv : TEXCOORD1; nointerpolation uint instanceId : TEXCOORD2; };
[shader("vertex")]
Varying vertexMain(uint vertex : SV_VertexID, uint instance : SV_InstanceID) {
    static const float2 corners[6] = {float2(0,0),float2(1,0),float2(0,1),float2(0,1),float2(1,0),float2(1,1)};
    Instance i=instances[pc.firstInstance+instance]; float2 local=corners[vertex];
    float2 p=i.rect.xy+local*i.rect.zw;
    Varying o; o.position=float4(p/pc.viewport*float2(2,-2)+float2(-1,1),0,1);
    o.local=local*i.rect.zw; o.uv=i.uv.xy+local*i.uv.zw; o.instanceId=pc.firstInstance+instance; return o;
}
float roundedDistance(float2 p,float2 size,float radius) {
    radius=clamp(radius,0,min(size.x,size.y)*0.5); float2 q=abs(p-size*0.5)-size*0.5+radius;
    return length(max(q,0))+min(max(q.x,q.y),0)-radius;
}
[shader("fragment")]
float4 fragmentMain(Varying input) : SV_Target {
    Instance i=instances[input.instanceId]; float4 color=i.color;
    if(pc.mode==0) {
        float d=roundedDistance(input.local,i.rect.zw,i.parameters.x);
        float aa=max(fwidth(d),0.0001); float coverage=saturate(0.5-d/aa);
        if(i.parameters.y>0) {
            float b=i.parameters.y; float2 inner=i.rect.zw-2*b;
            float hole=0;
            if(all(inner>0)) hole=saturate(0.5-roundedDistance(input.local-b,inner,max(i.parameters.x-b,0))/aa);
            coverage-=hole;
        }
        color.a*=coverage;
    } else {
        float4 sample=textures[NonUniformResourceIndex(pc.textureIndex)].Sample(linearClamp,input.uv);
        if(pc.mode==2) color.a*=sample.a;
        else if(pc.mode==3) {
            float median=max(min(sample.r,sample.g),min(max(sample.r,sample.g),sample.b));
            uint width,height; textures[NonUniformResourceIndex(pc.textureIndex)].GetDimensions(width,height);
            float2 unitRange=2*i.parameters.z/float2(width,height);
            float screenRange=max(0.5*dot(unitRange,1/fwidth(input.uv)),1);
            color.a*=saturate((median-0.5)*screenRange+0.5);
        } else color*=sample;
    }
    return float4(color.rgb*color.a,color.a);
}
