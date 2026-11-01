// Physical sky LUTs, the sky's image-based lighting and froxel fog (look/passes.rs).
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "look-fog.slangh"
void store(uint image,uint2 p,float3 value){images[NonUniformResourceIndex(image)][p]=float4(value,1);}
// The atmosphere's LUTs (atmosphere-model.slangh; transmittance 256x64, multiple
// scattering 32x32, sky view 192x108 at the camera's altitude).
[shader("compute")][numthreads(8,8,1)]
void transmittanceMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=ATMOSPHERE_TRANSMITTANCE))return;
    store(l.storage0.x,tid.xy,atmosphereTransmittanceTexelValue(l.sky,(float2(tid.xy)+0.5)/float2(ATMOSPHERE_TRANSMITTANCE)));
}
[shader("compute")][numthreads(8,8,1)]
void multiScatterMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=ATMOSPHERE_MULTISCATTER))return;
    store(l.storage0.y,tid.xy,atmosphereMultiScatterTexelValue(l.sky,l.sampled0.x,(float2(tid.xy)+0.5)/float2(ATMOSPHERE_MULTISCATTER)));
}
[shader("compute")][numthreads(8,8,1)]
void skyViewMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=ATMOSPHERE_SKY_VIEW))return;
    float r=atmosphereCamera(l.sky,f.camera.xyz).y;
    store(l.storage0.z,tid.xy,atmosphereSkyViewTexelValue(l.sky,l.sampled0.x,l.sampled0.y,r,(float2(tid.xy)+0.5)/float2(ATMOSPHERE_SKY_VIEW)));
}
float radicalInverse(uint bits){bits=(bits<<16)|(bits>>16);bits=((bits&0x55555555)<<1)|((bits&0xAAAAAAAA)>>1);bits=((bits&0x33333333)<<2)|((bits&0xCCCCCCCC)>>2);bits=((bits&0x0F0F0F0F)<<4)|((bits&0xF0F0F0F0)>>4);bits=((bits&0x00FF00FF)<<8)|((bits&0xFF00FF00)>>8);return float(bits)*2.3283064365386963e-10;}
float3 basisSample(float3 n,float3 local){float3 t=normalize(cross(abs(n.y)<0.99?float3(0,1,0):float3(1,0,0),n));return t*local.x+cross(n,t)*local.y+n*local.z;}
// 32x16 diffuse irradiance of the sky view (cosine-weighted Hammersley).
[shader("compute")][numthreads(8,8,1)]
void skyIrradianceMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=uint2(32,16)))return;
    float3 n=environmentDirection((float2(tid.xy)+0.5)/float2(32,16)),sum=0;
    for(uint i=0;i<256;i++){float2 u=float2((i+0.5)/256.,radicalInverse(i));float phi=2*PI*u.x,r=sqrt(u.y);sum+=skyRadiance(f,l,basisSample(n,float3(cos(phi)*r,sin(phi)*r,sqrt(1-u.y))),false);}
    store(l.storage1.x,tid.xy,sum*PI/256);
}
// 256x640 GGX prefilter strip of the sky view, five 256x128 roughness levels.
// Level 0 is the sky itself; rougher levels importance-sample 48 GGX lobes.
[shader("compute")][numthreads(8,8,1)]
void skyPrefilterMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=uint2(256,640)))return;
    uint level=tid.y/128;float3 n=environmentDirection((float2(tid.x,tid.y%128)+0.5)/float2(256,128));
    if(level==0){store(l.storage1.y,tid.xy,skyRadiance(f,l,n,false));return;}
    float a=float(level)/4;a*=a;float3 sum=0;float weight=0;
    for(uint i=0;i<48;i++){
        float2 u=float2((i+0.5)/48.,radicalInverse(i));float phi=2*PI*u.x;float cosine=sqrt((1-u.y)/(1+(a*a-1)*u.y)),sine=sqrt(max(0.,1-cosine*cosine));
        float3 h=basisSample(n,float3(cos(phi)*sine,sin(phi)*sine,cosine)),d=reflect(-n,h);float nl=saturate(dot(n,d));
        if(nl>0){sum+=skyRadiance(f,l,d,false)*nl;weight+=nl;}
    }
    store(l.storage1.y,tid.xy,sum/max(weight,0.001));
}
// One group per frustum column, one thread per logarithmic slice (FOG_SLICES; look.slangh
// sampleFroxel): each evaluates its segment, then accumulates the
// segments in front of it in order (front to back, deterministic). On near volume
// frames (LOOK_VOLUMETRIC) the column starts where the near volume ends, so shading
// composes the two exactly (applyFog). Volumetric fog's sun is shadowed by the
// cascades and far map (each segment the lit share of four points, so shafts move
// smoothly with the camera instead of a whole slice flipping lit or dark at once), and
// local lights scatter in it unless they scatter per pixel (fogLightsOn); quadrature is
// deterministic (no noise). Height fog and
// fog volumes come from fogMedium; under the physical sky the atmosphere's aerial
// perspective joins them (atmosphereAerial: both atmosphere lights, multiple scattering,
// from the start depth, stretched by the view distance scale; only light 0's single
// scattering is shadowed), and the height fog's sun and sky light scale by the height
// fog contribution (as Unreal's height fog beyond its volumetric fog).
groupshared float3 fogSegmentLight[FOG_SLICES],fogSegmentTrans[FOG_SLICES];
[shader("compute")][numthreads(64,1,1)]
void fogMain(uint3 group:SV_GroupID,uint z:SV_GroupIndex){
    Frame f=frame();Look l=look(f);
    float2 uv=(float2(group.xy)+0.5)/float2(64,36);float3 d=viewRay(f,uv*float2(f.extent.xy));
    bool physical=physicalSky(l),shadowed=(l.flags.y&LOOK_FOG_SHADOWED)!=0;float g=l.fog.w;float3 sun=f.sunDirection.xyz;
    // Local lights scatter per pixel when the near volume runs (look-fog-lights.slang).
    uint lights=shadowed&&!fogLightsOn(l)?f.extent.z:0;float cosine=dot(d,sun);
    float begin=(l.flags.y&LOOK_VOLUMETRIC)!=0?l.volumetric.x/max(dot(d,f.forward.xyz),0.05):0;
    // Threads past the last slice only join the barrier.
    uint slice=min(z,FOG_SLICES-1);
    float previous=slice==0?0:froxelEnd(slice-1);
    float end=froxelEnd(slice),start=max(previous,begin);
    float3 light=0,trans=1;
    if(z<FOG_SLICES && end>start){
        float ds=end-start;float3 at=f.camera.xyz+d*(start+ds*0.5);
        FogMedium m=fogMedium(f,l,at);float3 e=m.extinction;
        float lit=1;
        if(shadowed){lit=0;for(uint i=0;i<4;i++)lit+=fogVisibility(f,l,f.camera.xyz+d*(start+ds*(float(i)+0.5)/4));lit*=0.25;}
        // The height fog's sun and sky light (Unreal's height fog contribution).
        float fogLight=physical?l.sky.skyFactor.w:1;
        float3 source=sunAt(f,l,at)*lit*m.scattering*(phaseM(cosine,g)*l.volumetricEmissive.w*fogLight);
        if(physical){
            AtmosphereSegment air=atmosphereAerial(l.sky,l.sampled0.x,l.sampled0.y,f.camera.xyz,d,start,end,at);
            source+=air.sun*lit*cloudShadow(f,l,at)+air.source;
            e+=air.extinction;
        }
        if(any(m.scattering>0)){
            source+=m.scattering*fogAmbient(f,l,at)*fogLight;
            for(uint i=0;i<lights;i++)source+=m.scattering*fogLocalLight(f,l,loadLight(f.lighting.w,i),i,at,d,g);
        }
        source+=m.emission;
        light=source*segmentWeight(e,ds);trans=exp(-e*ds);
    }
    if(z<FOG_SLICES){fogSegmentLight[z]=light;fogSegmentTrans[z]=trans;}
    GroupMemoryBarrierWithGroupSync();
    if(z>=FOG_SLICES)return;
    float3 integral=0,through=1;
    for(uint i=0;i<=z;i++){integral+=through*fogSegmentLight[i];through*=fogSegmentTrans[i];}
    store(l.storage0.w,uint2(z*128+group.x,group.y),integral);
    store(l.storage0.w,uint2(z*128+64+group.x,group.y),through);
}
