#define RASTER 1
#include "common.slangh"
[shader("compute")][numthreads(8,8,1)]
void transmittanceMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();if(any(tid.xy>=uint2(256,64)))return;
    float2 uv=(float2(tid.xy)+0.5)/float2(256,64);float height=uv.y*uv.y*100000,mu=uv.x*2-1;
    float3 p=float3(0,PLANET+height,0),d=float3(sqrt(max(0,1-mu*mu)),mu,0);float extent=atmosphereLength(p,d),tau=0;float3 optical=0;
    for(uint i=0;i<64;i++){float3 sr;float sm;optical+=extinction(max(0,sqrt(dot(p+d*(extent*(i+0.5)/64),p+d*(extent*(i+0.5)/64)))-PLANET),sr,sm)*(extent/64);}
    float3 t=exp(-optical);if(mu<0 && extent<sphereExit(p,d,TOP)-1)t=0;store(f,TRANSMITTANCE,tid.xy,float4(t,1));
}
[shader("compute")][numthreads(8,8,1)]
void multiScatterMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();if(any(tid.xy>=uint2(32,32)))return;float2 uv=(float2(tid.xy)+0.5)/32;float h=uv.y*uv.y*100000,mu=uv.x*2-1;
    Frame local=f;local.sunDirection.xyz=float3(sqrt(max(0,1-mu*mu)),mu,0);local.sunLux.rgb=1;
    float3 incoming=0,feedback=0,p=float3(0,PLANET+h+1,0);
    for(uint j=0;j<16;j++){
        float y=1-2*(j+0.5)/16,phi=j*2.399963;float3 d=float3(sqrt(1-y*y)*cos(phi),y,sqrt(1-y*y)*sin(phi));float extent=atmosphereLength(p,d);float3 t;
        incoming+=integrateSky(local,p,d,extent,16,false,t);
        // Integrate unit isotropic radiance scattering, excluding absorption.
        float3 path=1,scatter=0;float ds=extent/16;
        for(uint i=0;i<16;i++){float3 sr;float sm;float3 e=extinction(max(0,length(p+d*(ds*(i+0.5)))-PLANET),sr,sm),segment=exp(-e*ds);scatter+=path*(sr+sm)*(1-segment)/max(e,float3(1e-9));path*=segment;}
        feedback+=scatter;
    }
    store(f,MULTISCATTER,tid.xy,float4((incoming/16)/max(1-feedback/16,float3(0.05)),1));
}
[shader("compute")][numthreads(8,8,1)]
void skyViewMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();if(any(tid.xy>=uint2(192,108)))return;float2 uv=(float2(tid.xy)+0.5)/float2(192,108);float phi=(uv.x-0.5)*2*PI,theta=uv.y*PI;
    float3 d=float3(cos(phi)*sin(theta),cos(theta),sin(phi)*sin(theta)),p=float3(0,PLANET+max(f.camera.y-f.atmosphere.x,2),0),t;
    float extent=atmosphereLength(p,d);float3 light=integrateSky(f,p,d,extent,32,true,t);
    if(extent<sphereExit(p,d,TOP)-1){float3 n=normalize(p+d*extent);float mu=dot(n,f.sunDirection.xyz);light+=t*f.sunLux.rgb*0.15*(transmittance(f,0,mu)*max(mu,0)+multiScattering(f,0,mu)*PI)/PI;}
    store(f,SKYVIEW,tid.xy,float4(light,1));
}
