#include "common.slangh"
groupshared float3 radiance[64];
groupshared float distances[64];
groupshared float3 directions[64];
groupshared uint backs[64];
[shader("compute")][numthreads(64,1,1)]
void ddgiMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint id=group.x;if(id>=f.counts.w)return;
    uint relative=(id+f.counts.w-f.probes.w)%f.counts.w;bool active=relative<f.probes.z;
    if(!active){for(uint i=lane;i<102;i+=64)buffers[NonUniformResourceIndex(f.probes.y)].Store<float4>((id*102+i)*16,probeLoad(f.probes.x,id,i));return;}
    float4 meta=probeLoad(f.probes.x,id,100);float3 origin=probePosition(f,id)+meta.xyz;
    // Rotate a spherical Fibonacci distribution deterministically each frame.
    float phi=lane*2.39996322973+float(hash(f.control.w)&65535)*(2*PI/65536);float y=1-2*(lane+0.5)/64.;float r=sqrt(max(0,1-y*y));float3 dir=float3(cos(phi)*r,y,sin(phi)*r);
    uint rng=hash(id*64+lane+f.control.w*7919);Hit hit;float3 incoming;float distance=50;uint back=0;
    if(trace(f,origin,dir,50,rng,hit)){
        Surface s=surface(f,hit.id,hit.primitive,hit.bary);distance=hit.distance;back=dot(s.geometricNormal,dir)>0?1:0;
        if(dot(s.normal,-dir)<0)s.normal=-s.normal;incoming=s.emissive+direct(f,s,-dir,rng)+indirect(f,s,-dir,f.probes.x);
    }else incoming=environment(f,dir);
    radiance[lane]=incoming;distances[lane]=distance;directions[lane]=dir;backs[lane]=back;GroupMemoryBarrierWithGroupSync();
    uint backCount=0;float nearest=50;uint nearestLane=0;
    for(uint j=0;j<64;j++){backCount+=backs[j];if(distances[j]<nearest){nearest=distances[j];nearestLane=j;}}
    float3 relocation=meta.xyz;bool inside=backCount>16;
    if(inside)relocation+=directions[nearestLane]*min(0.2,nearest+0.05);
    else if(nearest<0.12)relocation-=directions[nearestLane]*(0.12-nearest);
    relocation=clamp(relocation,-f.spacing.xyz*0.45,f.spacing.xyz*0.45);
    bool moved=length(relocation-meta.xyz)>0.01;
    float hysteresis=(f.sunLux.w>0 || meta.w<0.5 || moved)?0.15:0.95;
    if(lane<36){
        float3 n=octDecode((float2(lane%6,lane/6)+0.5)/6);float3 sum=0;float normalization=0;
        for(uint j=0;j<64;j++){float w=max(dot(n,directions[j]),0);sum+=radiance[j]*w;normalization+=w;}
        float3 value=sum*(PI/max(normalization,1e-5));float3 old=probeLoad(f.probes.x,id,lane).rgb;
        // Large lighting changes automatically reduce temporal debt even without a host flag.
        if(length(value-old)>max(length(old),0.1)*0.25)hysteresis=min(hysteresis,0.3);
        buffers[NonUniformResourceIndex(f.probes.y)].Store<float4>((id*102+lane)*16,float4(lerp(value,old,hysteresis),1));
    }
    float3 n=octDecode((float2(lane%8,lane/8)+0.5)/8);float2 moments=0;float normalization=0;
    for(uint j=0;j<64;j++){float w=pow(max(dot(n,directions[j]),0),32);moments+=float2(distances[j],distances[j]*distances[j])*w;normalization+=w;}
    moments/=max(normalization,1e-7);float2 old=probeLoad(f.probes.x,id,36+lane).xy;
    buffers[NonUniformResourceIndex(f.probes.y)].Store<float4>((id*102+36+lane)*16,float4(lerp(moments,old,hysteresis),0,0));
    if(lane==0){buffers[NonUniformResourceIndex(f.probes.y)].Store<float4>((id*102+100)*16,float4(relocation,inside?0:1));buffers[NonUniformResourceIndex(f.probes.y)].Store<float4>((id*102+101)*16,float4(backCount,nearest,f.control.w,1));}
}
