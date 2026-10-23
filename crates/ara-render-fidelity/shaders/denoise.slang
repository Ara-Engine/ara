#define RASTER 1
#include "common.slangh"
float luminance(float3 c){return dot(c,float3(0.2126,0.7152,0.0722));}
[shader("compute")][numthreads(8,8,1)]
void temporalMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint slot=f.control.x,previous=1-slot;
    float3 colour=pixel(f,RAW,p).rgb;float count=1;float l=luminance(colour);float2 moments=float2(l,l*l);
    float4 world=pixel(f,WORLD+slot,p),normal=pixel(f,NORMAL+slot,p),motion=pixel(f,MOTION,p);
    float2 uv=(float2(p)+0.5)/float2(f.sizes.xy)+motion.xy+(f.jitter.zw-f.jitter.xy)/float2(f.sizes.xy);
    if(f.temporal.x==0 && f.transport.x==1){
        float weight=float(f.transport.y)/max(f.transport.w+f.transport.y,1);colour=lerp(pixel(f,HISTORY+previous,p).rgb,colour,weight);count=f.transport.w+f.transport.y;
    }else if(f.transport.x==0 && f.temporal.x==0 && f.temporal.y==0 && world.w>0 && all(uv>0) && all(uv<1)){
        int2 at=int2(uv*f.sizes.xy);float4 oldWorld=pixel(f,WORLD+previous,at),oldNormal=pixel(f,NORMAL+previous,at);
        float depth=length(oldWorld.xyz-f.previousCamera.xyz);float threshold=max(0.015,motion.z*0.01);
        bool valid=oldWorld.w==world.w && abs(depth-motion.z)<threshold && dot(normal.xyz,oldNormal.xyz)>0.92 && abs(normal.w-oldNormal.w)<0.1;
        if(valid){
            float3 low=colour,high=colour,mean=0,second=0;
            for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){float3 c=pixel(f,RAW,int2(p)+int2(x,y)).rgb;low=min(low,c);high=max(high,c);mean+=c;second+=c*c;}
            mean/=9;float3 sigma=sqrt(max(second/9-mean*mean,0));low=max(low,mean-2*sigma);high=min(high,mean+2*sigma);
            float4 history=sampleImage(f,HISTORY+previous,uv);count=min(history.w+1,24);float weight=max(1/count,motion.w>0?0.35:0.08);
            float3 clipped=clamp(history.rgb,low,high);float oldL=luminance(clipped);
            // An illumination discontinuity cannot borrow long-lived radiance history.
            if(abs(l-oldL)>max(l,0.05)*0.5)weight=max(weight,0.5);
            colour=lerp(clipped,colour,weight);moments=lerp(sampleImage(f,MOMENTS+previous,uv).xy,moments,weight);
        }
    }
    if(f.transport.x==1){l=luminance(colour);moments=float2(l,l*l);}
    float variance=max(moments.y-moments.x*moments.x,0);
    store(f,HISTORY+slot,p,float4(colour,count));store(f,MOMENTS+slot,p,float4(moments,variance,count));
}
// Full 5x5 B-spline kernel. Stage radiance and normal/depth in shared memory,
// reducing each pass from 75 global vec4 loads/pixel to a cooperative halo load.
groupshared float4 filterColour[576];
groupshared float4 filterGuide[576];
[shader("compute")][numthreads(8,8,1)]
void atrousMain(uint3 tid:SV_DispatchThreadID,uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint2 p=tid.xy;uint slot=f.control.x;
    if(f.transport.x==1){if(all(p<f.sizes.xy))store(f,FILTER0,p,float4(pixel(f,HISTORY+slot,p).rgb,1));return;}
    uint source=root.pass==0?HISTORY+slot:root.pass==1?FILTER0:FILTER1;uint target=root.pass==1?FILTER1:FILTER0;
    int step=1<<root.pass;uint width=8+4*step;
    for(uint i=lane;i<width*width;i+=64){int2 at=int2(group.xy*8)+int2(i%width,i/width)-2*step;filterColour[i]=pixel(f,source,at);filterGuide[i]=pixel(f,BASE,at);}
    GroupMemoryBarrierWithGroupSync();
    if(any(p>=f.sizes.xy))return;
    uint2 local=tid.xy-group.xy*8+2*step;uint centerIndex=local.y*width+local.x;
    float3 center=filterColour[centerIndex].rgb;float4 guide=filterGuide[centerIndex];float3 sum=0;float total=0;
    float sigma=sqrt(max(pixel(f,MOMENTS+slot,p).z,0.00001));float l=luminance(center);
    float scale=max(guide.w*0.002,0.005)*step;
    for(int y=-2;y<=2;y++)for(int x=-2;x<=2;x++){
        uint at=(int(local.y)+y*step)*width+int(local.x)+x*step;
        float4 g=filterGuide[at];float3 c=filterColour[at].rgb;
        float weight=(x==0?6:abs(x)==1?4:1)*(y==0?6:abs(y)==1?4:1);
        weight*=exp(-abs(g.w-guide.w)/scale)*pow(max(dot(g.xyz,guide.xyz),0),64)*exp(-abs(luminance(c)-l)/(4*sigma+0.001));
        if(x==0 && y==0)weight=max(weight,1);sum+=c*weight;total+=weight;
    }store(f,target,p,float4(sum/max(total,1e-6),1));
}
