#define RASTER 1
#include "common.slangh"
#ifdef SPLIT_SIGNALS
static const bool splitSignals=true;
#else
static const bool splitSignals=false;
#endif
float luminance(float3 c){return dot(c,float3(0.2126,0.7152,0.0722));}
static const uint SPEC_RAW=28,SPEC_HISTORY=29,SPEC_MOMENTS=31,SPEC_FILTER0=33,SPEC_FILTER1=34;
float3 historyBox(Frame f,uint2 p,float3 history,uint signal){
    float3 mean=0,square=0;float count=0;float4 center=pixel(f,WORLD+f.control.x,p);
    for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
        int2 q=int2(p)+int2(x,y);float4 value=signal<28?pixel(f,signal,q):extraPixel(f,signal,q);
        if(pixel(f,WORLD+f.control.x,q).w!=center.w || (signal<28 && value.w<=0))continue;
        if(signal>=28)value.rgb*=value.w;
        mean+=value.rgb;square+=value.rgb*value.rgb;count+=1;
    }
    if(count<2)return history;mean/=count;float3 sigma=sqrt(max(square/count-mean*mean,float3(0)));
    return clamp(history,max(0,mean-3*sigma-0.1),mean+3*sigma+0.1);
}
void resolveSpecular(Frame f,uint2 p,uint directResidual){
    uint slot=f.control.x;float4 current=extraPixel(f,SPEC_RAW,p);float3 colour=current.rgb*current.w;float count=1;
    float l=luminance(colour),mixed=0;float2 moments=float2(l,l*l);float2 uv;int2 at;
    bool valid=reproject(f,p,uv,at);
    if(valid && f.transport.w!=1 && mixedCoverage(extraPixel(f,SPEC_MOMENTS+1-slot,at).z))valid=false;
    if(valid){
        float4 old=extraPixel(f,SPEC_HISTORY+1-slot,at);float rough=pixel(f,NORMAL+slot,p).w;
        float cap=f.transport.w==1?256.:lerp(8.,48.,smoothstep(0.1,0.5,rough));count=min(old.w+1,cap);
        // Inverse-probability samples include explicit zeros on skipped frames.
        // A ratio of sampled counts changes material coverage at mixed-rate edges.
        float alpha=max(1/max(count,1),f.temporal.y>0?0.5:f.transport.w==1?1./256.:0.025);
        float4 oldMoments=extraPixel(f,SPEC_MOMENTS+1-slot,at);
        if(f.transport.w==1){float oldRough=pixel(f,NORMAL+1-slot,at).w;mixed=float(mixedCoverage(oldMoments.z) || pixel(f,WORLD+slot,p).w!=pixel(f,WORLD+1-slot,at).w || ((rough<0.5)!=(oldRough<0.5)));}
        float3 previous=historyColour(old.rgb,historyPayload(oldMoments.w),10);
        if(length(pixel(f,MOTION,p).xy*f.sizes.xy)>0.5 || f.temporal.y>0)previous=historyBox(f,p,previous,SPEC_RAW);
        colour=lerp(previous,colour,alpha);moments=lerp(oldMoments.xy,moments,alpha);
    }
    float3 rounded;uint residual=roundHistory(colour,10,rounded);
    extraStore(f,SPEC_HISTORY+slot,p,float4(rounded,count));extraStore(f,SPEC_MOMENTS+slot,p,float4(moments,historyBits(directResidual|(uint(mixed)<<27)),historyBits(residual)));
}
[shader("compute")][numthreads(8,8,1)]
void temporalMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint slot=f.control.x,previous=1-slot;
    float directCount=1;float3 modulation=demodulation(f,p);float3 directLight=f.transport.x==0?pixel(f,DIRECT,p).rgb:float3(0);float3 colour=pixel(f,RAW,p).rgb;float count=1;float l=luminance(colour);float2 moments=float2(l,l*l);
    float4 world=pixel(f,WORLD+slot,p),normal=pixel(f,NORMAL+slot,p),motion=pixel(f,MOTION,p);
    float2 uv=(float2(p)+0.5)/float2(f.sizes.xy)+motion.xy+(f.jitter.zw-f.jitter.xy)/float2(f.sizes.xy);
    if(f.temporal.x==0 && f.transport.x==1){
        float weight=float(f.transport.y)/max(f.transport.w+f.transport.y,1);colour=lerp(pixel(f,HISTORY+previous,p).rgb,colour,weight);count=f.transport.w+f.transport.y;
    }else if(f.transport.x==0){
        int2 at;bool valid=reproject(f,p,uv,at);
        if(valid && splitSignals && f.transport.w!=1 && mixedCoverage(extraPixel(f,SPEC_MOMENTS+previous,at).z))valid=false;
        if(valid){
            // Bilinear taps must all belong to the same surface. Otherwise use the
            // validated nearest sample; this removes dark foreground/background rims.
            // Stationary pixel-area accumulation addresses the exact old texel;
            // neither a four-neighbor footprint test nor bilinear sampling is needed.
            int2 low=int2(floor(uv*f.sizes.xy-0.5));bool footprint=f.transport.w!=1 && !splitSignals;
            if(footprint)for(int y=0;y<2;y++)for(int x=0;x<2;x++)footprint=footprint && pixel(f,WORLD+previous,low+int2(x,y)).w==world.w;
            float4 history=footprint?sampleImage(f,HISTORY+previous,uv):pixel(f,HISTORY+previous,at);
            if(splitSignals)history.rgb=historyColour(history.rgb,historyPayload(pixel(f,MOMENTS+previous,at).w),10);
            bool sampled=pixel(f,RAW,p).w>0;float cap=splitSignals && f.transport.w==1?256.:64.;count=min(history.w+float(sampled),length(motion.xy*f.sizes.xy)>1?4.:cap);float weight=sampled?max(1/count,motion.w>0?0.35:splitSignals && f.transport.w==1?1./256.:0.02):0;
            if(f.temporal.y>0 && sampled){weight=max(weight,0.5);count=min(count,2.);}
            float3 clipped=history.rgb;
            if(splitSignals && (length(motion.xy*f.sizes.xy)>0.5 || f.temporal.y>0))clipped=historyBox(f,p,clipped,RAW);
            // Illumination changes are host-signalled; stochastic highlights are not resets.
            float4 oldDirect=footprint?sampleImage(f,DIRECT_HISTORY+previous,uv):pixel(f,DIRECT_HISTORY+previous,at);
            if(splitSignals)oldDirect.rgb=historyColour(oldDirect.rgb,historyPayload(extraPixel(f,SPEC_MOMENTS+previous,at).z)&((1u<<27)-1),9);
            directCount=min(oldDirect.w+1,length(motion.xy*f.sizes.xy)>1?4.:splitSignals && f.transport.w==1?256.:96.);
            // Pixel-area integration must include the material factor as well as
            // incident radiance. A current-jitter albedo creates stippled floors.
            if(f.transport.w==1){uint packed=asuint(pixel(f,MOMENTS+previous,p).z);float3 oldMod=float3(packed&1023,(packed>>10)&1023,(packed>>20)&1023)/1023.;modulation=lerp(oldMod,modulation,1/min(directCount,32.));}
            directLight=lerp(oldDirect.rgb,directLight,f.temporal.y>0?0.75:1/directCount);
            colour=lerp(clipped,colour,weight);moments=lerp((footprint?sampleImage(f,MOMENTS+previous,uv):pixel(f,MOMENTS+previous,at)).xy,moments,weight);
        }
    }
    if(f.transport.x==1){l=luminance(colour);moments=float2(l,l*l);}
    uint directResidual=0,diffuseResidual=0;
    if(splitSignals){float3 rounded;directResidual=roundHistory(directLight,9,rounded);directLight=rounded;diffuseResidual=roundHistory(colour,10,rounded);colour=rounded;}
    store(f,DIRECT_HISTORY+slot,p,float4(directLight,directCount));
    uint3 q=uint3(saturate(modulation)*1023+0.5);uint packed=q.x|(q.y<<10)|(q.z<<20);
    store(f,HISTORY+slot,p,float4(colour,count));store(f,MOMENTS+slot,p,float4(moments,asfloat(packed),splitSignals?historyBits(diffuseResidual):count));
    if(splitSignals)resolveSpecular(f,p,directResidual);
}
// Three 3x3 wavelet iterations stage incident radiance and geometric guides.
// Plane distance preserves slanted walls; roughness controls normal rejection.
static const uint FILTER_CAPACITY=256;
groupshared float3 filterColour[FILTER_CAPACITY];
groupshared float4 filterGuide[FILTER_CAPACITY];
groupshared float4 filterWorld[FILTER_CAPACITY];
groupshared float3 filterSpecular[FILTER_CAPACITY];
[shader("compute")][numthreads(8,8,1)]
void atrousMain(uint3 tid:SV_DispatchThreadID,uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint2 p=tid.xy;uint slot=f.control.x;
    if(f.transport.x==1){if(all(p<f.sizes.xy))store(f,FILTER0,p,float4(pixel(f,HISTORY+slot,p).rgb+(f.transport.x==0?pixel(f,DIRECT_HISTORY+slot,p).rgb:float3(0)),1));return;}
    uint source=root.pass==0?HISTORY+slot:root.pass==1?FILTER0:FILTER1;uint target=root.pass==1?FILTER1:FILTER0;
    int baseStep=splitSignals?(root.pass==0?1:root.pass==1?3:8):1<<root.pass;
    // Preserve the validated angular footprint at native/upscaled resolutions.
    // A fixed pixel radius hides noise in the 270p metric but leaves native walls mottled.
    int step=splitSignals?max(baseStep,int(float(baseStep)*f.sizes.y/270.+0.5)):baseStep;uint width=8+2*step;
    uint specSource=root.pass==0?SPEC_HISTORY+slot:root.pass==1?SPEC_FILTER0:SPEC_FILTER1;
    // Wide footprints provide little group reuse and exceed the compact tile.
    // Coalesced texture gathers preserve occupancy without a large shared halo.
    bool wide=step>4;
    if(!wide){
        for(uint i=lane;i<width*width;i+=64){int2 at=int2(group.xy*8)+int2(i%width,i/width)-step;filterColour[i]=pixel(f,source,at).rgb;filterWorld[i]=pixel(f,WORLD+slot,at);float4 n=pixel(f,NORMAL+slot,at);filterGuide[i]=float4(n.xyz,length(filterWorld[i].xyz-f.camera.xyz));if(splitSignals)filterSpecular[i]=extraPixel(f,specSource,at).rgb;}
        GroupMemoryBarrierWithGroupSync();
    }
    if(any(p>=f.sizes.xy))return;
    uint2 local=tid.xy-group.xy*8+step;uint centerIndex=local.y*width+local.x;
    float3 center,specCenter=0;float4 guide,world;
    if(wide){world=pixel(f,WORLD+slot,p);center=pixel(f,source,p).rgb;guide=float4(pixel(f,NORMAL+slot,p).xyz,length(world.xyz-f.camera.xyz));if(splitSignals)specCenter=extraPixel(f,specSource,p).rgb;}
    else{world=filterWorld[centerIndex];center=filterColour[centerIndex];guide=filterGuide[centerIndex];if(splitSignals)specCenter=filterSpecular[centerIndex];}
    float3 sum=0;float total=0;
    float2 temporalMoments=pixel(f,MOMENTS+slot,p).xy;float sigma=sqrt(max(temporalMoments.y-temporalMoments.x*temporalMoments.x,0.00001));float l=luminance(center);float historyCount=pixel(f,HISTORY+slot,p).w;
    // Geometry tolerance stays in world units; a larger image must not admit
    // samples through increasingly thick walls when its pixel radius grows.
    float scale=max(guide.w*0.001,0.003)*baseStep;float roughness=pixel(f,NORMAL+slot,p).w;
    float3 specSum=0;float specTotal=0;float specL=splitSignals?luminance(specCenter):0;
    float4 specMoments=splitSignals?extraPixel(f,SPEC_MOMENTS+slot,p):float4(0);float2 specMom=specMoments.xy;float specSigma=sqrt(max(specMom.y-specMom.x*specMom.x,0.00001));
    for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
        float3 n,c,spec=0;float4 neighborWorld;
        if(wide){int2 q=int2(p)+int2(x,y)*step;neighborWorld=pixel(f,WORLD+slot,q);n=pixel(f,NORMAL+slot,q).xyz;c=pixel(f,source,q).rgb;if(splitSignals)spec=extraPixel(f,specSource,q).rgb;}
        else{uint at=(int(local.y)+y*step)*width+int(local.x)+x*step;neighborWorld=filterWorld[at];n=filterGuide[at].xyz;c=filterColour[at];if(splitSignals)spec=filterSpecular[at];}
        float weight=(x==0?2:1)*(y==0?2:1);
        // Rough incident irradiance is smooth: a centre-dependent luminance
        // rejection biases rare bright bounce samples back toward darkness.
        float radianceWeight=historyCount<4?1:lerp(exp(-abs(luminance(c)-l)/(4*sigma+0.1)),1,smoothstep(0.1,0.3,roughness));
        weight*=exp(-abs(dot(neighborWorld.xyz-world.xyz,guide.xyz))/scale)*pow(max(dot(n,guide.xyz),0),splitSignals?lerp(64.,8.,roughness):lerp(128.,32.,roughness))*radianceWeight*float(neighborWorld.w==world.w);
        if(splitSignals && mixedCoverage(specMoments.z) && (x!=0 || y!=0))weight=0;
        if(x==0 && y==0)weight=max(weight,1);sum+=c*weight;total+=weight;
        if(splitSignals){
            float sw=weight*exp(-abs(luminance(spec)-specL)/(4*specSigma+0.02))*pow(max(dot(n,guide.xyz),0),lerp(256.,16.,roughness));
            if(x==0 && y==0)sw=max(sw,1);specSum+=spec*sw;specTotal+=sw;
        }
    }uint packed=asuint(pixel(f,MOMENTS+slot,p).z);float3 modulation=max(float3(packed&1023,(packed>>10)&1023,(packed>>20)&1023)/1023.,float3(0.04));
    float3 spec=specSum/max(specTotal,1e-6);
    if(splitSignals)extraStore(f,root.pass==1?SPEC_FILTER1:SPEC_FILTER0,p,float4(spec,1));
    store(f,target,p,float4(root.pass==2?sum/max(total,1e-6)*modulation+pixel(f,DIRECT_HISTORY+slot,p).rgb+spec:sum/max(total,1e-6),1));
}
