// Exposure metering, bloom and display grading of the anti-aliased HDR scene
// (look/passes.rs). Root word 1 (`root.materials`) is the scene's sampled index;
// root word 7 bit 0 marks it as the pre-exposed TAA history rather than linear HDR.
#include "raster.slangh"
#include "look.slangh"
// Divides the input back to linear HDR; `stored` once exposureMain has run this frame.
float inputScale(Frame f,Look l,bool stored){
    if((root.flags&1)==0)return 1;float4 s=exposureState(l);return exposureScale(f,stored?s.y:s.x);
}
// Fixed 128x72 sampling grid, 256 log-luminance bins over [-12,20] stops, counted
// after the exposure state (look.slangh EXPOSURE_BINS). Pass 0 (root word 2; 36
// groups, one grid sample per thread) bins in groupshared memory and adds each
// group's counts to the bins; pass 1 (one group) meters their trimmed mean with a
// parallel prefix sum, zeroes them for the next frame and writes the state
// (applied multiplier, previous multiplier, EV100, metered luminance).
groupshared uint histogram[256];
groupshared float2 reduce[256];
[shader("compute")][numthreads(256,1,1)]
void exposureMain(uint lane:SV_GroupIndex,uint3 group:SV_GroupID){
    Frame f=frame();Look l=look(f);bool automatic=l.exposure.x<=0;uint state=l.state.x;
    if(root.material==0){
        if(!automatic)return;
        histogram[lane]=0;GroupMemoryBarrierWithGroupSync();
        uint i=group.x*256+lane;float3 c=historyTex(root.materials,(float2(i%128,i/128)+0.5)/float2(128,72)).rgb/inputScale(f,l,false);
        float logL=clamp(log2(max(luminance(c),exp2(-12.))),-12.,20.);InterlockedAdd(histogram[min(uint((logL+12)*8),255)],1);
        GroupMemoryBarrierWithGroupSync();
        uint count=histogram[lane];if(count>0)buffers[NonUniformResourceIndex(state)].InterlockedAdd(EXPOSURE_BINS+lane*4,count);
        return;
    }
    float target=l.exposure.x,metered=0,ev=0;
    if(automatic){
        float count=float(buffers[NonUniformResourceIndex(state)].Load(EXPOSURE_BINS+lane*4));buffers[NonUniformResourceIndex(state)].Store(EXPOSURE_BINS+lane*4,0);
        // Inclusive prefix sum of the counts (Hillis-Steele), then this bin's share
        // of the trimmed [low, high) sample range, summed by a tree reduction.
        reduce[lane]=float2(count,0);GroupMemoryBarrierWithGroupSync();
        for(uint step=1;step<256;step*=2){float v=lane>=step?reduce[lane-step].x:0;GroupMemoryBarrierWithGroupSync();reduce[lane].x+=v;GroupMemoryBarrierWithGroupSync();}
        float prefix=reduce[lane].x-count,low=l.metering.z*9216,high=l.metering.w*9216;
        float weight=max(0,min(prefix+count,high)-max(prefix,low));GroupMemoryBarrierWithGroupSync();
        reduce[lane]=float2((-12+(lane+0.5)/8)*weight,weight);GroupMemoryBarrierWithGroupSync();
        for(uint stride=128;stride>0;stride/=2){if(lane<stride)reduce[lane]+=reduce[lane+stride];GroupMemoryBarrierWithGroupSync();}
        metered=exp2(reduce[0].x/max(reduce[0].y,1));ev=clamp(log2(metered/(0.18*1.2)),l.metering.x,l.metering.y);target=l.exposure.y/(1.2*exp2(ev));
    }else ev=log2(1/max(1.2*target,1e-12));
    if(lane!=0)return;
    // LOOK_SNAP: the meter applies at once (history reset, or new GI arriving);
    // otherwise exposure adapts from the last frame's (or a host's seed).
    float old=exposureState(l).x;bool reset=!automatic || (l.flags.y&LOOK_SNAP)!=0 || !(old>0);
    float value=reset?target:lerp(old,target,1-exp(-l.exposure.w*l.exposure.z));
    buffers[NonUniformResourceIndex(l.state.x)].Store<float4>(0,float4(value,old,ev,metered));
}
// Five-level bloom pyramid from quarter resolution, packed in the top half of the
// down and up atlases: level k (2..6) is (size>>k) at x offset sum(size>>j, 1<=j<k),
// y 0. Pass 0 is a
// Karis-weighted downsample of the scene, 1..4 tent downsamples, 5..8 tent
// upsamples adding each coarser level back; post samples level 2 as the glow.
static const uint BLOOM_FIRST=2;
uint2 bloomSize(Frame f,uint level){return max(f.extent.xy>>level,uint2(1));}
uint2 bloomOrigin(Frame f,uint level){uint x=0;for(uint j=1;j<level;j++)x+=max(f.extent.x>>j,1u);return uint2(x,0);}
float3 bloomLoad(Frame f,uint sampled,uint storage,bool inPlace,uint level,int2 p){
    int2 q=int2(bloomOrigin(f,level))+clamp(p,int2(0),int2(bloomSize(f,level))-1);
    return inPlace?images[NonUniformResourceIndex(storage)][q].rgb:pixel(sampled,q).rgb;
}
// Bilinear tap of `level` at continuous texel position `at`, then a 3x3 tent.
float3 bloomTent(Frame f,uint sampled,uint storage,bool inPlace,uint level,float2 at){
    float3 sum=0;
    for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
        float2 q=at+float2(x,y)-0.5;int2 i=int2(floor(q));float2 a=q-i;
        float3 v=lerp(lerp(bloomLoad(f,sampled,storage,inPlace,level,i),bloomLoad(f,sampled,storage,inPlace,level,i+int2(1,0)),a.x),
                      lerp(bloomLoad(f,sampled,storage,inPlace,level,i+int2(0,1)),bloomLoad(f,sampled,storage,inPlace,level,i+int2(1,1)),a.x),a.y);
        sum+=v*(x==0?2:1)*(y==0?2:1);
    }return sum/16;
}
[shader("compute")][numthreads(8,8,1)]
void bloomMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint pass=root.material;
    uint level=(pass<5?pass:8-pass)+BLOOM_FIRST;uint2 size=bloomSize(f,level),p=tid.xy;if(any(p>=size))return;
    float3 c;
    if(pass==0){
        // 13-tap downsample (Jimenez 2014) of the scene to quarter resolution, taps two
        // texels apart (each bilinear), with Karis weights per 2x2 block on exposed
        // luminance, so lone HDR fireflies cannot flash into the glow.
        float2 texel=2/float2(f.extent.xy),uv=(float2(p)+0.5)*2*texel;float3 t[13];float scale=1/inputScale(f,l,true);
        float2 o[13]={float2(-2,-2),float2(0,-2),float2(2,-2),float2(-1,-1),float2(1,-1),float2(-2,0),float2(0,0),float2(2,0),float2(-1,1),float2(1,1),float2(-2,2),float2(0,2),float2(2,2)};
        for(uint i=0;i<13;i++)t[i]=min(historyTex(root.materials,uv+o[i]*texel).rgb*scale,float3(60000));
        uint4 groups[5]={uint4(3,4,8,9),uint4(0,1,5,6),uint4(1,2,6,7),uint4(5,6,10,11),uint4(6,7,11,12)};
        float exposure=exposureState(l).x;float weights[5]={0.5,0.125,0.125,0.125,0.125};c=0;float total=0;
        for(uint g=0;g<5;g++){uint4 k=groups[g];float3 m=(t[k.x]+t[k.y]+t[k.z]+t[k.w])/4;float w=weights[g]/(1+luminance(m)*exposure);c+=m*w;total+=w;}
        c/=total;
    }else if(pass<5){
        // 4x4 [1 3 3 1] tent of the finer level, read in place from the down atlas.
        c=0;for(int y=0;y<4;y++)for(int x=0;x<4;x++)c+=bloomLoad(f,l.sampled1.z,l.storage1.z,true,level-1,int2(p)*2+int2(x,y)-1)*(x==0||x==3?1:3)*(y==0||y==3?1:3);c/=64;
    }else{
        // This level's downsample plus the coarser level's accumulated glow.
        float2 coarse=(float2(p)+0.5)*0.5;
        float3 up=level==BLOOM_FIRST+3?bloomTent(f,l.sampled1.z,l.storage1.z,false,BLOOM_FIRST+4,coarse):bloomTent(f,l.sampled1.w,l.storage1.w,true,level+1,coarse);
        c=bloomLoad(f,l.sampled1.z,l.storage1.z,false,level,int2(p))+up;
    }
    images[NonUniformResourceIndex(pass<5?l.storage1.z:l.storage1.w)][bloomOrigin(f,level)+p]=float4(c,1);
}
// The glow at a pixel: the finest upsampled level averaged over its five scales.
float3 bloom(Frame f,Look l,float2 uv){
    float2 size=float2(bloomSize(f,BLOOM_FIRST)),atlas=float2(f.extent.x,max(f.extent.y/2,1u));
    float2 origin=float2(bloomOrigin(f,BLOOM_FIRST));
    return historyTex(l.sampled1.w,(origin+clamp(uv*size,0.5,size-0.5))/atlas).rgb/5;
}
// Scene-linear grading after exposure: white balance, power contrast about 18%
// grey, saturation, then the screen-space vignette.
float3 grade(Frame f,Look l,float3 c,float2 uv){
    c=max(c*l.grading.rgb,0);if(l.look.x!=1)c=0.18*pow(c/0.18,l.look.x);c=max(lerp(float3(luminance(c)),c,l.grading.w),0);
    float2 d=(uv-0.5)*float2(float(f.extent.x)/f.extent.y,1);float falloff=1/(1+dot(d,d)*l.look.y*2.5);return c*falloff*falloff;
}
[shader("compute")][numthreads(8,8,1)]
void postMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=f.extent.xy))return;int2 p=int2(tid.xy);float2 uv=(float2(p)+0.5)/float2(f.extent.xy);
    float3 c=pixel(root.materials,p).rgb/inputScale(f,l,true);
    if(l.bloom.x>0)c=lerp(c,bloom(f,l,uv),l.bloom.x);
    c=grade(f,l,max(c*exposureState(l).x,0),uv);
    uint curve=l.flags.x;c=curve==1?neutralCurve(c):curve==2?filmicCurve(c):tonemap(c);
    images[NonUniformResourceIndex(f.post.w)][p]=float4(srgb(saturate(c)),1);
}
