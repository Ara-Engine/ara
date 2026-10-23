// ara-ui-render's 64-byte instance ABI. Rounded ancestors intersect analytic masks.
struct Instance { float4 rect; float4 uv; float4 color; float4 parameters; };
[[vk::binding(0,0)]] StructuredBuffer<Instance> instances;
[[vk::binding(1,0)]] Texture2D<float4> textures[128];
[[vk::binding(2,0)]] SamplerState linearClamp;
// Two float4 records per rounded ancestor: rect and radius. Rect clips also use scissor.
[[vk::binding(3,0)]] StructuredBuffer<float4> masks;
struct Push { uint firstInstance; uint textureIndex; uint mode; uint maskOffset; float2 viewport; uint maskCount; float dpi; };
[[vk::push_constant]] ConstantBuffer<Push> pc;
struct Varying { float4 position:SV_Position; float2 local:TEXCOORD0; float2 uv:TEXCOORD1; nointerpolation uint instanceId:TEXCOORD2; };
float roundedDistance(float2 p,float2 size,float radius) {
    radius=clamp(radius,0,min(size.x,size.y)*0.5);float2 q=abs(p-size*0.5)-size*0.5+radius;
    return length(max(q,0))+min(max(q.x,q.y),0)-radius;
}
[shader("vertex")]
Varying vertexMain(uint vertex:SV_VertexID,uint instance:SV_InstanceID) {
    static const float2 corners[6]={float2(0,0),float2(1,0),float2(0,1),float2(0,1),float2(1,0),float2(1,1)};
    Instance i=instances[pc.firstInstance+instance];float2 local=corners[vertex];float2 p=i.rect.xy+local*i.rect.zw;
    Varying o;o.position=float4(p/pc.viewport*float2(2,2)-1,0,1);o.local=local*i.rect.zw;o.uv=i.uv.xy+local*i.uv.zw;o.instanceId=pc.firstInstance+instance;return o;
}
[shader("fragment")]
float4 fragmentMain(Varying input):SV_Target {
    float coverage=1;
    for(uint n=0;n<pc.maskCount;n++) {
        float4 rect=masks[pc.maskOffset+2*n];float radius=masks[pc.maskOffset+2*n+1].x;
        float d=roundedDistance(input.position.xy/pc.dpi-rect.xy,rect.zw,radius);
        coverage=min(coverage,saturate(0.5-d/max(fwidth(d),0.0001)));
    }
    Instance i=instances[input.instanceId];float4 color=i.color;
    if(pc.mode==0) {
        float d=roundedDistance(input.local,i.rect.zw,i.parameters.x);float aa=max(fwidth(d),0.0001);
        float shape=saturate(0.5-d/aa);
        if(i.parameters.y>0) {float b=i.parameters.y;float2 inner=i.rect.zw-2*b;float hole=0;if(all(inner>0))hole=saturate(0.5-roundedDistance(input.local-b,inner,max(i.parameters.x-b,0))/aa);shape-=hole;}
        color.a*=shape;
    }else if(pc.mode==5){
        uint width,height;textures[NonUniformResourceIndex(pc.textureIndex)].GetDimensions(width,height);
        int2 pixel=clamp(int2(input.uv*float2(width,height)),int2(0,0),int2(width-1,height-1));
        uint wanted=uint(i.parameters.z);uint centre=uint(textures[NonUniformResourceIndex(pc.textureIndex)].Load(int3(pixel,0)).r);
        bool edge=false;
        static const int2 offsets[4]={int2(-1,0),int2(1,0),int2(0,-1),int2(0,1)};
        for(uint n=0;n<4;n++){int2 other=clamp(pixel+offsets[n]*2,int2(0,0),int2(width-1,height-1));uint neighbor=uint(textures[NonUniformResourceIndex(pc.textureIndex)].Load(int3(other,0)).r);edge=edge||((centre==wanted)!=(neighbor==wanted));}
        color.a*=edge?1:0;
    }else if(pc.mode==6){
        float2 a=i.uv.xy,b=i.uv.zw;float2 d=b-a;float t=saturate(dot(input.local-a,d)/max(dot(d,d),0.0001));float distance=length(input.local-a-t*d);
        color.a*=saturate((i.parameters.x-distance)*pc.dpi+0.5);
    }else{
        float4 sample=textures[NonUniformResourceIndex(pc.textureIndex)].Sample(linearClamp,input.uv);
        if(pc.mode==2)color.a*=sample.a;
        else if(pc.mode==3){float median=max(min(sample.r,sample.g),min(max(sample.r,sample.g),sample.b));uint width,height;textures[NonUniformResourceIndex(pc.textureIndex)].GetDimensions(width,height);float2 unitRange=2*i.parameters.z/float2(width,height);float screenRange=max(0.5*dot(unitRange,1/fwidth(input.uv)),1);color.a*=saturate((median-0.5)*screenRange+0.5);}
        else color*=sample;
    }
    color.a*=coverage;return float4(color.rgb*color.a,color.a);
}
