// Per-frame probe relighting (look/gi.rs): surface caches to 6x6 irradiance.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
// Unshadowed local-light irradiance on a surface facing `n`.
float3 localIrradiance(Frame f,float3 p,float3 n){
    float3 sum=0;
    for(uint i=0;i<f.extent.z;i++){
        Light light=buffers[NonUniformResourceIndex(f.lighting.w)].Load<Light>(i*64);
        float3 delta=light.positionRange.xyz-p;float d2=max(dot(delta,delta),0.01),d=sqrt(d2);if(d>light.positionRange.w)continue;
        float3 toLight=delta/d;float fade=pow(saturate(1-pow(d/light.positionRange.w,4)),2);float spot=1;
        if(light.parameters.x>0.5)spot=smoothstep(light.directionOuter.w,light.parameters.y,dot(-toLight,light.directionOuter.xyz));
        float solidAngle=light.parameters.x>0.5?2*PI*(1-light.directionOuter.w):4*PI;
        sum+=light.colorLumens.rgb*(light.colorLumens.w/(solidAngle*d2))*fade*spot*saturate(dot(n,toLight));
    }return sum;
}
// Relative solid angle of each 8x8 octahedral texel (mean one): the mapping's
// texels span 2.75x different areas of the sphere, so integration weights them.
static const float TEXEL_SOLID_ANGLE[64]={0.7166,1.1795,0.9555,0.5363,0.5328,0.9555,1.1795,0.7165,1.1795,1.4629,1.0142,0.9555,0.9555,1.0122,1.4629,1.1795,0.9555,1.0142,1.4629,1.1795,1.1795,1.4629,1.0162,0.9555,0.5363,0.9555,1.1795,0.7165,0.7165,1.1795,0.9555,0.5398,0.5328,0.9555,1.1795,0.7165,0.7166,1.1795,0.9555,0.5363,0.9555,1.0122,1.4629,1.1795,1.1795,1.4629,1.0142,0.9555,1.1795,1.4629,1.0162,0.9555,0.9555,1.0142,1.4629,1.1795,0.7165,1.1795,0.9555,0.5398,0.5363,0.9555,1.1795,0.7165};
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
        float3 irradiance=sunAt(f,l,hit)*saturate(dot(normal,sun))*visibility(f,hit,normal)+localIrradiance(f,hit,normal);
        // Last frame's probes close the loop: multiple bounces converge over frames.
        irradiance+=giIrradiance(l,read,hit,normal,normal*l.giSpacing.w);
        light=(1-sky)*(emissive+albedo/PI*irradiance);
    }
    if(sky>0)light+=sky*(physicalSky(l)?skyRadiance(f,l,dir,false):envTex(f.extra.w,environmentUv(dir)).rgb);
    radiance[lane]=light;directions[lane]=dir;GroupMemoryBarrierWithGroupSync();
    if(lane<36){
        float3 n=octDecode((float2(lane%6,lane/6)+0.5)/6),sum=0;float normalization=0;
        for(uint j=0;j<64;j++){float w=max(dot(n,directions[j]),0)*TEXEL_SOLID_ANGLE[j];sum+=radiance[j]*w;normalization+=w;}
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
    int2 p=int2(tid.xy)*2;float4 world=pixel(f.gbuffer.w,p);float4 diffuse=float4(0,0,0,1e9),specular=0;
    if(world.w>0){
        float3 n=normalize(pixel(f.gbuffer.y,p).xyz),v=normalize(f.camera.xyz-world.xyz);float coverage=giCoverage(l,world.xyz);
        if(coverage>0){float3 a,b;giLobes(l,l.giImages.y,world.xyz,n,reflect(-v,n),giBias(l,n,v),a,b);diffuse.rgb=a;specular=float4(b,coverage);}
        diffuse.w=length(world.xyz-f.camera.xyz);
    }
    images[NonUniformResourceIndex(l.giGather.z)][tid.xy]=diffuse;
    images[NonUniformResourceIndex(l.giGather.w)][tid.xy]=specular;
}
