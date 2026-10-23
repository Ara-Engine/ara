#define RASTER 1
#include "common.slangh"
#define A_GPU 1
#define A_HLSL 1
#include "fsr/ffx_a.h"
#define FSR_EASU_F 1
#define FSR_RCAS_F 1
AF4 FsrEasuRF(AF2 p){Frame f=frame();return textures[NonUniformResourceIndex(ri(f,POST))].GatherRed(samplers[2],p);}
AF4 FsrEasuGF(AF2 p){Frame f=frame();return textures[NonUniformResourceIndex(ri(f,POST))].GatherGreen(samplers[2],p);}
AF4 FsrEasuBF(AF2 p){Frame f=frame();return textures[NonUniformResourceIndex(ri(f,POST))].GatherBlue(samplers[2],p);}
AF4 FsrRcasLoadF(ASU2 p){Frame f=frame();return textures[NonUniformResourceIndex(ri(f,UPSCALED))].Load(int3(clamp(p,int2(0),int2(f.sizes.zw)-1),0));}
void FsrRcasInputF(inout AF1 r,inout AF1 g,inout AF1 b){}
#include "fsr/ffx_fsr1.h"
[shader("compute")][numthreads(8,8,1)]
void upscaleMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.zw))return;float3 colour;
    if(root.pass==1){uint4 a,b,c,d;FsrEasuCon(a,b,c,d,f.sizes.x,f.sizes.y,f.sizes.x,f.sizes.y,f.sizes.z,f.sizes.w);FsrEasuF(colour,p,a,b,c,d);}
    else {float2 uv=(float2(p)+0.5)/float2(f.sizes.zw);colour=sampleImage(f,POST,uv).rgb;}
    float2 exposureState=buffers[NonUniformResourceIndex(f.control.y)].Load<float2>(0);
    bool integrated=root.pass==0 && f.transport.x==0 && f.temporal.x==0 && f.temporal.y==0 && f.transport.w==1 && all(f.sizes.xy==f.sizes.zw) && abs(log2(max(exposureState.x,1e-6)/max(exposureState.y,1e-6)))<0.001;
    if(integrated){
        // Pixel coverage, direct light and remodulation already integrate their
        // area. An unchanged camera/scene needs no reprojection or 3x3 clipping.
        if(pixel(f,WORLD+f.control.x,p).w>0)colour=lerp(pixel(f,19+1-f.control.x,p).rgb,colour,0.15);
    }else if(root.pass==0 && f.transport.x==0 && f.temporal.x==0 && f.temporal.y==0){
        float2 uv=(float2(p)+0.5)/float2(f.sizes.zw);float4 motion=sampleImage(f,MOTION,uv);
        // Stationary transport already integrates pixel-area samples. Reapplying
        // jitter reprojection here blurs that integration a second time.
        float2 oldUv=f.transport.w==1?uv:uv+motion.xy+(f.jitter.zw-f.jitter.xy)/float2(f.sizes.xy);
        float4 world=sampleImage(f,WORLD+f.control.x,uv);
        if(all(oldUv>0) && all(oldUv<1) && world.w>0){
            int2 oldPixel=int2(oldUv*f.sizes.xy);float4 oldWorld=pixel(f,WORLD+1-f.control.x,oldPixel);
            float previousDepth=length(oldWorld.xyz-f.previousCamera.xyz);
            float2 exposure=buffers[NonUniformResourceIndex(f.control.y)].Load<float2>(0);
            bool valid=(f.transport.w==1 || (abs(oldWorld.w-world.w)<0.01 && abs(previousDepth-motion.z)<max(0.02,motion.z*0.015))) && abs(log2(max(exposure.x,1e-6)/max(exposure.y,1e-6)))<0.1;
            if(valid){float3 low=colour,high=colour;int2 at=int2(uv*f.sizes.xy);for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){float3 c=pixel(f,POST,at+int2(x,y)).rgb;low=min(low,c);high=max(high,c);}
                float3 previous=clamp(sampleImage(f,19+1-f.control.x,oldUv).rgb,low,high);colour=lerp(previous,colour,max(motion.w>0?0.5:0.15,saturate(length(motion.xy)*32)));
            }
        }
    }
    store(f,19+f.control.x,p,float4(colour,1));store(f,UPSCALED,p,float4(colour,1));
}
[shader("compute")][numthreads(8,8,1)]
void sharpenMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.zw))return;float3 colour;
    if(root.pass==1){uint4 constants=uint4(asuint(exp2(-0.5)),0,0,0);FsrRcasF(colour.r,colour.g,colour.b,p,constants);}
    else colour=textures[NonUniformResourceIndex(ri(f,UPSCALED))].Load(int3(p,0)).rgb;
    store(f,OUTPUT,p,float4(saturate(colour),1));
}
