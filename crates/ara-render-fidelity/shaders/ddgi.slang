#include "common.slangh"
groupshared float3 radiance[64];
groupshared float distances[64];
groupshared float3 directions[64];
groupshared uint backs[64];
[shader("compute")][numthreads(64,1,1)]
void ddgiMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint id=group.x;if(id>=f.counts.w)return;
    float4 meta=probeLoad(f.probes.x,id,100);bool responsive=f.sunLux.w>0;
    uint rays=responsive?16:64;
    uint relative=(id+f.counts.w-f.probes.w)%f.counts.w;bool active=relative<f.probes.z && (!responsive || meta.w>0.5);
    if(!active){for(uint i=lane;i<102;i+=64)buffers[NonUniformResourceIndex(f.probes.y)].Store<float4>((id*102+i)*16,probeLoad(f.probes.x,id,i));return;}
    float3 origin=probePosition(f,id)+meta.xyz;
    // A deterministic full-sphere rotation prevents permanent latitude bands.
    float phi=lane*2.39996322973+float(hash(f.control.w)&65535)*(2*PI/65536);float y=1-2*(lane+0.5)/float(rays);float r=sqrt(max(0,1-y*y));float3 dir=float3(cos(phi)*r,y,sin(phi)*r);
    uint rotation=hash(f.control.w+id*379);float3 axis=normalize(float3(float(hash(rotation)&65535),float(hash(rotation+1)&65535),float(hash(rotation+2)&65535))/32767.5-1);float angle=float(hash(rotation+3)&65535)*(2*PI/65536);
    dir=dir*cos(angle)+cross(axis,dir)*sin(angle)+axis*dot(axis,dir)*(1-cos(angle));
    uint rng=hash(id*64+lane+f.control.w*7919);Hit hit;float3 incoming;float distance=50;uint back=0;
    incoming=0;if(lane<rays){if(trace(f,origin,dir,50,rng,hit)){
        Surface s=surface(f,hit.id,hit.primitive,hit.bary);distance=hit.distance;back=dot(s.geometricNormal,dir)>0 && s.transmission==0 && material(f,s.material).flags.x==0?1:0;
        if(dot(s.normal,-dir)<0)s.normal=-s.normal;incoming=0;float3 throughput=1,pathDirection=dir;
        // Resolve several transport events inside the ray budget. Repeatedly feeding
        // visibility-filtered irradiance back as a first bounce creates artificial
        // absorption in enclosed rooms, especially across thin partitions.
        for(uint bounce=0;bounce<5;bounce++){
            incoming+=throughput*((bounce==0?s.emissive:float3(0))+direct(f,s,-pathDirection,rng));
            if(bounce==4){incoming+=throughput*indirect(f,s,-pathDirection,f.probes.x);break;}
            float3 weight;pathDirection=sampleBrdf(s,-pathDirection,rng,weight);throughput*=weight;
            if(max(max(throughput.x,throughput.y),throughput.z)<0.001)break;
            Hit next;if(!trace(f,rayOrigin(s,pathDirection),pathDirection,50,rng,next)){incoming+=throughput*environment(f,pathDirection);break;}
            s=surface(f,next.id,next.primitive,next.bary);if(dot(s.normal,-pathDirection)<0)s.normal=-s.normal;
        }
    }else incoming=environment(f,dir);}
    radiance[lane]=incoming;distances[lane]=distance;directions[lane]=dir;backs[lane]=back;GroupMemoryBarrierWithGroupSync();
    uint backCount=0;float nearest=50;uint nearestLane=0;float nearestBack=50;uint backLane=0;
    for(uint j=0;j<rays;j++){backCount+=backs[j];if(backs[j]!=0 && distances[j]<nearestBack){nearestBack=distances[j];backLane=j;}if(distances[j]<nearest){nearest=distances[j];nearestLane=j;}}
    float3 relocation=meta.xyz;bool inside=backCount>16;
    if(inside)relocation+=directions[backLane]*min(length(f.spacing.xyz)*0.25,nearestBack+0.03);
    else if(nearest<0.12)relocation-=directions[nearestLane]*(0.12-nearest);
    relocation=clamp(relocation,-f.spacing.xyz*0.45,f.spacing.xyz*0.45);
    if(responsive){relocation=meta.xyz;inside=false;}
    bool moved=length(relocation-meta.xyz)>0.005;if(!moved)relocation=meta.xyz;
    float hysteresis=(meta.w<0.5 || moved || responsive)?0:0.95;
    if(lane<36){
        float3 n=octDecode((float2(lane%6,lane/6)+0.5)/6);float3 sum=0;float normalization=0;
        for(uint j=0;j<rays;j++){float w=max(dot(n,directions[j]),0);sum+=radiance[j]*w;normalization+=w;}
        float3 value=sum*(PI/max(normalization,1e-5));float3 old=probeLoad(f.probes.x,id,lane).rgb;
        // Large lighting changes automatically reduce temporal debt even without a host flag.
        if(length(value-old)>max(length(old),0.1)*0.25)hysteresis=min(hysteresis,0.3);
        buffers[NonUniformResourceIndex(f.probes.y)].Store<float4>((id*102+lane)*16,float4(lerp(value,old,hysteresis),1));
    }
    float3 n=octDecode((float2(lane%8,lane/8)+0.5)/8);float2 moments=0;float normalization=0;
    for(uint j=0;j<64;j++){float w=pow(max(dot(n,directions[j]),0),32);moments+=float2(distances[j],distances[j]*distances[j])*w;normalization+=w;}
    moments/=max(normalization,1e-7);float2 old=probeLoad(f.probes.x,id,36+lane).xy;if(responsive)moments=old;
    buffers[NonUniformResourceIndex(f.probes.y)].Store<float4>((id*102+36+lane)*16,float4(lerp(moments,old,hysteresis),0,0));
    if(lane==0){buffers[NonUniformResourceIndex(f.probes.y)].Store<float4>((id*102+100)*16,float4(relocation,(inside || moved)?0:1));buffers[NonUniformResourceIndex(f.probes.y)].Store<float4>((id*102+101)*16,float4(backCount,nearest,f.control.w,1));}
}
