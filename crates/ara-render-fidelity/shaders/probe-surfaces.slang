// One-time probe surface acquisition for raster GI (ara_render::look::gi). Ray
// queries run only here, at bake time; the raster renderer relights the caches.
#define MATERIAL_LAYERS 1
// nativeAccept lets probe rays through glass by its transmittance.
#define PROBE_SURFACES 1
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
// gi_cpu::bounce_albedo: diffuse plus the specular lobe's directional albedo at
// `roughness` seen from 60 degrees (Karis's EnvBRDFApprox; 0.45 F0 at roughness 1).
float3 bounceAlbedo(float3 base,float metal,float roughness){
    float r=saturate(roughness),x=1-r,y=0.0425-0.0275*r;float a004=min(x*x,exp2(-9.28*0.5))*x+y;
    float scale=1.04-0.572*r-1.04*a004,bias=1.04*a004-0.04+0.022*r;
    return max(base*(1-metal)+lerp(float3(0.04),base,metal)*scale+bias,0);
}
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
    // Distance moments: this lane's 2x2 of the probe's 16x16 visibility texels, four
    // rays each, every ray clamped at twice the lattice spacing (DDGI clamps its ray
    // distances: shading weighs points within one cell, and a missed ray's far
    // distance would otherwise dominate a texel that also sees a near wall).
    float3 albedo=0,emissive=0,normal=0,centroid=0;float4 moment1=0,moment2=0;uint geometry=0;
    float limit=2*max(f.spacing.x,max(f.spacing.y,f.spacing.z));
    // Front hits' distances (-1: none) and normals, for the relit hit below.
    float fronts[16];float3 normals[16];
    for(uint k=0;k<16;k++){
        float2 uv=(float2(lane%8,lane/8)+(float2(k%4,k/4)+0.5)/4)/8;float3 dir=octDecode(uv);
        Hit h;float t=RANGE;fronts[k]=-1;normals[k]=0;
        if(trace(f,origin,dir,RANGE,rng,h)){
            Surface s=surface(f,h.id,h.primitive,h.bary);t=h.distance;geometry++;centroid+=dir*t;
            bool front=dot(s.geometricNormal,dir)<0 || s.transmission>0;
            // Diffuse plus the specular lobe's directional albedo at the hit's roughness
            // from 60 degrees (gi_cpu::bounce_albedo): metals bounce light rather than absorb it.
            // Glass that stopped the ray bounces its reflection only (its transmitted
            // share went on to the surfaces behind it, nativeAccept).
            if(front){if(dot(s.normal,-dir)<0)s.normal=-s.normal;albedo+=bounceAlbedo(s.albedo*(1-s.transmission),s.metal,s.roughness);emissive+=s.emissive;normal+=s.normal;
                fronts[k]=t;normals[k]=s.normal;}
            else normal-=dir;
        }
        // A miss is open as far as the limit (rays stop at RANGE, which a coarse
        // lattice's cells exceed).
        float m=t<RANGE?min(t,limit):limit;uint quadrant=(k%4)/2+(k/8)*2;
        moment1[quadrant]+=m;moment2[quadrant]+=m*m;
    }
    float sky=1-geometry/16.;float g=max(float(geometry),1);
    albedo/=g;emissive/=g;centroid/=g;normal=length(normal)>0?normalize(normal):-octDecode((float2(lane%8,lane/8)+0.5)/8);
    // The centre ray's distance to the plane through the hits' centroid along their
    // mean normal. Relighting (look-gi.slang giRelight) rebuilds one hit per texel on
    // the centre ray; the hits' mean distance would land behind a surface seen at an
    // angle (distance grows as 1/cos across the texel), where it shadows itself.
    float3 axis=octDecode((float2(lane%8,lane/8)+0.5)/8);float facing=dot(normal,axis);
    float hitDistance=facing<-0.05?clamp(dot(normal,centroid)/facing,0,RANGE):length(centroid);
    // Reflections take the probe's radiance at this distance (parallax), as seen;
    // eight bits of its base-2 log (ara_render::look::gi::log_distance).
    uint parallax=uint(clamp(round((log2(max(hitDistance,1e-6))+8)/24*255),0,255));
    // Relighting uses a real hit (gi_cpu relit_hit): the front hit closest to the
    // texel's centre (nearest first on ties), with its own normal. A point rebuilt from
    // the mean would float between the surfaces of a texel that sees two (a window frame
    // and the ground outside, a block and the wall behind), where relighting would find
    // light no surface it sees receives.
    uint ray=5;float best=1e9;
    for(uint k=0;k<16;k++){
        if(fronts[k]<0)continue;
        float c=pow(float(k%4)-1.5,2)+pow(float(k/4)-1.5,2);
        if(c<best||(c==best&&fronts[k]<fronts[ray])){best=c;ray=k;}
    }
    if(best<1e9){hitDistance=fronts[ray];normal=normals[ray];}
    float2 n=octEncode(normal);
    uint4 packed=uint4(unorm8(albedo.r)|(unorm8(albedo.g)<<8)|(unorm8(albedo.b)<<16)|(unorm8(sky)<<24),
        uint(round(n.x*1023))|(uint(round(n.y*1023))<<10)|(ray<<20)|(parallax<<24),
        halfBits(emissive.r)|(halfBits(emissive.g)<<16),
        halfBits(emissive.b)|(halfBits(hitDistance)<<16));
    buffers[NonUniformResourceIndex(root.pass)].Store<uint4>((id*64+lane)*16,packed);
    // ara_render::look::gi::pack_moments: UNORM16 mean | mean squared << 16, over the
    // limit (and its square).
    uint4 moments=uint4(round(saturate(moment1/(4*limit))*65535))|(uint4(round(saturate(moment2/(4*limit*limit))*65535))<<16);
    buffers[NonUniformResourceIndex(root.arg)].Store<uint4>((id*66+lane)*16,moments);
    if(lane==0){
        // Still inside geometry after moving: classified inactive, never sampled.
        buffers[NonUniformResourceIndex(root.arg)].Store<float4>((id*66+64)*16,float4(offset,backCount>16?0:1));
        buffers[NonUniformResourceIndex(root.arg)].Store<float4>((id*66+65)*16,float4(backCount,0,0,0));
    }
}
