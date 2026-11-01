// Per-frame probe relighting (look/gi.rs): surface caches to 6x6 irradiance.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
// Local-light irradiance on a surface facing `n`; with LOOK_LOCAL_INDIRECT, shadowed
// by the lights that own a local shadow map this frame (look::local; the rest stay
// unshadowed).
float3 localIrradiance(Frame f,Look l,float3 p,float3 n){
    float3 sum=0;
    for(uint i=0;i<f.extent.z;i++){
        // look-lights.slangh: each light's form factor over its area.
        Light light=loadLight(f.lighting.w,i);float3 e=lightIrradiance(f,l,light,i,p,n);
        if(any(e>0) && (l.flags.y&LOOK_LOCAL_INDIRECT)!=0)e*=localShadow(l,i,p,n,light.positionRange.xyz);
        sum+=e;
    }return sum;
}
groupshared float3 radiance[64];
groupshared float4 directions[64];
groupshared float3 tile[36];
groupshared float4 seen[64];
// Distance (m) recorded for sky texels: reflections of the sky need no parallax.
static const float GI_SKY_DISTANCE=10000;
// Writes this probe's bordered 10x10 radiance tile (giSampleRadiance) from `seen`.
void storeRadiance(Look l,uint id,uint lane){
    for(uint i=lane;i<100;i+=64){
        int2 p=int2(i%10,i/10)-1;images[NonUniformResourceIndex(l.giRadiance.y)][giTile(l,id,8)+p]=seen[octIndex(p,8)];
    }
}
// Writes this probe's bordered 8x8 atlas tile from the 6x6 interior in `tile`.
void storeTile(Look l,uint id,uint lane){
    int2 p=int2(lane%8,lane/8)-1;images[NonUniformResourceIndex(l.giImages.z)][giTile(l,id,6)+p]=float4(tile[octIndex(p,6)],1);
}
// Width in metres of the sun shadow texel that tests `world`: its virtual shadow
// level's, else its cascade's.
float shadowTexel(Frame f,Look l,float3 world){
    if(l.vsm.y!=0)return vsmStep(l,vsmLevel(f,l,world,vsmLight(l,world)))/VSM_PAGE;
    uint c=min(cascadeIndex(f,world),max(f.look.z,1u)-1);return 2/(length(frameShadow(c)[0].xyz)*float(f.look.y));
}
// One workgroup per relit probe, one lane per cache texel: this frame's window
// (giUpdate.xy), then last frame's (giCounts.yz). The atlas written now held every
// probe two frames ago, so only last frame's window is stale in it: those groups copy
// it forward, skipping probes this frame relights. Inactive probes stay black. `pages`
// (a literal per entry point) shadows hits with virtual shadow pages first. A texel's
// hit stands for its whole footprint: its sun test sits eight shadow texels off it
// along its normal, so the surface does not shadow itself.
void giRelight(uint3 group,uint lane,bool pages){
    Frame f=frame();Look l=look(f);uint cursor=l.giUpdate.x,window=l.giUpdate.y,read=l.giImages.x;
    if(group.x>=window){
        uint id=l.giCounts.y+group.x-window;if(id>=l.giCounts.w || id-cursor<window)return;
        if(lane<36)tile[lane]=textures[NonUniformResourceIndex(read)].Load(int3(giTile(l,id,6)+uint2(lane%6,lane/6),0)).rgb;
        GroupMemoryBarrierWithGroupSync();storeTile(l,id,lane);return;
    }
    uint id=cursor+group.x;if(id>=l.giCounts.w)return;
    float4 meta=giMeta(l,id);
    if(meta.w<0.5){
        if(lane<36)tile[lane]=float3(0);
        seen[lane]=0;
        GroupMemoryBarrierWithGroupSync();storeTile(l,id,lane);storeRadiance(l,id,lane);return;
    }
    uint4 t=buffers[NonUniformResourceIndex(l.giBuffers.x)].Load<uint4>((id*64+lane)*16);
    float3 albedo=float3(t.x&255,(t.x>>8)&255,(t.x>>16)&255)/255.;float sky=float(t.x>>24)/255.;
    float3 normal=octDecode(float2(t.y&4095,(t.y>>12)&4095)/4095.);
    float3 emissive=float3(halfToFloat(t.z&65535),halfToFloat(t.z>>16),halfToFloat(t.w&65535));float distance=halfToFloat(t.w>>16);
    float3 dir=octDecode((float2(lane%8,lane/8)+0.5)/8),origin=giProbePosition(l,id)+meta.xyz,light=0;
    if(sky<1){
        float3 hit=origin+dir*distance,sun=f.sunDirection.xyz,lifted=hit+normal*(8*shadowTexel(f,l,hit));
        // Glass and films between the hit and the sun tint it (look-translucent.slangh).
        float3 irradiance=sunAt(f,l,hit)*saturate(dot(normal,sun))*visibility(f,l,lifted,normal,pages?vsmVisibility(f,l,lifted,normal,true):-1)*translucentSun(l,lifted)+localIrradiance(f,l,hit,normal);
        // Last frame's probes close the loop: multiple bounces converge over frames.
        irradiance+=giIrradiance(l,read,hit,normal);
        light=(1-sky)*(emissive+albedo/PI*irradiance);
    }
    if(sky>0)light+=sky*skyLight(l)*(physicalSky(l)?skyRadiance(f,l,dir,false):envTex(f.extra.w,environmentUv(dir)).rgb);
    radiance[lane]=light;directions[lane]=float4(dir,octSolidAngle(uint2(lane%8,lane/8),8));
    // The radiance cache keeps what this texel sees, filtered over relights like the
    // irradiance (a reset, or a non-finite history, starts it over).
    {
        float hysteresis=(l.flags.y&LOOK_RESET)!=0?0:l.giParams.x;
        float4 value=float4(light,sky>0.5?GI_SKY_DISTANCE:giParallax(t.y));
        float4 old=images[NonUniformResourceIndex(l.giRadiance.y)][giTile(l,id,8)+uint2(lane%8,lane/8)];
        if(hysteresis>0 && all(isfinite(old)))value.rgb=lerp(value.rgb,old.rgb,hysteresis);
        seen[lane]=value;
    }
    GroupMemoryBarrierWithGroupSync();
    if(lane<36){
        float3 n=octDecode((float2(lane%6,lane/6)+0.5)/6),sum=0;float normalization=0;
        // Each texel weighs by its solid angle: octahedral texels near the axes cover
        // about half the mean, so equal weights would overcount light from axis-aligned
        // openings (a window in an axis-aligned room) up to twofold.
        for(uint j=0;j<64;j++){float w=max(dot(n,directions[j].xyz),0)*directions[j].w;sum+=radiance[j]*w;normalization+=w;}
        float3 value=sum*(PI/max(normalization,1e-5));
        float3 old=textures[NonUniformResourceIndex(read)].Load(int3(giTile(l,id,6)+uint2(lane%6,lane/6),0)).rgb;
        float hysteresis=(l.flags.y&LOOK_RESET)!=0?0:l.giParams.x;
        tile[lane]=lerp(value,old,hysteresis);
    }
    GroupMemoryBarrierWithGroupSync();storeTile(l,id,lane);storeRadiance(l,id,lane);
}
[shader("compute")][numthreads(64,1,1)]
void giRelightMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){giRelight(group,lane,false);}
// With virtual shadow maps (look::vsm): one page compare per hit.
[shader("compute")][numthreads(64,1,1)]
void giRelightVsmMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){giRelight(group,lane,true);}
// Two probe lobes per half-resolution pixel (the full-resolution pixel 2p): rgb
// irradiance facing the normal (w: camera distance) and the radiance the surface
// reflects along its specular lobe's dominant direction (specularDirection, as ibl
// looks it up; w: volume coverage). Lighting upsamples them depth-aware, so the
// per-pixel cost of the eight-probe visibility search drops fourfold.
[shader("compute")][numthreads(8,8,1)]
void giGatherMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 p=int2(tid.xy)*2;bool covered;float3 world=gbufferWorld(f,p,covered);float4 diffuse=float4(0,0,0,1e9),specular=0;
    if(covered){
        float3 n=normalize(gbufferNormal(f,p)),v=normalize(f.camera.xyz-world.xyz),a,b;
        float roughness=gbufferMaterial(f,p).x;
        float coverage=giPoolLobes(l,l.giImages.y,world.xyz,n,specularDirection(n,v,roughness),v,roughness,true,a,b);
        if(coverage>0){diffuse.rgb=a;specular=float4(b,coverage);}
        diffuse.w=length(world.xyz-f.camera.xyz);
    }
    images[NonUniformResourceIndex(l.giGather.z)][tid.xy]=diffuse;
    images[NonUniformResourceIndex(l.giGather.w)][tid.xy]=specular;
}
// Writes probe records (Renderer::update_probe_surfaces) into the surface cache,
// meta and moment atlas in place, one group per probe. Root words: first record,
// staging buffer (records of [probe id, 0, 0, 0], 64 cache texels as uint4, 66
// static vectors: 64 of packed 16x16 moments (look/gi.rs pack_moments), then
// float4s), cache, meta, moment atlas (storage, bordered 18x18 tiles of UNORM16
// moments), probes per atlas row with bit 31 set to clear instead (probe = record
// index: zero surfaces, an inactive probe and dark irradiance tiles), then both
// irradiance atlases.
static const uint SCATTER_RECORD=16+64*16+66*16;
[shader("compute")][numthreads(128,1,1)]
void giScatterMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    uint index=root.frame+group.x,staging=root.materials,base=index*SCATTER_RECORD,statics=base+16+64*16;
    bool clear=(root.mode>>31)!=0;uint atlas=root.mode&0x7fffffffu;
    uint id=clear?index:buffers[NonUniformResourceIndex(staging)].Load(base);
    if(lane<64)buffers[NonUniformResourceIndex(root.material)].Store<uint4>((id*64+lane)*16,clear?uint4(0,0,0,0):buffers[NonUniformResourceIndex(staging)].Load<uint4>(base+16+lane*16));
    if(lane==0)buffers[NonUniformResourceIndex(root.entity)].Store<float4>(id*16,clear?float4(0,0,0,0):buffers[NonUniformResourceIndex(staging)].Load<float4>(statics+64*16));
    for(uint i=lane;i<18*18;i+=128){
        // Border texels fold across the octahedral edges (DDGI borders, octIndex).
        uint t=octIndex(int2(i%18,i/18)-1,16),x=t%16,y=t/16;
        uint v=clear?0:buffers[NonUniformResourceIndex(staging)].Load(statics+((y/2)*8+x/2)*16+((y&1)*2+(x&1))*4);
        images[NonUniformResourceIndex(root.cascade)][uint2(id%atlas,id/atlas)*18+uint2(i%18,i/18)]=float4(float(v&65535)/65535,float(v>>16)/65535,0,0);
    }
    if(clear && lane<64){
        uint2 texel=uint2(id%atlas,id/atlas)*8+uint2(lane%8,lane/8);
        images[NonUniformResourceIndex(root.sample)][texel]=0;images[NonUniformResourceIndex(root.flags)][texel]=0;
    }
}
