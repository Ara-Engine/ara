#include "common.slangh"
[shader("compute")][numthreads(8,8,1)]
void primaryMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint rng=hash(p.y*f.sizes.x+p.x+f.control.w*7919);
    float3 ray=cameraRay(f,float2(p)+0.5);Hit hit;uint slot=f.control.x;
    if(!trace(f,f.camera.xyz,ray,10000,rng,hit)){store(f,WORLD+slot,p,0);store(f,NORMAL+slot,p,0);store(f,BASE,p,0);store(f,MOTION,p,0);store(f,DEPTH,p,0);if(f.transport.x==0){store(f,DIRECT,p,0);store(f,RAW,p,float4(environment(f,ray),1));}return;}
    Surface s=surface(f,hit.id,hit.primitive,hit.bary);if(dot(s.normal,-ray)<0)s.normal=-s.normal;
    float4 previous=mul(f.previousVp,float4(s.previousPosition,1));float4 current=mul(f.vp,float4(s.position,1));
    float2 previousUv=float2(previous.x,-previous.y)/previous.w*0.5+0.5;float2 currentUv=float2(current.x,-current.y)/current.w*0.5+0.5;
    store(f,WORLD+slot,p,float4(s.position,float(s.id+1)));store(f,NORMAL+slot,p,float4(s.normal,s.roughness));store(f,BASE,p,float4(hit.bary,float(hit.primitive),hit.distance));
    store(f,MOTION,p,float4(previousUv-currentUv,length(s.previousPosition-f.previousCamera.xyz),max(max(s.emissive.x,s.emissive.y),s.emissive.z)>1?1:0));
    store(f,DEPTH,p,float4(current.w,0,0,0));
    if(f.transport.x==0){store(f,DIRECT,p,float4(s.emissive+direct(f,s,-ray,rng),1));store(f,RAW,p,0);}
}
float3 gather(Frame f,Surface s,float3 direction,inout uint rng){
    float3 result=0,throughput=1;
    for(uint bounce=0;bounce<2;bounce++){
        Hit hit;if(!trace(f,rayOrigin(s,direction),direction,200,rng,hit)){result+=throughput*environment(f,direction);break;}
        Surface r=surface(f,hit.id,hit.primitive,hit.bary);if(dot(r.normal,-direction)<0)r.normal=-r.normal;
        result+=throughput*(r.emissive+direct(f,r,-direction,rng));
        if(bounce==1){result+=throughput*indirect(f,r,-direction,f.probes.y);break;}
        float3 v=-direction;direction=cosineDirection(r.normal,rng);float3 F=fresnel(max(dot(normalize(v+direction),v),0),lerp(float3(0.04),r.albedo,r.metal));
        throughput*=r.albedo*(1-r.metal)*(1-F);s=r;
    }return result;
}
[shader("compute")][numthreads(8,8,1)]
void hybridMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(root.pass>0)p.x=p.x*2+((p.y+f.control.w+(root.pass==2?1:0))&1);if(any(p>=f.sizes.xy))return;uint rng=hash(p.y*f.sizes.x+p.x+f.control.w*7919);
    float4 world=pixel(f,WORLD+f.control.x,p);float3 ray=cameraRay(f,float2(p)+0.5);
    if(world.w==0)return;
    float4 hit=pixel(f,BASE,p);Surface s=surface(f,uint(world.w)-1,uint(hit.z),hit.xy);if(dot(s.normal,-ray)<0)s.normal=-s.normal;float3 v=-ray;
    float3 gi=0;
    float2 oldUv;int2 oldAt;
    if(root.pass==2 && reproject(f,p,oldUv,oldAt))return;
    float3 f0=lerp(float3(0.04),s.albedo,s.metal);
    if(s.metal<1){float3 l=cosineDirection(s.normal,rng);float3 F=fresnel(max(dot(normalize(v+l),v),0),f0);
        gi+=gather(f,s,l,rng)*s.albedo*(1-s.metal)*(1-F);
    }
    // Visible-normal reflection weights are bounded by Fresnel, including at the rim.
    float3 h=visibleGgx(s,v,rng),l=reflect(-v,h);float nl=dot(s.normal,l);
    float traced=1-smoothstep(max(0,f.temporal.w-0.1),max(0.001,f.temporal.w),s.roughness);float3 specular=0;
    if(traced>0 && nl>0)specular=gather(f,s,l,rng)*fresnel(max(dot(v,h),0),f0)*smith(nl,pow(s.roughness,4))*traced;
    if(traced<1)specular+=irradiance(f,s.position,reflect(-v,s.normal),f.probes.y)*fresnel(sqrt(0.5+0.5*saturate(dot(s.normal,v))),f0)*(1-traced)/PI;
    float3 colour=gi+specular;
    store(f,RAW,p,float4(max(colour,0),1));
}
[shader("compute")][numthreads(8,8,1)]
void referenceMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;float3 result=0;
    for(uint sample=0;sample<f.transport.y;sample++){
        uint rng=hash((p.y*f.sizes.x+p.x)*747796405u+(f.transport.w+sample)*2891336453u);float2 jitter=float2(random(rng),random(rng));
        float3 origin=f.camera.xyz,direction=cameraRay(f,float2(p)+jitter),throughput=1;
        for(uint bounce=0;bounce<f.transport.z;bounce++){
            Hit hit;if(!trace(f,origin,direction,10000,rng,hit)){result+=throughput*environment(f,direction);break;}
            Surface s=surface(f,hit.id,hit.primitive,hit.bary);if(dot(s.normal,-direction)<0)s.normal=-s.normal;
            float3 lighting=s.emissive+direct(f,s,-direction,rng);result+=throughput*lighting;float3 weight;direction=sampleBrdf(s,-direction,rng,weight);throughput*=weight;
            if(any(!isfinite(throughput)) || max(max(throughput.x,throughput.y),throughput.z)<1e-8)break;
            if(bounce>=3){float survive=clamp(max(max(throughput.x,throughput.y),throughput.z),0.05,0.95);if(random(rng)>survive)break;throughput/=survive;}
            origin=rayOrigin(s,direction);
        }
    }store(f,RAW,p,float4(result/max(f.transport.y,1),1));
}
