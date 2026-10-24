// One-time probe surface acquisition for raster GI (ara_render::look::gi). Ray
// queries run only here, at bake time; the raster renderer relights the caches.
#define MATERIAL_LAYERS 1
#include "common.slangh"
static const float RANGE=200;
groupshared uint backs[64];
groupshared float hits[64];
groupshared float3 directions[64];
groupshared float3 offset;
uint unorm8(float v){return uint(round(saturate(v)*255));}
// Nonnegative float to IEEE half bits (truncating, flushing denormals, clamping to
// the largest finite half); avoids 16-bit shader capabilities.
uint halfBits(float v){uint x=asuint(max(v,0.));int e=int((x>>23)&255)-112;if(e<=0)return 0;if(e>=31)return 0x7bff;return (uint(e)<<10)|((x&0x7fffff)>>13);}
// Root: pass = surface cache buffer, arg = statics buffer, pad0 = first probe.
// One workgroup per probe, one lane per 8x8 octahedral cache texel.
[shader("compute")][numthreads(64,1,1)]
void probeSurfacesMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint id=group.x+root.pad0;if(id>=f.counts.w)return;
    float3 centre=probePosition(f,id);uint rng=hash(id*64+lane);
    // Relocation (as DDGI): 64 Fibonacci rays find back faces (the probe is inside
    // geometry) and the nearest surface, then move the probe once.
    float y=1-2*(lane+0.5)/64,phi=lane*2.39996323;float r=sqrt(max(0,1-y*y));float3 d=float3(cos(phi)*r,y,sin(phi)*r);
    Hit hit;float distance=RANGE;uint back=0;
    if(trace(f,centre,d,RANGE,rng,hit)){Surface s=surface(f,hit.id,hit.primitive,hit.bary);distance=hit.distance;back=dot(s.geometricNormal,d)>0 && s.transmission==0?1:0;}
    backs[lane]=back;hits[lane]=distance;directions[lane]=d;GroupMemoryBarrierWithGroupSync();
    if(lane==0){
        float nearest=RANGE,nearestBack=RANGE;uint nearestLane=0,backLane=0,count=0;
        for(uint j=0;j<64;j++){count+=backs[j];if(backs[j]!=0 && hits[j]<nearestBack){nearestBack=hits[j];backLane=j;}if(hits[j]<nearest){nearest=hits[j];nearestLane=j;}}
        float3 relocation=0;
        if(count>16)relocation=directions[backLane]*min(length(f.spacing.xyz)*0.25,nearestBack+0.03);
        else if(nearest<0.12)relocation=-directions[nearestLane]*(0.12-nearest);
        offset=clamp(relocation,-f.spacing.xyz*0.45,f.spacing.xyz*0.45);
    }
    GroupMemoryBarrierWithGroupSync();
    float3 origin=centre+offset;
    // Classify from the final position: a quarter of back faces means still inside.
    back=0;if(trace(f,origin,d,RANGE,rng,hit)){Surface s=surface(f,hit.id,hit.primitive,hit.bary);back=dot(s.geometricNormal,d)>0 && s.transmission==0?1:0;}
    GroupMemoryBarrierWithGroupSync();backs[lane]=back;GroupMemoryBarrierWithGroupSync();
    uint backCount=0;for(uint j=0;j<64;j++)backCount+=backs[j];
    // Sixteen stratified rays inside this lane's texel: averaged surface data
    // smears small emitters over their texel instead of aliasing them away.
    float3 albedo=0,emissive=0,normal=0;float meanHit=0,moment1=0,moment2=0;uint geometry=0;
    for(uint k=0;k<16;k++){
        float2 uv=(float2(lane%8,lane/8)+(float2(k%4,k/4)+0.5)/4)/8;float3 dir=octDecode(uv);
        Hit h;float t=RANGE;
        if(trace(f,origin,dir,RANGE,rng,h)){
            Surface s=surface(f,h.id,h.primitive,h.bary);t=h.distance;geometry++;meanHit+=t;
            bool front=dot(s.geometricNormal,dir)<0 || s.transmission>0;
            if(front){if(dot(s.normal,-dir)<0)s.normal=-s.normal;albedo+=s.albedo*(1-s.metal);emissive+=s.emissive;normal+=s.normal;}
            else normal-=dir;
        }
        moment1+=t;moment2+=t*t;
    }
    float sky=1-geometry/16.;float g=max(float(geometry),1);
    albedo/=g;emissive/=g;meanHit/=g;normal=length(normal)>0?normalize(normal):-octDecode((float2(lane%8,lane/8)+0.5)/8);
    float2 n=octEncode(normal);
    uint4 packed=uint4(unorm8(albedo.r)|(unorm8(albedo.g)<<8)|(unorm8(albedo.b)<<16)|(unorm8(sky)<<24),
        uint(round(n.x*65535))|(uint(round(n.y*65535))<<16),
        halfBits(emissive.r)|(halfBits(emissive.g)<<16),
        halfBits(emissive.b)|(halfBits(meanHit)<<16));
    buffers[NonUniformResourceIndex(root.pass)].Store<uint4>((id*64+lane)*16,packed);
    buffers[NonUniformResourceIndex(root.arg)].Store<float4>((id*66+lane)*16,float4(moment1/16,moment2/16,0,0));
    if(lane==0){
        // Still inside geometry after moving: classified inactive, never sampled.
        buffers[NonUniformResourceIndex(root.arg)].Store<float4>((id*66+64)*16,float4(offset,backCount>16?0:1));
        buffers[NonUniformResourceIndex(root.arg)].Store<float4>((id*66+65)*16,float4(backCount,0,0,0));
    }
}
