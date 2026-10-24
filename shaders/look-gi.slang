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
groupshared float3 radiance[64];
groupshared float3 directions[64];
// One workgroup per probe, one lane per cache texel. Probes outside this frame's
// update window (or classified inside geometry) carry their irradiance forward.
[shader("compute")][numthreads(64,1,1)]
void giRelightMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();Look l=look(f);uint id=group.x;if(id>=l.giCounts.w)return;
    uint read=l.giBuffers.z,write=l.giBuffers.w;float4 meta=giStatic(l,id,64);
    uint relative=(id+l.giCounts.w-l.giUpdate.x)%l.giCounts.w;
    if(relative>=l.giUpdate.y || meta.w<0.5){
        if(lane<GI_IRRADIANCE)buffers[NonUniformResourceIndex(write)].Store<float4>((id*GI_IRRADIANCE+lane)*16,meta.w<0.5?float4(0):giTexel(read,id,lane));
        return;
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
    if(lane<GI_IRRADIANCE){
        float3 n=octDecode((float2(lane%6,lane/6)+0.5)/6),sum=0;float normalization=0;
        for(uint j=0;j<64;j++){float w=max(dot(n,directions[j]),0);sum+=radiance[j]*w;normalization+=w;}
        float3 value=sum*(PI/max(normalization,1e-5));
        float3 old=giTexel(read,id,lane).rgb;float hysteresis=(l.flags.y&LOOK_RESET)!=0?0:l.giOrigin.w;
        buffers[NonUniformResourceIndex(write)].Store<float4>((id*GI_IRRADIANCE+lane)*16,float4(lerp(value,old,hysteresis),1));
    }
}
