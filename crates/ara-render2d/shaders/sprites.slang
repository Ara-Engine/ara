// Procedural sprite ABI: 128-byte instances, 64-byte view header, 32-byte lights.
struct Root { uint frame; uint instances; uint first; uint unused0; uint unused1; uint unused2; uint unused3; uint unused4; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(0) Texture2D<float4> textures[128] : register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[64] : register(u0,space2);
BIND(2) SamplerState samplers[3] : register(s0,space0);
struct Frame { float4 view; float4 ambient; uint4 counts; uint4 offsets; };
struct Instance { float4 pose; float4 sizePivot; float4 uv; float4 borders; float4 color; uint4 info; float4 parallaxSource; float4 basis; };
struct Light { float4 positionRadius; float4 colorShadows; };
struct Varying { float4 clip:SV_Position; float2 uv:TEXCOORD0; float2 world:TEXCOORD1; nointerpolation uint instance:TEXCOORD2; };
float2 corner(uint index) { const float2 corners[6]={float2(0,0),float2(1,0),float2(1,1),float2(0,0),float2(1,1),float2(0,1)};return corners[index]; }
[shader("vertex")]
Varying spriteVertex(uint vertex:SV_VertexID,uint instanceId:SV_InstanceID) {
    Frame f=buffers[NonUniformResourceIndex(root.frame)].Load<Frame>(0);
    uint index=root.first+instanceId; Instance s=buffers[NonUniformResourceIndex(root.instances)].Load<Instance>(index*128);
    float2 source=s.parallaxSource.zw;float2 size=float2(s.sizePivot.x>0?s.sizePivot.x:source.x*f.ambient.w,s.sizePivot.y>0?s.sizePivot.y:source.y*f.ambient.w);
    float2 point=corner(vertex%6);float2 local;float2 texPoint;
    if((s.info.w&1)!=0) {
        uint cell=vertex/6;uint2 at=uint2(cell%3,cell/3)+uint2(point);
        float4 border=s.borders*f.ambient.w;float2 fit=min(float2(1),size/max(float2(border.x+border.z,border.y+border.w),0.000001));border*=float4(fit.x,fit.y,fit.x,fit.y);
        float xs[4]={0,border.x,size.x-border.z,size.x};float ys[4]={0,border.y,size.y-border.w,size.y};
        float us[4]={0,s.borders.x/source.x,1-s.borders.z/source.x,1};float vs[4]={0,s.borders.y/source.y,1-s.borders.w/source.y,1};
        local=float2(xs[at.x],ys[at.y]);texPoint=float2(us[at.x],vs[at.y]);
    } else {local=point*size;texPoint=point;}
    local-=s.sizePivot.zw*size;
    // Flips mirror both geometry about the authored pivot and its tangent normals.
    if((s.info.w&4)!=0)local.x=-local.x;if((s.info.w&8)!=0)local.y=-local.y;
    float2 pivot=s.pose.xy+f.view.xy*(1-s.parallaxSource.xy);
    if(f.counts.w!=0)pivot=round(pivot/f.ambient.w)*f.ambient.w;
    float2 world=pivot+float2(dot(s.basis.xy,local),dot(s.basis.zw,local));
    Varying o;o.clip=float4((world-f.view.xy)*f.view.zw,0,1);o.uv=s.uv.xy+float2(texPoint.x,1-texPoint.y)*s.uv.zw;o.world=world;o.instance=index;return o;
}
float4 sampleAtlas(uint index,float2 uv,bool nearest) {
    if(nearest){uint w,h;textures[NonUniformResourceIndex(index)].GetDimensions(w,h);return textures[NonUniformResourceIndex(index)].Load(int3(clamp(int2(uv*float2(w,h)),int2(0),int2(w-1,h-1)),0));}
    return textures[NonUniformResourceIndex(index)].SampleLevel(samplers[2],uv,0);
}
float cross2(float2 a,float2 b){return a.x*b.y-a.y*b.x;}
// Occluders are solid-region outlines wound counter-clockwise (solid on the left). Only
// edges facing the fragment cast shadows: solid tiles are lit through their own surface
// but shadowed by other solids between them and the light.
bool blocked(Frame f,float2 point,float2 light) {
    float2 ray=light-point;
    for(uint i=0;i<f.counts.y;i++) {
        float4 edge=buffers[NonUniformResourceIndex(root.frame)].Load<float4>(f.offsets.y+i*16);
        float2 segment=edge.zw-edge.xy;if(dot(float2(segment.y,-segment.x),ray)>=0)continue;float den=cross2(ray,segment);if(abs(den)<0.000001)continue;
        float2 offset=edge.xy-point;float t=cross2(offset,segment)/den;float u=cross2(offset,ray)/den;
        if(t>0.002&&t<0.998&&u>=0&&u<=1)return true;
    }
    return false;
}
struct Output { float4 color:SV_Target0;float id:SV_Target1; };
[shader("fragment")]
Output spriteFragment(Varying v) {
    Frame f=buffers[NonUniformResourceIndex(root.frame)].Load<Frame>(0);Instance s=buffers[NonUniformResourceIndex(root.instances)].Load<Instance>(v.instance*128);
    float4 color=sampleAtlas(s.info.x,v.uv,f.counts.w!=0)*s.color;if(color.a<0.0039)discard;
    float3 normal=float3(0,0,1);
    if((s.info.w&2)!=0 || f.counts.z==2)normal=sampleAtlas(s.info.y,v.uv,f.counts.w!=0).xyz*2-1;
    if((s.info.w&4)!=0)normal.x=-normal.x;if((s.info.w&8)!=0)normal.y=-normal.y;
    float determinant=s.basis.x*s.basis.w-s.basis.y*s.basis.z;
    float2 n=abs(determinant)>0.000001?float2(s.basis.w*normal.x-s.basis.z*normal.y,-s.basis.y*normal.x+s.basis.x*normal.y)/determinant:normal.xy;
    normal=normalize(float3(n,normal.z));
    float3 illumination=float3(1);float visibility=1;
    if((s.info.w&2)!=0 && (f.counts.z==0 || f.counts.z==6)) {
        illumination=f.ambient.xyz;
        for(uint i=0;i<f.counts.x;i++) {
            Light light=buffers[NonUniformResourceIndex(root.frame)].Load<Light>(f.offsets.x+i*32);
            float3 delta=light.positionRadius.xyz-float3(v.world,0);float distance=length(delta);if(distance>=light.positionRadius.w)continue;
            float visible=light.colorShadows.w>0&&blocked(f,v.world,light.positionRadius.xy)?0:1;visibility=min(visibility,visible);
            float attenuation=pow(saturate(1-distance/light.positionRadius.w),2);
            illumination+=light.colorShadows.rgb*attenuation*saturate(dot(normal,delta/max(distance,0.00001)))*visible;
        }
    }
    float3 result=color.rgb*illumination;
    if(f.counts.z==1)result=color.rgb;
    else if(f.counts.z==2)result=normal*0.5+0.5;
    else if(f.counts.z==5)result=frac(float3(s.info.z)*float3(0.6180339,0.381966,0.7548776));
    else if(f.counts.z==6)result=float3(visibility);
    Output o;o.color=float4(result*color.a,color.a);o.id=float(s.info.z);return o;
}
