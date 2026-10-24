#define RASTER 1
#include "common.slangh"
float luminance(float3 c){return dot(c,float3(0.2126,0.7152,0.0722));}
// Fixed 128x72 sampling grid, 256 log-luminance bins over [-12,20] stops.
// A single workgroup avoids global histogram clears, atomics or readbacks.
groupshared uint histogram[256];
[shader("compute")][numthreads(256,1,1)]
void exposureMain(uint lane:SV_GroupIndex){
    Frame f=frame();histogram[lane]=0;GroupMemoryBarrierWithGroupSync();
    if(f.post.x<0){
        for(uint i=lane;i<9216;i+=256){float3 c=sampleImage(f,FILTER0,(float2(i%128,i/128)+0.5)/float2(128,72)).rgb;
            float logL=clamp(log2(max(luminance(c),exp2(-12.))),-12.,20.);uint bin=min(uint((logL+12)*8),255);InterlockedAdd(histogram[bin],1);}
    }
    GroupMemoryBarrierWithGroupSync();if(lane!=0)return;
    float target=f.post.x,metered=0,ev=0;
    if(target<0){float low=f.metering.z*9216,high=f.metering.w*9216,prefix=0,sum=0,mass=0;
        for(uint bin=0;bin<256;bin++){float count=histogram[bin];float weight=max(0,min(prefix+count,high)-max(prefix,low));sum+=(-12+(bin+0.5)/8)*weight;mass+=weight;prefix+=count;}
        metered=exp2(sum/max(mass,1));ev=clamp(log2(metered/(0.18*1.2)),f.metering.x,f.metering.y);target=(-f.post.x)/(1.2*exp2(ev));
    }else ev=log2(1/max(1.2*target,1e-12));
    float old=buffers[NonUniformResourceIndex(f.control.y)].Load<float>(0);float value=f.temporal.x>0 || f.post.x>0?target:lerp(old,target,1-exp(-f.temporal.z*f.post.y));
    buffers[NonUniformResourceIndex(f.control.y)].Store<float4>(0,float4(value,old,ev,metered));
}
float3 neutral(float3 colour){
    float minimum=min(colour.x,min(colour.y,colour.z));float offset=minimum<=0.08?minimum-6.25*minimum*minimum:0.04;colour-=offset;
    float peak=max(colour.x,max(colour.y,colour.z));if(peak<=0.76)return colour;
    float compressed=1-0.0576/(peak-0.52);float blend=1-1/(1+0.15*(peak-compressed));return lerp(colour*(compressed/peak),float3(compressed),blend);
}
// Five-level bloom pyramid packed as an atlas in the top half of BLOOM0 (down) and
// BLOOM1 (up): level k (1..5) is (size>>k) at x offset sum(size>>j, j<k), y 0.
// pass 0 Karis-weighted downsample of FILTER0, 1..4 tent downsamples, 5..8 tent
// upsamples adding each coarser level back; post samples level 1 / 5 as the glow.
uint2 bloomSize(Frame f,uint level){return max(f.sizes.xy>>level,uint2(1));}
uint2 bloomOrigin(Frame f,uint level){uint x=0;for(uint j=1;j<level;j++)x+=max(f.sizes.x>>j,1u);return uint2(x,0);}
float3 bloomLoad(Frame f,uint image,bool storage,uint level,int2 p){
    int2 q=int2(bloomOrigin(f,level))+clamp(p,int2(0),int2(bloomSize(f,level))-1);
    return storage?images[wi(f,image)][q].rgb:textures[ri(f,image)].Load(int3(q,0)).rgb;
}
float3 bloomTent(Frame f,uint image,bool storage,uint level,float2 at){
    // Bilinear tap of `level` at continuous texel position `at`, then a 3x3 tent.
    float3 sum=0;
    for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
        float2 q=at+float2(x,y)-0.5;int2 i=int2(floor(q));float2 a=q-i;
        float3 v=lerp(lerp(bloomLoad(f,image,storage,level,i),bloomLoad(f,image,storage,level,i+int2(1,0)),a.x),
                      lerp(bloomLoad(f,image,storage,level,i+int2(0,1)),bloomLoad(f,image,storage,level,i+int2(1,1)),a.x),a.y);
        sum+=v*(x==0?2:1)*(y==0?2:1);
    }return sum/16;
}
[shader("compute")][numthreads(8,8,1)]
void bloomMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint pass=root.pass;
    uint level=pass<5?pass+1:9-pass;uint2 size=bloomSize(f,level),p=tid.xy;if(any(p>=size))return;
    float3 c;
    if(pass==0){
        // 13-tap downsample (Jimenez 2014) with Karis luminance weights per 2x2
        // block, so lone HDR fireflies cannot flash into the glow.
        float2 texel=1/float2(f.sizes.xy),uv=(float2(p)+0.5)*2*texel;float3 t[13];
        float2 o[13]={float2(-2,-2),float2(0,-2),float2(2,-2),float2(-1,-1),float2(1,-1),float2(-2,0),float2(0,0),float2(2,0),float2(-1,1),float2(1,1),float2(-2,2),float2(0,2),float2(2,2)};
        for(uint i=0;i<13;i++)t[i]=min(sampleImage(f,FILTER0,uv+o[i]*texel).rgb,float3(60000));
        uint4 groups[5]={uint4(3,4,8,9),uint4(0,1,5,6),uint4(1,2,6,7),uint4(5,6,10,11),uint4(6,7,11,12)};
        // Weights use exposed luminance; physical cd/m2 would weight every block ~1/L.
        float exposure=buffers[NonUniformResourceIndex(f.control.y)].Load<float>(0);
        float weights[5]={0.5,0.125,0.125,0.125,0.125};c=0;float total=0;
        for(uint g=0;g<5;g++){
            uint4 k=groups[g];float3 m=(t[k.x]+t[k.y]+t[k.z]+t[k.w])/4;
            float w=weights[g]/(1+luminance(m)*exposure);c+=m*w;total+=w;
        }c/=total;
    }else if(pass<5){
        // 4x4 [1 3 3 1] tent from the finer level, read in place from BLOOM0.
        c=0;for(int y=0;y<4;y++)for(int x=0;x<4;x++)c+=bloomLoad(f,BLOOM0,true,level-1,int2(p)*2+int2(x,y)-1)*(x==0||x==3?1:3)*(y==0||y==3?1:3);c/=64;
    }else{
        // Upsample: this level's downsample plus the coarser level's accumulated glow.
        float2 coarse=(float2(p)+0.5)*0.5;
        float3 up=level==4?bloomTent(f,BLOOM0,false,5,coarse):bloomTent(f,BLOOM1,true,level+1,coarse);
        c=bloomLoad(f,BLOOM0,false,level,int2(p))+up;
    }
    store(f,pass<5?BLOOM0:BLOOM1,bloomOrigin(f,level)+p,float4(c,1));
}
// The glow at a full-resolution pixel: level 1 of the upsampled pyramid averaged
// over its five scales, sampled bilinearly inside its atlas region.
float3 bloom(Frame f,float2 uv){
    float2 size=float2(bloomSize(f,1)),atlas=float2(f.sizes.xy);
    float2 at=clamp(uv*size,0.5,size-0.5)/atlas;return sampleImage(f,BLOOM1,at).rgb/5;
}
float3 sky(Frame f,float3 direction){
    float sun=dot(direction,f.sunDirection.xyz);float rayleigh=3/(16*PI)*(1+sun*sun);float g=0.76;float mie=(1-g*g)/(4*PI*pow(max(1+g*g-2*g*sun,0.001),1.5));
    float horizon=1/max(direction.y*0.7+0.3,0.08);float3 beta=float3(0.0058,0.0135,0.0331)*horizon;
    return f.sunLux.rgb*(beta*rayleigh+float3(0.002)*mie)*max(f.sunDirection.y*0.5+0.5,0.02);
}
float3 grading(Frame f,float3 colour){
    float3 position=saturate(colour)*15;uint3 low=uint3(floor(position)),high=min(low+1,uint3(15));float3 alpha=frac(position);float3 sum=0;
    for(uint i=0;i<8;i++){uint3 d=uint3(i&1,(i>>1)&1,(i>>2)&1),p=uint3(d.x?high.x:low.x,d.y?high.y:low.y,d.z?high.z:low.z);float3 w=lerp(1-alpha,alpha,float3(d));sum+=textures[NonUniformResourceIndex(f.control.z)].Load(int3(p.z*16+p.x,p.y,0)).rgb*w.x*w.y*w.z;}return sum;
}
float3 srgb(float3 c){return float3(c.x<=0.0031308?c.x*12.92:1.055*pow(c.x,1/2.4)-0.055,c.y<=0.0031308?c.y*12.92:1.055*pow(c.y,1/2.4)-0.055,c.z<=0.0031308?c.z*12.92:1.055*pow(c.z,1/2.4)-0.055);}
// Hill's fitted ACES RRT+ODT in AP1; the 1.464 pre-scale keeps 18% grey at 18%
// (colour.rs FILMIC_SCALE). Matrices are row-major as written.
float3 filmic(float3 colour){
    colour*=1.464;
    float3 v=float3(dot(float3(0.59719,0.35458,0.04823),colour),dot(float3(0.07600,0.90834,0.01566),colour),dot(float3(0.02840,0.13383,0.83777),colour));
    v=(v*(v+0.0245786)-0.000090537)/(v*(0.983729*v+0.4329510)+0.238081);
    return saturate(float3(dot(float3(1.60475,-0.53108,-0.07367),v),dot(float3(-0.10208,1.10813,-0.00605),v),dot(float3(-0.00327,-0.07276,1.07602),v)));
}
// Scene-linear grading after exposure (colour.rs Grading::apply): white balance,
// power contrast about 18% grey, saturation, then the screen-space vignette.
float3 grade(Frame f,float3 c,float2 uv){
    c=max(c*f.grading.rgb,0);if(f.look.x!=1)c=0.18*pow(c/0.18,f.look.x);c=max(lerp(float3(luminance(c)),c,f.grading.w),0);
    float2 d=(uv-0.5)*float2(float(f.sizes.x)/f.sizes.y,1);float falloff=1/(1+dot(d,d)*f.look.y*2.5);return c*falloff*falloff;
}
[shader("compute")][numthreads(8,8,1)]
void postMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;float2 uv=(float2(p)+0.5)/float2(f.sizes.xy);float3 c=pixel(f,FILTER0,p).rgb;
    float depth=pixel(f,DEPTH,p).x;float2 motion=pixel(f,MOTION,p).xy;float coc=depth>0?min(12,abs(depth-f.lens.x)/max(depth,0.1)*f.lens.y):0;
    if(f.lens.w>0){float3 sum=c;float total=1;for(uint i=0;i<12;i++){float angle=i*2.399963;float radius=sqrt((i+0.5)/12.);float2 offset=float2(cos(angle),sin(angle))*radius*coc/float2(f.sizes.xy)+motion*(float(i)/11.-0.5)*f.lens.z;
        float other=sampleImage(f,DEPTH,uv+offset).x;float w=abs(other-depth)<max(0.1,depth*0.1)?1:0;sum+=sampleImage(f,FILTER0,uv+offset).rgb*w;total+=w;}c=sum/total;}
    c=lerp(c,bloom(f,uv),f.post.z);
    float3 direction=cameraRay(f,float2(p)+0.5);
    if(f.display.w!=0){
        // The physical sky already integrates the whole atmosphere along sky rays;
        // under an environment map, sky pixels take the fog of the full froxel range.
        if(depth==0 && (f.display.w&2)!=0)c=atmosphereEnvironment(f,direction,true);
        else {float distance=depth==0?4000.:length(pixel(f,WORLD+f.control.x,p).xyz-f.camera.xyz);float3 trans;float3 fog=sampleFroxel(f,uv,distance,trans);c=c*trans+fog;}
    }else {float transmittance=exp(-f.post.w*(depth>0?depth:500));c=c*transmittance+sky(f,direction)*(1-transmittance);}
    float exposure=buffers[NonUniformResourceIndex(f.control.y)].Load<float>(0);c=grade(f,max(c*exposure,0),uv);
    // 0: Narkowicz's ACES fit (an SDR fit, not a full ACES transform), 1: PBR Neutral, 2: filmic.
    c=f.display.x==1?neutral(c):f.display.x==2?filmic(c):saturate((c*(2.51*c+0.03))/(c*(2.43*c+0.59)+0.14));c=srgb(grading(f,c));store(f,POST,p,float4(c,1));
}
