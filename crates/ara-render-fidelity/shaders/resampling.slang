#define EMISSIVE_LIGHTS 1
#define CACHE_EXTENSION 1
#include "transport.slang"
static const uint SPEC_RAW=28;
struct ReservoirFrame {uint4 di;uint4 gi;};
ReservoirFrame reservoirFrame(){ReservoirFrame r;r.di=extensionRow(0);r.gi=extensionRow(1);return r;}
struct DIReservoir {uint light;uint seed;float W;float M;float visibility;float owner;float age;float pad;};
struct GIReservoir {float4 position;float4 normal;float4 radiance;float4 stats;};
DIReservoir readDI(uint buffer,uint p){return buffers[NonUniformResourceIndex(buffer)].Load<DIReservoir>(p*32);}
GIReservoir readGI(uint buffer,uint p){return buffers[NonUniformResourceIndex(buffer)].Load<GIReservoir>(p*64);}
void updateDI(inout DIReservoir r,DIReservoir candidate,float weight,float count,inout uint rng){
    float sum=r.W+weight,m=r.M+count;
    if(r.M==0 || (weight>0 && random(rng)*sum<weight))r=candidate;
    r.W=sum;r.M=m;
}
void updateGI(inout GIReservoir r,GIReservoir candidate,float weight,float count,inout uint rng){
    float sum=r.stats.x+weight,m=r.stats.y+count;
    if(r.stats.y==0 || (weight>0 && random(rng)*sum<weight))r=candidate;
    r.stats.x=sum;r.stats.y=m;
}
float3 diffuseValue(Surface s,float3 v,float3 l){
    float nl=max(dot(s.normal,l),0);float3 f0=lerp(float3(0.04*s.specular),s.albedo,s.metal);
    float3 F=fresnel(max(dot(normalize(v+l),v),0),f0);
    float3 value=s.albedo*(1-s.metal)*(1-F)*(nl/PI);
    if(s.coat>0)value*=1-s.coat*fresnel(max(dot(s.normal,v),0),float3(0.04)).x;
    if(any(s.sheen>0)){float energy=sheenAlbedo(s,saturate(dot(s.normal,v)));value*=1-max(max(s.sheen.x,s.sheen.y),s.sheen.z)*energy;}
    return value*saturate(nl+2*s.ao-1);
}
bool layeredMaterial(Surface s){return s.coat>0 || any(s.sheen>0) || s.transmission>0;}
float3 giValue(Surface s,float3 v,GIReservoir r,out float3 direction,out float distance){
    float geometry=1;
    if(r.position.w>0.5){direction=r.position.xyz;distance=10000;}
    else{float3 delta=r.position.xyz-s.position;distance=length(delta);direction=delta/max(distance,1e-6);geometry=abs(dot(r.normal.xyz,-direction))/max(distance*distance,1e-12);}
    return r.radiance.rgb*diffuseValue(s,v,direction)*geometry;
}
float reconnectionGeometry(Surface s,GIReservoir r){
    if(r.position.w>0.5)return 1;float3 delta=r.position.xyz-s.position;
    float d2=max(dot(delta,delta),1e-12);return abs(dot(r.normal.xyz,-delta*rsqrt(d2)))/d2;
}
bool reconnectable(Surface s,GIReservoir r){
    float ratio=reconnectionGeometry(s,r)/max(r.stats.w,1e-20);
    return r.stats.y>0 && ratio>=0.1 && ratio<=10;
}
bool compatible(Frame f,uint2 p,int2 q,bool previous){
    if(any(q<0) || any(q>=int2(f.sizes.xy)))return false;
    uint slot=f.control.x;float4 a=pixel(f,WORLD+slot,p),b=pixel(f,WORLD+(previous?1-slot:slot),q);
    float4 na=pixel(f,NORMAL+slot,p),nb=pixel(f,NORMAL+(previous?1-slot:slot),q);
    return a.w>0 && a.w==b.w && dot(na.xyz,nb.xyz)>0.95 && abs(na.w-nb.w)<0.12
        && abs(dot(a.xyz-b.xyz,na.xyz))<max(0.01,length(a.xyz-f.camera.xyz)*0.001);
}
Surface primarySurface(Frame f,uint2 p,out float3 v){
    float4 w=pixel(f,WORLD+f.control.x,p),h=pixel(f,BASE,p);v=-cameraRay(f,float2(p)+0.5);
#ifndef MATERIAL_LAYERS
    Surface s=(Surface)0;float4 a=extraPixel(f,35,p),g=extraPixel(f,36,p),n=pixel(f,NORMAL+f.control.x,p);
    s.position=w.xyz;s.id=uint(w.w)-1;s.albedo=a.rgb;s.metal=a.w;s.normal=n.xyz;s.roughness=n.w;s.geometricNormal=g.xyz;s.ao=g.w;s.specular=1;return s;
#else
    Surface s=surface(f,uint(w.w)-1,uint(h.z),h.xy);if(dot(s.normal,v)<0)s.normal=-s.normal;return s;
#endif
}
uint2 gridPhase(Frame f){uint t=f.control.w;uint phase=t^(t>>2)^(t>>4)^(t>>6);return uint2(phase&1,(phase>>1)&1);}
int2 gridPoint(Frame f,int2 p){int2 phase=int2(gridPhase(f));int2 q=max((p-phase)/2,int2(0))*2+phase;return min(q,int2(f.sizes.xy)-1-((int2(f.sizes.xy)-1-phase)&1));}
// Cache a bounded path estimate, not an interpolated probe terminal. In narrow
// low-light rooms, that terminal's bias otherwise survives reservoir averaging.
bool screenRadiance(Frame f,Surface s,float3 view,out float3 value){
    value=0;if(f.transport.w!=1 || f.temporal.x!=0 || f.temporal.y!=0 || s.roughness<0.5 || s.metal>0.1 || layeredMaterial(s))return false;
    // Screen radiance is outgoing toward the old camera. Reusing it for another
    // grazing/view-dependent BRDF direction injects energy into floors and walls.
    float3 oldView=normalize(f.previousCamera.xyz-s.position);
    if(dot(oldView,view)<0.9 || abs(dot(s.normal,oldView)-dot(s.normal,view))>0.1)return false;
    float4 clip=mul(f.previousVp,float4(s.position,1));if(clip.w<=0)return false;
    float2 uv=float2(clip.x,-clip.y)/clip.w*0.5+0.5;int2 at=int2(uv*f.sizes.xy+f.jitter.zw);
    if(any(at<0) || any(at>=int2(f.sizes.xy)))return false;
    float4 world=pixel(f,WORLD+1-f.control.x,at),normal=pixel(f,NORMAL+1-f.control.x,at),h=pixel(f,HISTORY+1-f.control.x,at);
    if(world.w!=float(s.id+1) || distance(world.xyz,s.position)>max(0.015,clip.w*0.002) || abs(dot(normal.xyz,s.normal))<0.99 || h.w<16)return false;
    float4 moments=pixel(f,MOMENTS+1-f.control.x,at),specMoments=extraPixel(f,31+1-f.control.x,at);
    if(mixedCoverage(specMoments.z))return false;
    uint packed=asuint(moments.z);float3 modulation=max(float3(packed&1023,(packed>>10)&1023,(packed>>20)&1023)/1023.,float3(0.04));
    float3 direct=historyColour(pixel(f,DIRECT_HISTORY+1-f.control.x,at).rgb,historyPayload(specMoments.z)&((1u<<27)-1),9);
    float3 diffuse=historyColour(h.rgb,historyPayload(moments.w),10);
    float3 specular=historyColour(extraPixel(f,29+1-f.control.x,at).rgb,historyPayload(specMoments.w),10);
    value=max(0,direct-s.emissive)+diffuse*modulation+specular;
    return all(isfinite(value));
}
float3 cachedPath(Frame f,Surface s,float3 v,inout uint rng){
    float3 result=0,beta=1;
    for(uint bounce=0;bounce<11;bounce++){
        float3 reused;if(screenRadiance(f,s,v,reused)){result+=beta*reused;break;}
        result+=beta*direct(f,s,v,rng);
        if(bounce==10)break;
        float3 weight;float3 l=sampleBrdf(s,v,rng,weight);beta*=weight;
        if(any(!isfinite(beta)) || max(max(beta.x,beta.y),beta.z)<1e-7)break;
        if(bounce>=2){float survival=clamp(max(max(beta.x,beta.y),beta.z),0.05,0.95);if(random(rng)>survival)break;beta/=survival;}
        Hit hit;if(!trace(f,rayOrigin(s,l),l,10000,rng,hit)){result+=beta*environment(f,l);break;}
        s=surface(f,hit.id,hit.primitive,hit.bary);if(dot(s.normal,-l)<0)s.normal=-s.normal;v=-l;
    }return result;
}
float3 cachedRay(Frame f,Surface s,float3 direction,inout uint rng){
    Hit hit;if(!trace(f,rayOrigin(s,direction),direction,10000,rng,hit))return environment(f,direction);
    Surface second=surface(f,hit.id,hit.primitive,hit.bary);if(dot(second.normal,-direction)<0)second.normal=-second.normal;
    return cachedPath(f,second,-direction,rng);
}
// Independent candidates plus reprojected prior reservoirs. Re-evaluate the target
// at the receiving surface; retain the original proposal normalization and count.
[shader("compute")][numthreads(8,8,1)]
void reservoirInit(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();ReservoirFrame reservoir=reservoirFrame();uint2 p=tid.xy*2+gridPhase(f);if(any(p>=f.sizes.xy))return;uint index=p.y*f.sizes.x+p.x,rng=hash(index+f.control.w*7919);
    DIReservoir dr=(DIReservoir)0;dr.light=0xffffffff;dr.owner=float(index);
    GIReservoir gr=(GIReservoir)0;
    if(pixel(f,WORLD+f.control.x,p).w==0){
        buffers[NonUniformResourceIndex(reservoir.di.y)].Store(index*32,dr);
        buffers[NonUniformResourceIndex(reservoir.gi.y)].Store(index*64,gr);return;
    }
    float3 v;Surface s=primarySurface(f,p,v);
    for(uint i=0;i<1;i++){
        DIReservoir c=(DIReservoir)0;c.light=chooseLight(f,rng);c.seed=hash(rng^0xa511e9b3u);random(rng);c.owner=float(index);c.visibility=-1;
        LightCandidate light=lightAt(f,s,v,c.light,c.seed);updateDI(dr,c,lightTarget(light.value)/max(light.pdf,1e-20),1,rng);
    }
    float2 uv;int2 at=int2(p);bool history=false;
    if(f.temporal.x==0 && f.temporal.y==0 && (reservoir.gi.w&1)==0){history=reproject(f,p,uv,at);if(history)history=compatible(f,p,at,true);}
    if(history){
        DIReservoir old=readDI(reservoir.di.x,uint(at.y)*f.sizes.x+uint(at.x));float m=min(old.M,16.);
        float t=lightTarget(lightAt(f,s,v,old.light,old.seed).value);
        // Only exactly matching receivers reuse visibility; moving or jittered
        // receivers re-test the selected light, and caches expire after eight frames.
        if(length(pixel(f,WORLD+1-f.control.x,at).xyz-s.position)>0.0001 || old.age>=8)old.visibility=-1;
        old.owner=float(index);old.age+=1;updateDI(dr,old,t*old.W*m,m,rng);
    }
    float target=lightTarget(lightAt(f,s,v,dr.light,dr.seed).value);dr.W=target>0?dr.W/max(dr.M*target,1e-20):0;
    buffers[NonUniformResourceIndex(reservoir.di.y)].Store(index*32,dr);
    if(history){
        GIReservoir old=readGI(reservoir.gi.x,uint(at.y)*f.sizes.x+uint(at.x));
        uint owner=uint(old.normal.w);float4 source=pixel(f,WORLD+1-f.control.x,int2(owner%f.sizes.x,owner/f.sizes.x));
        bool local=source.w==float(s.id+1) && distance(source.xyz,s.position)<0.025;
        if(!layeredMaterial(s) && (old.radiance.w==0 || local) && old.stats.z<12 && reconnectable(s,old)){
            gr=old;gr.stats.y=min(gr.stats.y,12.);gr.stats.z+=1;gr.stats.w=reconnectionGeometry(s,gr);
        }
    }
    buffers[NonUniformResourceIndex(reservoir.gi.y)].Store(index*64,gr);

}
// Compact trace dispatch keeps all lanes tracing, rather than leaving three
// quarters idle inside the full-rate direct-reservoir kernel.
[shader("compute")][numthreads(8,8,1)]
void reservoirTrace(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();ReservoirFrame reservoir=reservoirFrame();uint2 p=tid.xy;uint stride=max(reservoir.gi.w>>1,2);
    float sampleWeight=f.temporal.x>0 || f.temporal.y>0?4.:4.*stride;
    uint phase=f.control.w^(f.control.w>>2)^(f.control.w>>4);
    if(f.temporal.x==0 && f.temporal.y==0)p.x=p.x*stride+((p.y+phase)&(stride-1));
    p=p*2+gridPhase(f);
    if(any(p>=f.sizes.xy) || pixel(f,WORLD+f.control.x,p).w==0)return;
    uint index=p.y*f.sizes.x+p.x,rng=hash(index+f.control.w*7919);float3 v;Surface s=primarySurface(f,p,v);
    if(layeredMaterial(s)){
        float3 weight;float3 direction=sampleBrdf(s,v,rng,weight);
        extraStore(f,SPEC_RAW,p,float4(cachedRay(f,s,direction,rng)*weight,sampleWeight));
        GIReservoir empty=(GIReservoir)0;buffers[NonUniformResourceIndex(reservoir.gi.y)].Store(index*64,empty);return;
    }
    GIReservoir gr=readGI(reservoir.gi.y,index);float3 oldDirection;float oldDistance;
    float target=lightTarget(giValue(s,v,gr,oldDirection,oldDistance));gr.stats.x*=gr.stats.y*target;
    bool fresh=true;float3 specular=0;
    if(fresh && (s.metal<1 || s.roughness>=0.5)){
        float3 weight=1;float proposal;bool shared=s.roughness>=0.5;
        float3 l;if(shared)l=sampleBrdfPdf(s,v,rng,weight,proposal);else{l=cosineDirection(s.normal,rng);proposal=max(dot(s.normal,l),0)/PI;}float omegaProposal=proposal;
        Hit hit;
        GIReservoir c=(GIReservoir)0;c.normal.w=float(index);
        if(proposal>0 && any(weight>0) && trace(f,rayOrigin(s,l),l,200,rng,hit)){
            Surface second=surface(f,hit.id,hit.primitive,hit.bary);if(dot(second.normal,-l)<0)second.normal=-second.normal;
            c.position=float4(second.position,0);c.normal.xyz=second.geometricNormal;
            c.radiance=float4(cachedPath(f,second,-l,rng),second.roughness<0.3 || second.metal>0.8?1:0);
            proposal*=abs(dot(second.geometricNormal,-l))/max(dot(second.position-s.position,second.position-s.position),1e-12);
        }else{c.position=float4(l,1);c.radiance=float4(proposal>0 && any(weight>0)?environment(f,l):float3(0),0);}
        if(shared)specular=c.radiance.rgb*max(weight-diffuseValue(s,v,l)/max(omegaProposal,1e-20),float3(0));
        c.stats.w=reconnectionGeometry(s,c);
        float3 direction;float distance;float target=lightTarget(giValue(s,v,c,direction,distance));
        if(s.metal<1)updateGI(gr,c,target/max(proposal,1e-20),1,rng);
    }
    float3 direction;float distance;target=lightTarget(giValue(s,v,gr,direction,distance));gr.stats.x=target>0?gr.stats.x/max(gr.stats.y*target,1e-20):0;
    gr.stats.w=reconnectionGeometry(s,gr);
    buffers[NonUniformResourceIndex(reservoir.gi.y)].Store(index*64,gr);
    extraStore(f,SPEC_RAW,p,float4(specular,fresh?sampleWeight:0));
}

