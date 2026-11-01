// Physical sky LUTs, the sky's image-based lighting and froxel fog (look/passes.rs).
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "look-fog.slangh"
void store(uint image,uint2 p,float3 value){images[NonUniformResourceIndex(image)][p]=float4(value,1);}
// 256x64: transmittance to the top of the atmosphere by altitude and sun cosine.
[shader("compute")][numthreads(8,8,1)]
void transmittanceMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=uint2(256,64)))return;
    float2 uv=(float2(tid.xy)+0.5)/float2(256,64);float height=uv.y*uv.y*100000,mu=uv.x*2-1;
    float3 p=float3(0,PLANET+height,0),d=float3(sqrt(max(0,1-mu*mu)),mu,0);float extent=atmosphereLength(p,d);float3 optical=0;
    for(uint i=0;i<64;i++){float3 sr;float sm;optical+=extinction(max(0,length(p+d*(extent*(i+0.5)/64))-PLANET),l.look.w,sr,sm)*(extent/64);}
    float3 t=exp(-optical);if(mu<0 && extent<sphereExit(p,d,TOP)-1)t=0;store(l.storage0.x,tid.xy,t);
}
// 32x32: isotropic multiple-scattering transfer (Hillaire's Psi_ms), per lux.
[shader("compute")][numthreads(8,8,1)]
void multiScatterMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=uint2(32,32)))return;float2 uv=(float2(tid.xy)+0.5)/32;float h=uv.y*uv.y*100000,mu=uv.x*2-1;
    float3 sun=float3(sqrt(max(0,1-mu*mu)),mu,0),incoming=0,feedback=0,p=float3(0,PLANET+h+1,0);
    for(uint j=0;j<16;j++){
        float y=1-2*(j+0.5)/16,phi=j*2.399963;float3 d=float3(sqrt(1-y*y)*cos(phi),y,sqrt(1-y*y)*sin(phi));float extent=atmosphereLength(p,d);float3 t;
        incoming+=integrateSky(l,sun,p,d,extent,16,false,t);
        // Unit isotropic radiance scattered along the path, excluding absorption.
        float3 path=1,scatter=0;float ds=extent/16;
        for(uint i=0;i<16;i++){float3 sr;float sm;float3 e=extinction(max(0,length(p+d*(ds*(i+0.5)))-PLANET),l.look.w,sr,sm),segment=exp(-e*ds);scatter+=path*(sr+sm)*(1-segment)/max(e,float3(1e-9));path*=segment;}
        feedback+=scatter;
    }
    store(l.storage0.y,tid.xy,(incoming/16)/max(1-feedback/16,float3(0.05)));
}
// 192x108 equirectangular sky radiance seen from the camera altitude, with a
// 15% albedo ground below the horizon.
[shader("compute")][numthreads(8,8,1)]
void skyViewMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=uint2(192,108)))return;
    float3 d=environmentDirection((float2(tid.xy)+0.5)/float2(192,108)),sun=f.sunDirection.xyz,p=float3(0,PLANET+max(f.camera.y-l.atmosphere.x,2),0),t;
    float extent=atmosphereLength(p,d);float3 light=integrateSky(l,sun,p,d,extent,32,true,t);
    if(extent<sphereExit(p,d,TOP)-1){float3 n=normalize(p+d*extent);float mu=dot(n,sun);light+=t*0.15*(transmittance(l,0,mu)*max(mu,0)+multiScattering(l,0,mu)*PI)/PI;}
    store(l.storage0.z,tid.xy,light*l.sunLux.rgb);
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
// One group per frustum column, one thread per logarithmic slice to FOG_RANGE
// (look.slangh sampleFroxel): each evaluates its segment, then accumulates the
// segments in front of it in order (front to back, deterministic). On near volume
// frames (LOOK_VOLUMETRIC) the column starts where the near volume ends, so shading
// composes the two exactly (applyFog). Volumetric fog's sun is shadowed by the
// cascades and far map (each segment the lit share of four points, so shafts move
// smoothly with the camera instead of a whole slice flipping lit or dark at once), and
// local lights scatter in it; quadrature is deterministic (no noise). Height fog and
// fog volumes come from fogMedium; clear-air Rayleigh and Mie scattering join them
// under the physical sky.
groupshared float3 fogSegmentLight[32],fogSegmentTrans[32];
[shader("compute")][numthreads(32,1,1)]
void fogMain(uint3 group:SV_GroupID,uint z:SV_GroupIndex){
    Frame f=frame();Look l=look(f);
    float2 uv=(float2(group.xy)+0.5)/float2(64,36);float3 d=viewRay(f,uv*float2(f.extent.xy));
    bool physical=physicalSky(l),shadowed=(l.flags.y&LOOK_FOG_SHADOWED)!=0;float g=l.fog.w;float3 sun=f.sunDirection.xyz;
    uint lights=shadowed?f.extent.z:0;float cosine=dot(d,sun);
    float begin=(l.flags.y&LOOK_VOLUMETRIC)!=0?l.volumetric.x/max(dot(d,f.forward.xyz),0.05):0;
    float previous=z==0?0:2*(exp2(log2(1+FOG_RANGE/2)*float(z)/32)-1);
    float end=2*(exp2(log2(1+FOG_RANGE/2)*float(z+1)/32)-1),start=max(previous,begin);
    float3 light=0,trans=1;
    if(end>start){
        float ds=end-start;float3 at=f.camera.xyz+d*(start+ds*0.5);
        float height=max(at.y-l.atmosphere.x,0);FogMedium m=fogMedium(f,l,at);
        float3 sr=0;float sm=0;float3 e=m.extinction;
        if(physical)e+=extinction(height,l.look.w,sr,sm);
        float lit=1;
        if(shadowed){lit=0;for(uint i=0;i<4;i++)lit+=fogVisibility(f,l,f.camera.xyz+d*(start+ds*(float(i)+0.5)/4));lit*=0.25;}
        float3 source=sunAt(f,l,at)*lit*(sr*phaseR(cosine)+sm*phaseM(cosine,0.8)+m.scattering*(phaseM(cosine,g)*l.volumetricEmissive.w));
        if(physical)source+=(sr+sm)*multiScattering(l,height,sun.y)*l.sunLux.rgb;
        if(any(m.scattering>0)){
            source+=m.scattering*fogAmbient(f,l,at);
            for(uint i=0;i<lights;i++)source+=m.scattering*fogLocalLight(f,l,loadLight(f.lighting.w,i),i,at,d,g);
        }
        source+=m.emission;
        light=source*segmentWeight(e,ds);trans=exp(-e*ds);
    }
    fogSegmentLight[z]=light;fogSegmentTrans[z]=trans;
    GroupMemoryBarrierWithGroupSync();
    float3 integral=0,through=1;
    for(uint i=0;i<=z;i++){integral+=through*fogSegmentLight[i];through*=fogSegmentTrans[i];}
    store(l.storage0.w,uint2(z*128+group.x,group.y),integral);
    store(l.storage0.w,uint2(z*128+64+group.x,group.y),through);
}
