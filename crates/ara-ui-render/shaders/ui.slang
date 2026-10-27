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
// Mode 9 fills (ara_ui::paint::Fill): sRGB transfer, full-saturation hue.
float3 srgbToLinear(float3 c){return select(c<=0.04045,c/12.92,pow((c+0.055)/1.055,2.4));}
float3 linearToSrgb(float3 c){return select(c<=0.0031308,c*12.92,1.055*pow(max(c,0),1.0/2.4)-0.055);}
float3 hueColor(float h){return saturate(abs(fmod(h*6+float3(0,4,2),6)-3)-1);}
// Linear gradient (kind 0; flags 1 vertical, 2 linear light), hue strip (1), hue ring
// (2: annulus of thickness z, 0 deg red at +x, counterclockwise) or checkerboard (3).
float4 fillColor(Instance i,float2 p,inout float shape){
    uint kind=uint(i.parameters.y+0.5);float2 size=i.rect.zw;
    if(kind==0){
        uint flags=uint(i.parameters.w+0.5);
        float t=saturate((flags&1)!=0?p.y/max(size.y,0.0001):p.x/max(size.x,0.0001));
        float4 c=lerp(i.color,i.uv,t);
        if((flags&2)!=0)c.rgb=linearToSrgb(lerp(srgbToLinear(i.color.rgb),srgbToLinear(i.uv.rgb),t));
        return c;
    }
    if(kind==1)return float4(hueColor(saturate(p.x/max(size.x,0.0001))),i.color.a);
    if(kind==2){
        float2 d=p-size*0.5;float r=length(d);float outer=min(size.x,size.y)*0.5;float inner=outer-i.parameters.z;
        float aa=max(fwidth(r),0.0001);
        shape=saturate((outer-r)/aa+0.5)*saturate((r-inner)/aa+0.5);
        float h=frac(atan2(-d.y,d.x)/6.28318530718+1);
        return float4(hueColor(h),i.color.a);
    }
    float cell=max(i.parameters.z,1);int2 q=int2(floor(p/cell));
    return ((q.x+q.y)&1)==0?i.color:i.uv;
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
    } else if(pc.mode==9) {
        float d=roundedDistance(input.local,i.rect.zw,i.parameters.x);
        float shape=saturate(0.5-d/max(fwidth(d),0.0001));
        color=fillColor(i,input.local,shape); color.a*=shape;
    } else if(pc.mode==6) {
        float2 a=i.uv.xy,b=i.uv.zw,d=b-a;
        float t=saturate(dot(input.local-a,d)/max(dot(d,d),0.0001));
        color.a*=saturate(i.parameters.x-length(input.local-a-t*d)+0.5);
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
