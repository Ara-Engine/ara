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
        Light light=buffers[NonUniformResourceIndex(f.lighting.w)].Load<Light>(i*64);
        float3 delta=light.positionRange.xyz-p;float d2=max(dot(delta,delta),max(0.01,light.parameters.z*light.parameters.z)),d=sqrt(d2);if(d>light.positionRange.w)continue;
        float3 toLight=normalize(delta);float fade=pow(saturate(1-pow(d/light.positionRange.w,4)),2);float spot=1;
        if(light.parameters.x>0.5)spot=smoothstep(light.directionOuter.w,light.parameters.y,dot(-toLight,light.directionOuter.xyz));
        float3 e=light.colorLumens.rgb*(light.colorLumens.w/(4*PI*d2))*fade*spot*saturate(dot(n,toLight));
        if(any(e>0) && (l.flags.y&LOOK_LOCAL_INDIRECT)!=0)e*=localShadow(l,i,p,n,light.positionRange.xyz);
        sum+=e;
    }return sum;
}
groupshared float3 radiance[64];
groupshared float3 directions[64];
groupshared float3 tile[36];
// Writes this probe's bordered 8x8 atlas tile from the 6x6 interior in `tile`.
void storeTile(Look l,uint id,uint lane){
    int2 p=int2(lane%8,lane/8)-1;images[NonUniformResourceIndex(l.giImages.z)][giTile(l,id,6)+p]=float4(tile[octIndex(p,6)],1);
}
// One workgroup per probe, one lane per cache texel. Probes outside this frame's
// update window carry their irradiance forward; inactive probes stay black.
[shader("compute")][numthreads(64,1,1)]
void giRelightMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();Look l=look(f);uint id=group.x;if(id>=l.giCounts.w)return;
    float4 meta=giMeta(l,id);uint read=l.giImages.x;
    uint relative=(id+l.giCounts.w-l.giUpdate.x)%l.giCounts.w;
    if(relative>=l.giUpdate.y || meta.w<0.5){
        if(lane<36)tile[lane]=meta.w<0.5?float3(0):textures[NonUniformResourceIndex(read)].Load(int3(giTile(l,id,6)+uint2(lane%6,lane/6),0)).rgb;
        GroupMemoryBarrierWithGroupSync();storeTile(l,id,lane);return;
    }
    uint4 t=buffers[NonUniformResourceIndex(l.giBuffers.x)].Load<uint4>((id*64+lane)*16);
    float3 albedo=float3(t.x&255,(t.x>>8)&255,(t.x>>16)&255)/255.;float sky=float(t.x>>24)/255.;
    float3 normal=octDecode(float2(t.y&65535,t.y>>16)/65535.);
    float3 emissive=float3(halfToFloat(t.z&65535),halfToFloat(t.z>>16),halfToFloat(t.w&65535));float distance=halfToFloat(t.w>>16);
    float3 dir=octDecode((float2(lane%8,lane/8)+0.5)/8),origin=giProbePosition(l,id)+meta.xyz,light=0;
    if(sky<1){
        float3 hit=origin+dir*distance,sun=f.sunDirection.xyz;
        float3 irradiance=sunAt(f,l,hit)*saturate(dot(normal,sun))*visibility(f,l,hit,normal)+localIrradiance(f,l,hit,normal);
        // Last frame's probes close the loop: multiple bounces converge over frames.
        irradiance+=giIrradiance(l,read,hit,normal,normal*l.giSpacing.w);
        light=(1-sky)*(emissive+albedo/PI*irradiance);
    }
    if(sky>0)light+=sky*(physicalSky(l)?skyRadiance(f,l,dir,false):envTex(f.extra.w,environmentUv(dir)).rgb);
    radiance[lane]=light;directions[lane]=dir;GroupMemoryBarrierWithGroupSync();
    if(lane<36){
        float3 n=octDecode((float2(lane%6,lane/6)+0.5)/6),sum=0;float normalization=0;
        for(uint j=0;j<64;j++){float w=max(dot(n,directions[j]),0);sum+=radiance[j]*w;normalization+=w;}
        float3 value=sum*(PI/max(normalization,1e-5));
        float3 old=textures[NonUniformResourceIndex(read)].Load(int3(giTile(l,id,6)+uint2(lane%6,lane/6),0)).rgb;
        float hysteresis=(l.flags.y&LOOK_RESET)!=0?0:l.giOrigin.w;
        tile[lane]=lerp(value,old,hysteresis);
    }
    GroupMemoryBarrierWithGroupSync();storeTile(l,id,lane);
}
// Two probe lobes per half-resolution pixel (the full-resolution pixel 2p): rgb
// irradiance facing the normal (w: camera distance) and facing the mirror direction
// (w: volume coverage). Lighting upsamples them depth-aware, so the per-pixel cost
// of the eight-probe visibility search drops fourfold.
[shader("compute")][numthreads(8,8,1)]
void giGatherMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 p=int2(tid.xy)*2;bool covered;float3 world=gbufferWorld(f,p,covered);float4 diffuse=float4(0,0,0,1e9),specular=0;
    if(covered){
        float3 n=normalize(gbufferNormal(f,p)),v=normalize(f.camera.xyz-world.xyz);float coverage=giCoverage(l,world.xyz);
        if(coverage>0){float3 a,b;giLobes(l,l.giImages.y,world.xyz,n,reflect(-v,n),giBias(l,n,v),a,b);diffuse.rgb=a;specular=float4(b,coverage);}
        diffuse.w=length(world.xyz-f.camera.xyz);
    }
    images[NonUniformResourceIndex(l.giGather.z)][tid.xy]=diffuse;
    images[NonUniformResourceIndex(l.giGather.w)][tid.xy]=specular;
}
// Bordered moment-tile texel (-1..=8) folded onto the octahedral interior, as
// renderer/gi.rs `fold` (DDGI borders).
uint foldMoment(int x,int y){
    if(x<0){x=-x-1;y=7-y;}else if(x>=8){x=15-x;y=7-y;}
    if(y<0){y=-y-1;x=7-x;}else if(y>=8){y=15-y;x=7-x;}
    return uint(y*8+x);
}
// Writes probe records (Renderer::update_probe_surfaces) into the surface cache,
// meta and moment atlas in place, one group per probe. Root words: first record,
// staging buffer (records of [probe id, 0, 0, 0], 64 cache texels as uint4, 66
// static vectors as float4), cache, meta, moment atlas (storage), probes per atlas
// row with bit 31 set to clear instead (probe = record index: zero surfaces, an
// inactive probe and dark irradiance tiles), then both irradiance atlases.
static const uint SCATTER_RECORD=16+64*16+66*16;
[shader("compute")][numthreads(128,1,1)]
void giScatterMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    uint index=root.frame+group.x,staging=root.materials,base=index*SCATTER_RECORD,statics=base+16+64*16;
    bool clear=(root.mode>>31)!=0;uint atlas=root.mode&0x7fffffffu;
    uint id=clear?index:buffers[NonUniformResourceIndex(staging)].Load(base);
    if(lane<64)buffers[NonUniformResourceIndex(root.material)].Store<uint4>((id*64+lane)*16,clear?uint4(0,0,0,0):buffers[NonUniformResourceIndex(staging)].Load<uint4>(base+16+lane*16));
    if(lane==0)buffers[NonUniformResourceIndex(root.entity)].Store<float4>(id*16,clear?float4(0,0,0,0):buffers[NonUniformResourceIndex(staging)].Load<float4>(statics+64*16));
    if(lane<100){
        int x=int(lane%10),y=int(lane/10);
        float4 v=clear?float4(0,0,0,0):buffers[NonUniformResourceIndex(staging)].Load<float4>(statics+foldMoment(x-1,y-1)*16);
        images[NonUniformResourceIndex(root.cascade)][uint2(id%atlas,id/atlas)*10+uint2(x,y)]=float4(v.xy,0,0);
    }
    if(clear && lane<64){
        uint2 texel=uint2(id%atlas,id/atlas)*8+uint2(lane%8,lane/8);
        images[NonUniformResourceIndex(root.sample)][texel]=0;images[NonUniformResourceIndex(root.flags)][texel]=0;
    }
}
