#define RASTER 1
#include "common.slangh"
// The shared atmosphere's LUTs (atmosphere-model.slangh): transmittance 256x64,
// multiple scattering 32x32, sky view 192x108 at the camera's altitude.
[shader("compute")][numthreads(8,8,1)]
void transmittanceMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();if(any(tid.xy>=ATMOSPHERE_TRANSMITTANCE))return;
    store(f,TRANSMITTANCE,tid.xy,float4(atmosphereTransmittanceTexelValue(atmosphereModel(),(float2(tid.xy)+0.5)/float2(ATMOSPHERE_TRANSMITTANCE)),1));
}
[shader("compute")][numthreads(8,8,1)]
void multiScatterMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();if(any(tid.xy>=ATMOSPHERE_MULTISCATTER))return;
    store(f,MULTISCATTER,tid.xy,float4(atmosphereMultiScatterTexelValue(atmosphereModel(),ri(f,TRANSMITTANCE),(float2(tid.xy)+0.5)/float2(ATMOSPHERE_MULTISCATTER)),1));
}
[shader("compute")][numthreads(8,8,1)]
void skyViewMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();if(any(tid.xy>=ATMOSPHERE_SKY_VIEW))return;AtmosphereModel a=atmosphereModel();
    float r=atmosphereCamera(a,f.camera.xyz).y;
    store(f,SKYVIEW,tid.xy,float4(atmosphereSkyViewTexelValue(a,ri(f,TRANSMITTANCE),ri(f,MULTISCATTER),r,(float2(tid.xy)+0.5)/float2(ATMOSPHERE_SKY_VIEW)),1));
}
