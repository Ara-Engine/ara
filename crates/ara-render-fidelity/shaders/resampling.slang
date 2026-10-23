#define EMISSIVE_LIGHTS 1
#include "transport.slang"
static const uint SPEC_RAW=28;
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
    Surface s=surface(f,uint(w.w)-1,uint(h.z),h.xy);if(dot(s.normal,v)<0)s.normal=-s.normal;return s;
}
// Cache a bounded path estimate, not an interpolated probe terminal. In narrow
// low-light rooms, that terminal's bias otherwise survives reservoir averaging.
float3 cachedPath(Frame f,Surface s,float3 v,inout uint rng){
    float3 result=0,beta=1;
    for(uint bounce=0;bounce<6;bounce++){
        result+=beta*direct(f,s,v,rng);
        if(bounce==5)break;
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
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint index=p.y*f.sizes.x+p.x,rng=hash(index+f.control.w*7919);
    DIReservoir dr=(DIReservoir)0;dr.light=0xffffffff;dr.owner=float(index);
    GIReservoir gr=(GIReservoir)0;
    if(pixel(f,WORLD+f.control.x,p).w==0){
        buffers[NonUniformResourceIndex(f.reservoirs[0].y)].Store(index*32,dr);
        buffers[NonUniformResourceIndex(f.reservoirs[1].y)].Store(index*64,gr);extraStore(f,SPEC_RAW,p,float4(0,0,0,1));return;
    }
    float3 v;Surface s=primarySurface(f,p,v);
    for(uint i=0;i<2;i++){
        DIReservoir c=(DIReservoir)0;c.light=chooseLight(f,rng);c.seed=hash(rng^0xa511e9b3u);random(rng);c.owner=float(index);c.visibility=-1;
        LightCandidate light=lightAt(f,s,v,c.light,c.seed);updateDI(dr,c,lightTarget(light.value)/max(light.pdf,1e-20),1,rng);
    }
    float2 uv;int2 at=int2(p);bool history=false;
    if(f.temporal.x==0 && f.temporal.y==0 && (f.reservoirs[1].w&1)==0){history=reproject(f,p,uv,at);if(history)history=compatible(f,p,at,true);}
    if(history){
        DIReservoir old=readDI(f.reservoirs[0].x,uint(at.y)*f.sizes.x+uint(at.x));float m=min(old.M,16.);
        float t=lightTarget(lightAt(f,s,v,old.light,old.seed).value);
        // Only exactly matching receivers reuse visibility; moving or jittered
        // receivers re-test the selected light, and caches expire after eight frames.
        if(length(pixel(f,WORLD+1-f.control.x,at).xyz-s.position)>0.0001 || old.age>=8)old.visibility=-1;
        old.owner=float(index);old.age+=1;updateDI(dr,old,t*old.W*m,m,rng);
    }
    float target=lightTarget(lightAt(f,s,v,dr.light,dr.seed).value);dr.W=target>0?dr.W/max(dr.M*target,1e-20):0;
    buffers[NonUniformResourceIndex(f.reservoirs[0].y)].Store(index*32,dr);
    extraStore(f,SPEC_RAW,p,float4(0,0,0,0));
    if(history){
        GIReservoir old=readGI(f.reservoirs[1].x,uint(at.y)*f.sizes.x+uint(at.x));
        if(!layeredMaterial(s) && old.radiance.w==0 && old.stats.z<12 && reconnectable(s,old)){
            gr=old;gr.stats.y=min(gr.stats.y,12.);gr.stats.z+=1;gr.stats.w=reconnectionGeometry(s,gr);
        }
    }
    buffers[NonUniformResourceIndex(f.reservoirs[1].y)].Store(index*64,gr);

}
// Compact trace dispatch keeps all lanes tracing, rather than leaving three
// quarters idle inside the full-rate direct-reservoir kernel.
[shader("compute")][numthreads(8,8,1)]
void reservoirTrace(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;uint stride=max(f.reservoirs[1].w>>1,2);
    uint phase=f.control.w^(f.control.w>>2)^(f.control.w>>4);
    if(f.temporal.x==0 && f.temporal.y==0)p.x=p.x*stride+((p.y+phase)&(stride-1));
    if(any(p>=f.sizes.xy) || pixel(f,WORLD+f.control.x,p).w==0)return;
    uint index=p.y*f.sizes.x+p.x,rng=hash(index+f.control.w*7919);float3 v;Surface s=primarySurface(f,p,v);
    if(layeredMaterial(s)){
        float3 weight;float3 direction=sampleBrdf(s,v,rng,weight);
        extraStore(f,SPEC_RAW,p,float4(cachedRay(f,s,direction,rng)*weight,1));
        GIReservoir empty=(GIReservoir)0;buffers[NonUniformResourceIndex(f.reservoirs[1].y)].Store(index*64,empty);return;
    }
    GIReservoir gr=readGI(f.reservoirs[1].y,index);float3 oldDirection;float oldDistance;
    float target=lightTarget(giValue(s,v,gr,oldDirection,oldDistance));gr.stats.x*=gr.stats.y*target;
    bool fresh=true;
    if(fresh && s.metal<1){
        float3 l=cosineDirection(s.normal,rng);Hit hit;float proposal=max(dot(s.normal,l),0)/PI;
        GIReservoir c=(GIReservoir)0;c.normal.w=float(index);
        if(trace(f,rayOrigin(s,l),l,200,rng,hit)){
            Surface second=surface(f,hit.id,hit.primitive,hit.bary);if(dot(second.normal,-l)<0)second.normal=-second.normal;
            c.position=float4(second.position,0);c.normal.xyz=second.geometricNormal;
            c.radiance=float4(cachedPath(f,second,-l,rng),second.roughness<0.3 || second.metal>0.8?1:0);
            proposal*=abs(dot(second.geometricNormal,-l))/max(dot(second.position-s.position,second.position-s.position),1e-12);
        }else{c.position=float4(l,1);c.radiance=float4(environment(f,l),0);}
        c.stats.w=reconnectionGeometry(s,c);
        float3 direction;float distance;float target=lightTarget(giValue(s,v,c,direction,distance));
        updateGI(gr,c,target/max(proposal,1e-20),1,rng);
    }
    float3 direction;float distance;target=lightTarget(giValue(s,v,gr,direction,distance));gr.stats.x=target>0?gr.stats.x/max(gr.stats.y*target,1e-20):0;
    gr.stats.w=reconnectionGeometry(s,gr);
    buffers[NonUniformResourceIndex(f.reservoirs[1].y)].Store(index*64,gr);
    float3 specular=0;
    if(fresh){
        float3 h=visibleGgx(s,v,rng),l=reflect(-v,h);float nl=dot(s.normal,l);
        float3 f0=lerp(float3(0.04*s.specular),s.albedo,s.metal);
        if(nl>0)specular=cachedRay(f,s,l,rng)*fresnel(max(dot(v,h),0),f0)*smith(nl,pow(s.roughness,4));
    }extraStore(f,SPEC_RAW,p,float4(specular,fresh?1:0));
}

// Reconnect diffuse paths in area measure; cos(theta_secondary)/distance^2 is
// evaluated at each receiver, providing the reconnection Jacobian automatically.
[shader("compute")][numthreads(8,8,1)]
void reservoirSpatial(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint index=p.y*f.sizes.x+p.x,rng=hash(index+f.control.w*4567);
    if(pixel(f,WORLD+f.control.x,p).w==0){extraStore(f,SPEC_RAW,p,float4(0,0,0,1));return;}
    float3 v;Surface s=primarySurface(f,p,v);DIReservoir dr=(DIReservoir)0;dr.light=0xffffffff;GIReservoir gr=(GIReservoir)0;
    for(uint i=0;i<3;i++){
        int2 q=int2(p);if(i>0){float angle=random(rng)*2*PI;float radius=4+random(rng)*12;q+=int2(cos(angle)*radius,sin(angle)*radius);}
        if(!compatible(f,p,q,false))continue;uint owner=uint(q.y)*f.sizes.x+uint(q.x);
        DIReservoir d=readDI(f.reservoirs[0].y,owner);float m=min(d.M,20.);
        float target=lightTarget(lightAt(f,s,v,d.light,d.seed).value);if(i>0)d.visibility=-1;
        updateDI(dr,d,target*d.W*m,m,rng);
        if(layeredMaterial(s))continue;
        GIReservoir g=readGI(f.reservoirs[1].y,owner);if(i>0 && (g.radiance.w>0 || !reconnectable(s,g)))continue;
        float3 direction;float distance;target=lightTarget(giValue(s,v,g,direction,distance));m=min(g.stats.y,16.);
        updateGI(gr,g,target*g.stats.x*m,m,rng);
    }
    LightCandidate selected=lightAt(f,s,v,dr.light,dr.seed);float target=lightTarget(selected.value);
    dr.W=target>0?dr.W/max(dr.M*target,1e-20):0;
    if(dr.W>0 && dr.visibility<0){dr.visibility=occluded(f,rayOrigin(s,selected.direction),selected.direction,selected.distance,rng)?0:1;dr.age=0;}
    dr.owner=float(index);buffers[NonUniformResourceIndex(f.reservoirs[0].z)].Store(index*32,dr);
    store(f,DIRECT,p,float4(pixel(f,DIRECT,p).rgb+selected.value*dr.W*max(dr.visibility,0),1));
    float3 direction;float distance;float3 gi=giValue(s,v,gr,direction,distance);target=lightTarget(gi);
    gr.stats.x=target>0?gr.stats.x/max(gr.stats.y*target,1e-20):0;
    gr.stats.w=reconnectionGeometry(s,gr);
    if(gr.stats.x>0 && occluded(f,rayOrigin(s,direction),direction,max(distance-0.003,0.001),rng))gi=0;
    buffers[NonUniformResourceIndex(f.reservoirs[1].z)].Store(index*64,gr);
    store(f,RAW,p,float4(gi*gr.stats.x/demodulation(f,p),1));
}
