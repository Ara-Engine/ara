#define RASTER 1
#include "common.slangh"
float luminance(float3 c){return dot(c,float3(0.2126,0.7152,0.0722));}
[shader("compute")][numthreads(8,8,1)]
void temporalMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint slot=f.control.x,previous=1-slot;
    float3 directLight=f.transport.x==0?pixel(f,DIRECT,p).rgb:float3(0);float3 colour=pixel(f,RAW,p).rgb;float count=1;float l=luminance(colour);float2 moments=float2(l,l*l);
    float4 world=pixel(f,WORLD+slot,p),normal=pixel(f,NORMAL+slot,p),motion=pixel(f,MOTION,p);
    float2 uv=(float2(p)+0.5)/float2(f.sizes.xy)+motion.xy+(f.jitter.zw-f.jitter.xy)/float2(f.sizes.xy);
    if(f.temporal.x==0 && f.transport.x==1){
        float weight=float(f.transport.y)/max(f.transport.w+f.transport.y,1);colour=lerp(pixel(f,HISTORY+previous,p).rgb,colour,weight);count=f.transport.w+f.transport.y;
    }else if(f.transport.x==0){
        int2 at;bool valid=reproject(f,p,uv,at);
        if(valid){
            // Bilinear taps must all belong to the same surface. Otherwise use the
            // validated nearest sample; this removes dark foreground/background rims.
            int2 low=int2(floor(uv*f.sizes.xy-0.5));bool footprint=true;
            for(int y=0;y<2;y++)for(int x=0;x<2;x++)footprint=footprint && pixel(f,WORLD+previous,low+int2(x,y)).w==world.w;
            float4 history=footprint?sampleImage(f,HISTORY+previous,uv):pixel(f,HISTORY+previous,at);
            bool sampled=pixel(f,RAW,p).w>0;count=min(history.w+float(sampled),length(motion.xy*f.sizes.xy)>1?4.:64.);float weight=sampled?max(1/count,motion.w>0?0.35:0.02):0;
            float3 clipped=history.rgb;
            // Clamp only rare incident-radiance outliers against temporal variance.
            // Direct highlights are a separate signal, so this cannot flatten them.
            float2 oldMoments=(footprint?sampleImage(f,MOMENTS+previous,uv):pixel(f,MOMENTS+previous,at)).xy;
            if(sampled && history.w>8 && motion.w==0){
                float ceiling=oldMoments.x+12*sqrt(max(oldMoments.y-oldMoments.x*oldMoments.x,0))+max(1,oldMoments.x*4);
                if(l>ceiling){colour*=ceiling/max(l,1e-6);l=ceiling;moments=float2(l,l*l);}
            }
            // Illumination changes are host-signalled; stochastic highlights are not resets.
            directLight=lerp((footprint?sampleImage(f,DIRECT_HISTORY+previous,uv):pixel(f,DIRECT_HISTORY+previous,at)).rgb,directLight,max(1/max(count,1),0.05));
            colour=lerp(clipped,colour,weight);moments=lerp((footprint?sampleImage(f,MOMENTS+previous,uv):pixel(f,MOMENTS+previous,at)).xy,moments,weight);
        }
    }
    if(f.transport.x==1){l=luminance(colour);moments=float2(l,l*l);}
    float variance=max(moments.y-moments.x*moments.x,0);
    store(f,DIRECT_HISTORY+slot,p,float4(directLight,1));
    store(f,HISTORY+slot,p,float4(colour,count));store(f,MOMENTS+slot,p,float4(moments,variance,count));
}
// Three 3x3 wavelet iterations stage incident radiance and geometric guides.
// Plane distance preserves slanted walls; roughness controls normal rejection.
groupshared float4 filterColour[256];
groupshared float4 filterGuide[256];
groupshared float4 filterWorld[256];
[shader("compute")][numthreads(8,8,1)]
void atrousMain(uint3 tid:SV_DispatchThreadID,uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint2 p=tid.xy;uint slot=f.control.x;
    if(f.transport.x==1){if(all(p<f.sizes.xy))store(f,FILTER0,p,float4(pixel(f,HISTORY+slot,p).rgb+(f.transport.x==0?pixel(f,DIRECT_HISTORY+slot,p).rgb:float3(0)),1));return;}
    uint source=root.pass==0?HISTORY+slot:root.pass==1?FILTER0:FILTER1;uint target=root.pass==1?FILTER1:FILTER0;
    int step=1<<root.pass;uint width=8+2*step;
    for(uint i=lane;i<width*width;i+=64){int2 at=int2(group.xy*8)+int2(i%width,i/width)-step;filterColour[i]=pixel(f,source,at);filterWorld[i]=pixel(f,WORLD+slot,at);float4 n=pixel(f,NORMAL+slot,at);filterGuide[i]=float4(n.xyz,length(filterWorld[i].xyz-f.camera.xyz));}
    GroupMemoryBarrierWithGroupSync();
    if(any(p>=f.sizes.xy))return;
    uint2 local=tid.xy-group.xy*8+step;uint centerIndex=local.y*width+local.x;
    float3 center=filterColour[centerIndex].rgb;float4 guide=filterGuide[centerIndex];float3 sum=0;float total=0;
    float sigma=sqrt(max(pixel(f,MOMENTS+slot,p).z,0.00001));float l=luminance(center);float historyCount=pixel(f,HISTORY+slot,p).w;
    float scale=max(guide.w*0.001,0.003)*step;float roughness=pixel(f,NORMAL+slot,p).w;float4 world=filterWorld[centerIndex];
    for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
        uint at=(int(local.y)+y*step)*width+int(local.x)+x*step;
        float4 g=filterGuide[at];float3 c=filterColour[at].rgb;
        float weight=(x==0?2:1)*(y==0?2:1);
        weight*=exp(-abs(dot(filterWorld[at].xyz-world.xyz,guide.xyz))/scale)*pow(max(dot(g.xyz,guide.xyz),0),lerp(128.,32.,roughness))*(historyCount<4?1:exp(-abs(luminance(c)-l)/(4*sigma+0.1)))*float(filterWorld[at].w==world.w);
        if(x==0 && y==0)weight=max(weight,1);sum+=c*weight;total+=weight;
    }store(f,target,p,float4(sum/max(total,1e-6)+(root.pass==2?pixel(f,DIRECT_HISTORY+slot,p).rgb:float3(0)),1));
}