// Glossy primary pixels need their own full-rate samples; coarse diffuse-path
// candidates cannot carry the view-dependent GGX lobe without visible speckles.
[shader("compute")][numthreads(8,8,1)]
void specularTrace(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy) || pixel(f,WORLD+f.control.x,p).w==0 || pixel(f,NORMAL+f.control.x,p).w>=0.5)return;
    uint rng=hash((p.y*f.sizes.x+p.x)^f.control.w*8737);float3 v;Surface s=primarySurface(f,p,v);if(layeredMaterial(s))return;
    float3 h=visibleGgx(s,v,rng),l=reflect(-v,h),result=0;float nl=dot(s.normal,l);
    if(nl>0)result=cachedRay(f,s,l,rng)*fresnel(max(dot(v,h),0),lerp(float3(0.04*s.specular),s.albedo,s.metal))*smith(nl,pow(s.roughness,4))*saturate(nl+2*s.ao-1);
    extraStore(f,SPEC_RAW,p,float4(result,1));
}

// Reconnect diffuse paths in area measure; cos(theta_secondary)/distance^2 is
// evaluated at each receiver, providing the reconnection Jacobian automatically.
[shader("compute")][numthreads(8,8,1)]
void reservoirSpatial(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();ReservoirFrame reservoir=reservoirFrame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint index=p.y*f.sizes.x+p.x,rng=hash(index+f.control.w*4567);
    if(pixel(f,WORLD+f.control.x,p).w==0){extraStore(f,SPEC_RAW,p,float4(0,0,0,1));return;}
    float3 v;Surface s=primarySurface(f,p,v);DIReservoir dr=(DIReservoir)0;dr.light=0xffffffff;GIReservoir gr=(GIReservoir)0;
    for(uint i=0;i<2;i++){
        int2 q=int2(p);if(i>0){float angle=random(rng)*2*PI;float radius=1+random(rng)*3;q+=int2(cos(angle)*radius,sin(angle)*radius);}q=gridPoint(f,q);
        if(!compatible(f,p,q,false))continue;uint owner=uint(q.y)*f.sizes.x+uint(q.x);
        DIReservoir d=readDI(reservoir.di.y,owner);float m=min(d.M,20.);
        float target=lightTarget(lightAt(f,s,v,d.light,d.seed).value);if(i>0)d.visibility=-1;
        updateDI(dr,d,target*d.W*m,m,rng);
        if(layeredMaterial(s))continue;
        GIReservoir g=readGI(reservoir.gi.y,owner);
        if((i>0 && g.radiance.w>0) || !reconnectable(s,g)){gr.stats.y+=min(g.stats.y,16.);continue;}
        float3 direction;float distance;target=lightTarget(giValue(s,v,g,direction,distance));m=min(g.stats.y,16.);
        updateGI(gr,g,target*g.stats.x*m,m,rng);
    }
    if(dr.M==0){
        float3 local=risDirect(f,s,v,rng),indirect=0;
        // Specular samples keep their fixed dense/coarse inclusion probability.
        // Adding geometry-dependent fallback samples would bias pixel coverage.
        if(!layeredMaterial(s) && s.metal<1){float3 direction=cosineDirection(s.normal,rng);indirect=cachedRay(f,s,direction,rng)*diffuseValue(s,v,direction)/max(dot(s.normal,direction)/PI,1e-20);}
        store(f,DIRECT,p,float4(pixel(f,DIRECT,p).rgb+local,1));store(f,RAW,p,float4(indirect/demodulation(f,p),1));
        buffers[NonUniformResourceIndex(reservoir.di.z)].Store(index*32,dr);buffers[NonUniformResourceIndex(reservoir.gi.z)].Store(index*64,gr);return;
    }
    LightCandidate selected=lightAt(f,s,v,dr.light,dr.seed);float target=lightTarget(selected.value);
    dr.W=target>0?dr.W/max(dr.M*target,1e-20):0;
    if(dr.W>0 && dr.visibility<0){dr.visibility=occluded(f,rayOrigin(s,selected.direction),selected.direction,selected.distance,rng)?0:1;dr.age=0;}
    dr.owner=float(index);buffers[NonUniformResourceIndex(reservoir.di.z)].Store(index*32,dr);
    store(f,DIRECT,p,float4(pixel(f,DIRECT,p).rgb+selected.value*dr.W*max(dr.visibility,0),1));
    float3 direction;float distance;float3 gi=giValue(s,v,gr,direction,distance);target=lightTarget(gi);
    gr.stats.x=target>0?gr.stats.x/max(gr.stats.y*target,1e-20):0;
    gr.stats.w=reconnectionGeometry(s,gr);
    if(gr.stats.x>0 && occluded(f,rayOrigin(s,direction),direction,max(distance-0.003,0.001),rng))gi=0;
    buffers[NonUniformResourceIndex(reservoir.gi.z)].Store(index*64,gr);
    store(f,RAW,p,float4(gi*gr.stats.x/demodulation(f,p),1));
}
