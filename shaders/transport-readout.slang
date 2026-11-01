// Native-resolution surface-patch readout. No screen colour/history feeds the
// transport integral. The control's specular and direct-light paths are retained.
#include "baseline.slang"

// Sorted rows: int3 world cell + native instance id, first patch + count + padding.
bool transportLess(int4 a,int4 b) {
    if(a.w!=b.w)return a.w<b.w;
    if(a.x!=b.x)return a.x<b.x;
    if(a.y!=b.y)return a.y<b.y;
    return a.z<b.z;
}
uint2 transportCell(uint record,int4 key) {
    uint4 head=buffers[record].Load<uint4>(0);uint lo=0,hi=head.x;
    while(lo<hi) {
        uint mid=(lo+hi)/2;int4 found=buffers[record].Load<int4>(16+mid*32);
        if(transportLess(found,key))lo=mid+1;else hi=mid;
    }
    if(lo>=head.x)return uint2(0,0);
    int4 found=buffers[record].Load<int4>(16+lo*32);
    if(any(found!=key))return uint2(0,0);
    return buffers[record].Load<uint2>(32+lo*32);
}

float3 transportIrradiance(Frame f,int2 pixelPosition,float3 world,float3 normal) {
    uint record=root.entity;uint4 head=buffers[record].Load<uint4>(0);
    uint entity=uint(round(pixel(f.extra.y,pixelPosition).r));
    int3 cell=int3(floor(world/0.25));
    // Native tangent-space hat basis. Reject the opposite normal and plane, so a
    // thin wall does not read the patch on its other side.
    float3 axis=abs(normal.y)<0.9?float3(0,1,0):float3(1,0,0);
    float3 tangent=normalize(cross(axis,normal)),bitangent=cross(normal,tangent);
    float3 value=0;float weight=0,nearest=1e30;float3 fallback=0;
    for(int z=-1;z<=1;++z)for(int y=-1;y<=1;++y)for(int x=-1;x<=1;++x) {
        uint2 span=transportCell(record,int4(cell+int3(x,y,z),int(entity)));
        for(uint i=0;i<span.y;++i) {
            uint offset=head.y+(span.x+i)*48;
            float4 p=buffers[record].Load<float4>(offset);
            float4 n=buffers[record].Load<float4>(offset+16);
            float3 e=buffers[record].Load<float3>(offset+32);
            for(uint channel=0;channel<asuint(n.w);++channel) {
                uint response=asuint(p.w)+channel*32;
                e+=buffers[record].Load<float3>(response)*buffers[record].Load<float3>(response+16);
            }
            float3 delta=world-p.xyz;
            if(dot(n.xyz,normal)<0.8||abs(dot(delta,n.xyz))>0.025)continue;
            float distance=dot(delta,delta);
            if(distance<nearest){nearest=distance;fallback=e;}
            float w=max(0,1-abs(dot(delta,tangent))/0.25)*max(0,1-abs(dot(delta,bitangent))/0.25);
            value+=e*w;weight+=w;
        }
    }
    if(weight>0)return value/weight;
    if(nearest<1e29)return fallback;
    // Awake bodies use a readout-only 25 cm irradiance volume, never patch rows.
    float3 blend=world/0.25-float3(cell);float3 received=0;
    for(int z=0;z<=1;++z)for(int y=0;y<=1;++y)for(int x=0;x<=1;++x) {
        uint2 span=transportCell(record,int4(cell+int3(x,y,z),0));
        if(span.y!=6)continue;
        float w=(x!=0?blend.x:1-blend.x)*(y!=0?blend.y:1-blend.y)*(z!=0?blend.z:1-blend.z);
        for(uint axis=0;axis<3;++axis) {
            float component=axis==0?normal.x:axis==1?normal.y:normal.z;
            uint side=axis*2+(component<0?1:0);
            received+=buffers[record].Load<float3>(head.y+(span.x+side)*48+32)*(w*component*component);
        }
    }
    return received;
}

float4 transportLighting(FullscreenVarying input,bool diffuseOnly) {
    Frame f=frame();Look l=look(f);int2 p=int2(input.clip.xy);
    float2 uv=(float2(p)+0.5)/float2(f.extent.xy);bool covered;
    float3 world=gbufferWorld(f,p,covered);
    if(!covered)return float4(diffuseOnly?0:exposeHdr(f,l,background(f,l,viewRay(f,float2(p)+0.5),uv)),1);
    Surface s;float4 a=gbufferAlbedo(f,p);float2 m=gbufferMaterial(f,p);
    s.albedo=a.rgb;s.alpha=1;s.transmission=0;s.ao=min(dynamicOcclusion(l)?1:a.a,screenOcclusion(f,l,p,length(world-f.camera.xyz),true));
    s.normal=gbufferNormal(f,p);s.roughness=m.x;s.metal=m.y;s.emissive=gbufferEmissive(f,p);
    float3 v=normalize(f.camera.xyz-world);Lobes lobes=specularLobes(s,v);
    float3 bounce=transportIrradiance(f,p,world,s.normal)*lobes.diffuse;
    if(diffuseOnly)return float4(exposeHdr(f,l,bounce),1);
    float3 controlDiffuse=ibl(f,l,s,lobes,world,v,p,true,true,false,true);
    float3 radiance=shade(f,l,s,world,p,true,true,false)-controlDiffuse+bounce;
    return float4(exposeHdr(f,l,applyFog(f,l,radiance,world,uv)),1);
}
[shader("fragment")]
float4 transportLightingFragment(FullscreenVarying input):SV_Target0 {return transportLighting(input,false);}
[shader("fragment")]
float4 transportDiffuseFragment(FullscreenVarying input):SV_Target0 {return transportLighting(input,true);}
