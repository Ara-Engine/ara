#include "common.slangh"
[shader("compute")][numthreads(8,8,1)]
void primaryMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint rng=hash(p.y*f.sizes.x+p.x+f.control.w*7919);
#ifdef RESERVOIR_SURFACES
    extraStore(f,28,p,float4(0,0,0,0));
#endif
    float3 ray=cameraRay(f,float2(p)+0.5);Hit hit;uint slot=f.control.x;
    if(!trace(f,f.camera.xyz,ray,10000,rng,hit)){
#ifdef RESERVOIR_SURFACES
        extraStore(f,28,p,float4(0,0,0,1));
#endif
        store(f,WORLD+slot,p,0);store(f,NORMAL+slot,p,0);store(f,BASE,p,0);store(f,MOTION,p,0);store(f,DEPTH,p,0);if(f.transport.x==0){store(f,DIRECT,p,0);store(f,RAW,p,float4(environment(f,ray),1));}return;}
    Surface s=surface(f,hit.id,hit.primitive,hit.bary);if(dot(s.normal,-ray)<0)s.normal=-s.normal;
#ifdef RESERVOIR_SURFACES
    extraStore(f,35,p,float4(s.albedo,s.metal));extraStore(f,36,p,float4(s.geometricNormal,s.ao));
#endif
    float3 previousWorld=previousPosition(f,hit.id,hit.primitive,hit.bary,s.position);
    float4 previous=mul(f.previousVp,float4(previousWorld,1));float4 current=mul(f.vp,float4(s.position,1));
    float2 previousUv=float2(previous.x,-previous.y)/previous.w*0.5+0.5;float2 currentUv=float2(current.x,-current.y)/current.w*0.5+0.5;
    store(f,WORLD+slot,p,float4(s.position,float(s.id+1)));store(f,NORMAL+slot,p,float4(s.normal,s.roughness));uint3 modulation=uint3(saturate(s.albedo*(1-s.metal)+lerp(float3(0.04*s.specular),s.albedo,s.metal))*255+0.5);
    store(f,BASE,p,float4(hit.bary,float(hit.primitive),float(modulation.x|(modulation.y<<8)|(modulation.z<<16))));
    store(f,MOTION,p,float4(previousUv-currentUv,length(previousWorld-f.previousCamera.xyz),max(max(s.emissive.x,s.emissive.y),s.emissive.z)>1?1:0));
    // DEPTH.y carries imported ambient occlusion to the indirect remodulation.
    store(f,DEPTH,p,float4(current.w,s.ao,0,0));
    if(f.transport.x==0){float3 lighting=primarySun(f,s,-ray,p,rng);
#ifndef RESERVOIR_SURFACES
        lighting+=directLocal(f,s,-ray,rng);
#endif
        store(f,DIRECT,p,float4(s.emissive+lighting,1));store(f,RAW,p,0);}
}
float3 gather(Frame f,Surface s,float3 direction,inout uint rng){
    float3 result=0,throughput=1;
    for(uint bounce=0;bounce<((f.transport.z>>1)&15);bounce++){
        Hit hit;if(!trace(f,rayOrigin(s,direction),direction,200,rng,hit)){result+=throughput*environment(f,direction);break;}
        Surface r=surface(f,hit.id,hit.primitive,hit.bary);if(dot(r.normal,-direction)<0)r.normal=-r.normal;
        result+=throughput*(direct(f,r,-direction,rng));
        if(bounce+1==((f.transport.z>>1)&15)){result+=throughput*indirect(f,r,-direction,f.probes.y);break;}
        float3 weight;direction=sampleBrdf(r,-direction,rng,weight);
        throughput*=weight;s=r;
    }return result;
}
[shader("compute")][numthreads(8,8,1)]
void hybridMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;uint t=f.control.w;
    // XOR-fold the frame digits so each transport phase covers every horizontal
    // Halton stratum, instead of repeatedly selecting one subpixel quadrant.
    uint halfPhase=t^(t>>1)^(t>>2)^(t>>3)^(t>>4)^(t>>5)^(t>>6)^(t>>7);
    uint quarterPhase=t^(t>>2)^(t>>4)^(t>>6);
    if(root.pass==1 || root.pass==2)p.x=p.x*2+((p.y+halfPhase+(root.pass==2?1:0))&1);
    if(root.pass==3)p.x=p.x*4+((p.y+quarterPhase)&3);
    if(any(p>=f.sizes.xy) || (root.pass==4 && ((p.x-p.y-quarterPhase)&3)==0))return;
    uint rng=hash(p.y*f.sizes.x+p.x+f.control.w*7919);
    float2 oldUv;int2 oldAt;
    if((root.pass==2 || root.pass==4) && reproject(f,p,oldUv,oldAt))return;
    float4 world=pixel(f,WORLD+f.control.x,p);float3 ray=cameraRay(f,float2(p)+0.5);
    if(world.w==0)return;
    float4 hit=pixel(f,BASE,p);Surface s=surface(f,uint(world.w)-1,uint(hit.z),hit.xy);if(dot(s.normal,-ray)<0)s.normal=-s.normal;float3 v=-ray;
    float3 gi=0;
    float3 f0=lerp(float3(0.04*s.specular),s.albedo,s.metal);
    if(s.coat>0 || any(s.sheen>0) || s.transmission>0){float3 weight;float3 l=sampleBrdf(s,v,rng,weight);float3 colour=gather(f,s,l,rng)*weight;store(f,RAW,p,float4(max(colour,0)/demodulation(f,p),1));return;}
    // One importance-sampled lobe, with its selection PDF, estimates the same
    // diffuse + roughness-faded specular sum as the split-lobe room preset.
    if((f.transport.z&32)!=0){
        float traced=1-smoothstep(max(0,f.temporal.w-0.1),max(0.001,f.temporal.w),s.roughness);
        float3 viewF=fresnel(max(dot(s.normal,v),0),f0);
        float probability=traced>0?clamp(max(max(viewF.x,viewF.y),viewF.z),0.125,s.metal>=1?1.:0.875):0;
        float3 colour=0;
        if(random(rng)<probability){
            float3 h=visibleGgx(s,v,rng),l=reflect(-v,h);float nl=dot(s.normal,l);
            if(nl>0)colour=gather(f,s,l,rng)*fresnel(max(dot(v,h),0),f0)*smith(nl,pow(s.roughness,4))*traced/probability;
        }else if(s.metal<1){
            float3 l=cosineDirection(s.normal,rng),F=fresnel(max(dot(normalize(v+l),v),0),f0);
            colour=gather(f,s,l,rng)*s.albedo*(1-s.metal)*(1-F)/(1-probability);
        }
        if(traced<1)colour+=irradiance(f,s.position,reflect(-v,s.normal),f.probes.y)*fresnel(sqrt(0.5+0.5*saturate(dot(s.normal,v))),f0)*(1-traced)/PI;
        store(f,RAW,p,float4(max(colour,0)/demodulation(f,p),1));return;
    }
    if(s.metal<1){float3 l=cosineDirection(s.normal,rng);float3 F=fresnel(max(dot(normalize(v+l),v),0),f0);
        gi+=gather(f,s,l,rng)*s.albedo*(1-s.metal)*(1-F);
    }
    // Visible-normal reflection weights are bounded by Fresnel, including at the rim.
    float3 h=visibleGgx(s,v,rng),l=reflect(-v,h);float nl=dot(s.normal,l);
    float traced=1-smoothstep(max(0,f.temporal.w-0.1),max(0.001,f.temporal.w),s.roughness);float3 specular=0;
    if(traced>0 && nl>0)specular=gather(f,s,l,rng)*fresnel(max(dot(v,h),0),f0)*smith(nl,pow(s.roughness,4))*traced;
    if(traced<1)specular+=irradiance(f,s.position,reflect(-v,s.normal),f.probes.y)*fresnel(sqrt(0.5+0.5*saturate(dot(s.normal,v))),f0)*(1-traced)/PI;
    float3 colour=gi+specular;
    store(f,RAW,p,float4(max(colour,0)/demodulation(f,p),1));
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
            float3 lighting=(bounce==0?s.emissive:float3(0))+direct(f,s,-direction,rng);result+=throughput*lighting;float3 weight;direction=sampleBrdf(s,-direction,rng,weight);throughput*=weight;
            if(any(!isfinite(throughput)) || max(max(throughput.x,throughput.y),throughput.z)<1e-8)break;
            if(bounce>=3){float survive=clamp(max(max(throughput.x,throughput.y),throughput.z),0.05,0.95);if(random(rng)>survive)break;throughput/=survive;}
            origin=rayOrigin(s,direction);
        }
    }store(f,RAW,p,float4(result/max(f.transport.y,1),1));
}
