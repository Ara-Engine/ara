#include "common.slangh"
[shader("compute")][numthreads(8,8,1)]
void primaryMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint rng=hash(p.y*f.sizes.x+p.x+f.control.w*7919);
    float3 ray=cameraRay(f,float2(p)+0.5);Hit hit;uint slot=f.control.x;
    if(!trace(f,f.camera.xyz,ray,10000,rng,hit)){store(f,WORLD+slot,p,0);store(f,NORMAL+slot,p,0);store(f,BASE,p,0);store(f,MOTION,p,0);store(f,DEPTH,p,0);return;}
    Surface s=surface(f,hit.id,hit.primitive,hit.bary);if(dot(s.normal,-ray)<0)s.normal=-s.normal;
    float4 previous=mul(f.previousVp,float4(s.previousPosition,1));float4 current=mul(f.vp,float4(s.position,1));
    float2 previousUv=float2(previous.x,-previous.y)/previous.w*0.5+0.5;float2 currentUv=float2(current.x,-current.y)/current.w*0.5+0.5;
    store(f,WORLD+slot,p,float4(s.position,float(s.id+1)));store(f,NORMAL+slot,p,float4(s.normal,s.roughness));store(f,BASE,p,float4(s.normal,hit.distance));
    store(f,MOTION,p,float4(previousUv-currentUv,length(s.previousPosition-f.previousCamera.xyz),max(max(s.emissive.x,s.emissive.y),s.emissive.z)>1?1:0));
    store(f,DEPTH,p,float4(current.w,0,0,0));
}
[shader("compute")][numthreads(8,8,1)]
void hybridMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint rng=hash(p.y*f.sizes.x+p.x+f.control.w*7919);
    float3 ray=cameraRay(f,float2(p)+0.5);Hit hit;
    if(!trace(f,f.camera.xyz,ray,10000,rng,hit)){store(f,RAW,p,float4(environment(f,ray),1));return;}
    Surface s=surface(f,hit.id,hit.primitive,hit.bary);if(dot(s.normal,-ray)<0)s.normal=-s.normal;float3 v=-ray;
    float3 colour=s.emissive+direct(f,s,v,rng)+indirect(f,s,v,f.probes.y);
    float3 F=fresnel(max(dot(s.normal,v),0),lerp(float3(0.04),s.albedo,s.metal));
    if(s.roughness<=f.temporal.w){
        // GGX NDF importance sample, bounded to one selective specular ray.
        float u=random(rng),phi=random(rng)*2*PI,a=s.roughness*s.roughness;float ct=sqrt((1-u)/(1+(a*a-1)*u));
        float3 h=basis(s.normal,float3(sqrt(1-ct*ct)*cos(phi),sqrt(1-ct*ct)*sin(phi),ct));float3 l=reflect(-v,h);float nl=dot(s.normal,l);
        if(nl>0){float3 incoming;Hit reflected;
            if(trace(f,s.position+s.normal*0.003,l,200,rng,reflected)){
                Surface r=surface(f,reflected.id,reflected.primitive,reflected.bary);if(dot(r.normal,-l)<0)r.normal=-r.normal;
                incoming=r.emissive+direct(f,r,-l,rng)+indirect(f,r,-l,f.probes.y);
            }else incoming=environment(f,l);
            float nv=max(dot(s.normal,v),1e-5),a2=a*a;float g=2*nv/(nv+sqrt(a2+(1-a2)*nv*nv))*2*nl/max(nl+sqrt(a2+(1-a2)*nl*nl),1e-6);
            colour+=incoming*fresnel(max(dot(v,h),0),lerp(float3(0.04),s.albedo,s.metal))*g*max(dot(v,h),0)/max(nv*dot(s.normal,h),1e-5);
        }
    }else colour+=environment(f,reflect(-v,s.normal))*F*(1-0.5*s.roughness);
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
            result+=throughput*(s.emissive+direct(f,s,-direction,rng));float3 weight;direction=sampleBrdf(s,-direction,rng,weight);throughput*=weight;
            if(any(!isfinite(throughput)) || max(max(throughput.x,throughput.y),throughput.z)<1e-8)break;
            if(bounce>=3){float survive=clamp(max(max(throughput.x,throughput.y),throughput.z),0.05,0.95);if(random(rng)>survive)break;throughput/=survive;}
            origin=s.position+s.normal*0.003;
        }
    }store(f,RAW,p,float4(result/max(f.transport.y,1),1));
}
