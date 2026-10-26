// Heightfield horizon maps (look/horizon.rs): the sun's visible share and the sky
// visibility of each capture texel. Root word 0 is the frame record.
#include "raster.slangh"
#include "look.slangh"
#include "look-horizon.slangh"

// The sun map: at each capture texel, x the sun's visible share (the steepest rise
// towards the sun's azimuth against the sun's elevation, softened over the sun disk
// and never sharper than a quarter degree, the heightfield's angular precision far
// off) and y the top surface height (-1e4 where nothing was captured), so shading
// needs one fetch.
[shader("compute")][numthreads(8,8,1)]
void horizonSunMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint res=l.horizonImages.w;if(any(tid.xy>=res))return;
    float2 uv=(float2(tid.xy)+0.5)/res;float h=horizonTop(l,uv);
    float3 sun=normalize(f.sunDirection.xyz);float flat=length(sun.xz),visible=1;
    if(sun.y<=0)visible=0;
    else if(h>-1e29 && flat>1e-4){
        float elevation=sun.y/flat,rise=horizonRise(l,uv,h,sun.xz/flat,1.07);
        float soft=max(l.sunLux.w,0.0044)*(1+elevation*elevation);
        visible=saturate((elevation-rise)/(2*soft)+0.5);
    }
    images[NonUniformResourceIndex(l.horizonStorage.x)][tid.xy]=float4(visible,max(h,-1e4),0,0);
}

// Cosine-weighted sky visibility of an upward surface at each sky texel: the mean
// over twelve azimuths of cos^2 of the horizon's elevation there.
[shader("compute")][numthreads(8,8,1)]
void horizonSkyMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint res=l.horizonStorage.z;if(any(tid.xy>=res))return;
    float2 uv=(float2(tid.xy)+0.5)/res;float h=horizonTop(l,uv),visible=1;
    if(h>-1e29){
        float sum=0;
        for(uint a=0;a<12;a++){
            float angle=(a+0.5)*(2*PI/12);
            float rise=max(horizonRise(l,uv,h,float2(cos(angle),sin(angle)),1.15),0);
            sum+=1/(1+rise*rise);
        }
        visible=sum/12;
    }
    images[NonUniformResourceIndex(l.horizonStorage.y)][tid.xy]=float4(visible,0,0,0);
}
