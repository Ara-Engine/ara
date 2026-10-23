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
[shader("compute")][numthreads(8,8,1)]
void bloomMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint source=root.pass==0?FILTER0:BLOOM0,target=root.pass==0?BLOOM0:BLOOM1;float3 sum=0;float total=0;
    for(int i=-8;i<=8;i++){float w=exp(-float(i*i)/18);int2 at=int2(p)+(root.pass==0?int2(i*2,0):int2(0,i*2));sum+=pixel(f,source,at).rgb*w;total+=w;}store(f,target,p,float4(sum/total,1));
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
[shader("compute")][numthreads(8,8,1)]
void postMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;float2 uv=(float2(p)+0.5)/float2(f.sizes.xy);float3 c=pixel(f,FILTER0,p).rgb;
    float depth=pixel(f,DEPTH,p).x;float2 motion=pixel(f,MOTION,p).xy;float coc=depth>0?min(12,abs(depth-f.lens.x)/max(depth,0.1)*f.lens.y):0;
    if(f.lens.w>0){float3 sum=c;float total=1;for(uint i=0;i<12;i++){float angle=i*2.399963;float radius=sqrt((i+0.5)/12.);float2 offset=float2(cos(angle),sin(angle))*radius*coc/float2(f.sizes.xy)+motion*(float(i)/11.-0.5)*f.lens.z;
        float other=sampleImage(f,DEPTH,uv+offset).x;float w=abs(other-depth)<max(0.1,depth*0.1)?1:0;sum+=sampleImage(f,FILTER0,uv+offset).rgb*w;total+=w;}c=sum/total;}
    c=lerp(c,pixel(f,BLOOM1,p).rgb,f.post.z);
    float3 direction=cameraRay(f,float2(p)+0.5);float3 atmosphere=sky(f,direction);
    float transmittance=exp(-f.post.w*(depth>0?depth:500));c=c*transmittance+atmosphere*(1-transmittance);
    float exposure=buffers[NonUniformResourceIndex(f.control.y)].Load<float>(0);c=max(c*exposure,0);
    // Narkowicz's ACES fitted curve, matching M1; an SDR fit, not a full ACES transform.
    c=f.display.x==1?neutral(c):saturate((c*(2.51*c+0.03))/(c*(2.43*c+0.59)+0.14));c=srgb(grading(f,c));store(f,POST,p,float4(c,1));
}
