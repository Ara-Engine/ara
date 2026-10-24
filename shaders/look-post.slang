// Exposure metering, bloom and display grading of the anti-aliased HDR scene
// (look/passes.rs). Root word 1 (`root.materials`) is the scene's sampled index.
#include "raster.slangh"
#include "look.slangh"
float4 exposureState(Look l){return buffers[NonUniformResourceIndex(l.state.x)].Load<float4>(0);}
// Fixed 128x72 sampling grid, 256 log-luminance bins over [-12,20] stops. A single
// workgroup avoids global histogram clears, atomics or readbacks. The state is
// (applied multiplier, previous multiplier, EV100, metered luminance).
groupshared uint histogram[256];
[shader("compute")][numthreads(256,1,1)]
void exposureMain(uint lane:SV_GroupIndex){
    Frame f=frame();Look l=look(f);histogram[lane]=0;GroupMemoryBarrierWithGroupSync();
    bool automatic=l.exposure.x<=0;
    if(automatic){
        for(uint i=lane;i<9216;i+=256){float3 c=historyTex(root.materials,(float2(i%128,i/128)+0.5)/float2(128,72)).rgb;
            float logL=clamp(log2(max(luminance(c),exp2(-12.))),-12.,20.);InterlockedAdd(histogram[min(uint((logL+12)*8),255)],1);}
    }
    GroupMemoryBarrierWithGroupSync();if(lane!=0)return;
    float target=l.exposure.x,metered=0,ev=0;
    if(automatic){
        float low=l.metering.z*9216,high=l.metering.w*9216,prefix=0,sum=0,mass=0;
        for(uint bin=0;bin<256;bin++){float count=histogram[bin];float weight=max(0,min(prefix+count,high)-max(prefix,low));sum+=(-12+(bin+0.5)/8)*weight;mass+=weight;prefix+=count;}
        metered=exp2(sum/max(mass,1));ev=clamp(log2(metered/(0.18*1.2)),l.metering.x,l.metering.y);target=l.exposure.y/(1.2*exp2(ev));
    }else ev=log2(1/max(1.2*target,1e-12));
    float old=exposureState(l).x;bool reset=!automatic || (l.flags.y&LOOK_RESET)!=0 || !(old>0);
    float value=reset?target:lerp(old,target,1-exp(-l.exposure.w*l.exposure.z));
    buffers[NonUniformResourceIndex(l.state.x)].Store<float4>(0,float4(value,old,ev,metered));
}
// Five-level bloom pyramid packed in the top half of the down and up atlases:
// level k (1..5) is (size>>k) at x offset sum(size>>j, j<k), y 0. Pass 0 is a
// Karis-weighted downsample of the scene, 1..4 tent downsamples, 5..8 tent
// upsamples adding each coarser level back; post samples level 1 as the glow.
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
    uint level=pass<5?pass+1:9-pass;uint2 size=bloomSize(f,level),p=tid.xy;if(any(p>=size))return;
    float3 c;
    if(pass==0){
        // 13-tap downsample (Jimenez 2014) with Karis weights per 2x2 block on
        // exposed luminance, so lone HDR fireflies cannot flash into the glow.
        float2 texel=1/float2(f.extent.xy),uv=(float2(p)+0.5)*2*texel;float3 t[13];
        float2 o[13]={float2(-2,-2),float2(0,-2),float2(2,-2),float2(-1,-1),float2(1,-1),float2(-2,0),float2(0,0),float2(2,0),float2(-1,1),float2(1,1),float2(-2,2),float2(0,2),float2(2,2)};
        for(uint i=0;i<13;i++)t[i]=min(historyTex(root.materials,uv+o[i]*texel).rgb,float3(60000));
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
        float3 up=level==4?bloomTent(f,l.sampled1.z,l.storage1.z,false,5,coarse):bloomTent(f,l.sampled1.w,l.storage1.w,true,level+1,coarse);
        c=bloomLoad(f,l.sampled1.z,l.storage1.z,false,level,int2(p))+up;
    }
    images[NonUniformResourceIndex(pass<5?l.storage1.z:l.storage1.w)][bloomOrigin(f,level)+p]=float4(c,1);
}
// The glow at a pixel: level 1 of the upsampled pyramid averaged over its five scales.
float3 bloom(Frame f,Look l,float2 uv){
    float2 size=float2(bloomSize(f,1)),atlas=float2(f.extent.x,max(f.extent.y/2,1u));
    return historyTex(l.sampled1.w,clamp(uv*size,0.5,size-0.5)/atlas).rgb/5;
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
    float3 c=pixel(root.materials,p).rgb;
    if(l.bloom.x>0)c=lerp(c,bloom(f,l,uv),l.bloom.x);
    c=grade(f,l,max(c*exposureState(l).x,0),uv);
    uint curve=l.flags.x;c=curve==1?neutralCurve(c):curve==2?filmicCurve(c):tonemap(c);
    images[NonUniformResourceIndex(f.post.w)][p]=float4(srgb(saturate(c)),1);
}
