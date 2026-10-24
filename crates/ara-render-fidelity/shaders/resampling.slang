#define EMISSIVE_LIGHTS 1
#define CACHE_EXTENSION 1
#include "transport.slang"
static const uint SPEC_RAW=28;
struct ReservoirFrame {uint4 di;uint4 gi;};
ReservoirFrame reservoirFrame(){ReservoirFrame r;r.di=extensionRow(0);r.gi=extensionRow(1);return r;}
struct DIReservoir {uint light;uint seed;float W;float M;float visibility;float owner;float age;float pad;};
struct GIReservoir {float4 position;float3 normal;uint owner;float4 radiance;float4 stats;};
// Counts/visibility/age fit in 18 exact bits. The unused receiver/padding fields
// stay out of device memory, halving direct-reservoir traffic.
DIReservoir readDI(uint buffer,uint p){
    uint4 a=buffers[buffer].Load<uint4>(p*16);DIReservoir r=(DIReservoir)0;
    r.light=a.x;r.seed=a.y;r.W=asfloat(a.z);r.M=float(a.w&255);r.visibility=float((a.w>>8)&3)-1;r.age=float((a.w>>10)&255);return r;
}
void storeDI(uint buffer,uint p,DIReservoir r){
    uint state=uint(min(r.M,255.)+0.5)|(uint(r.visibility+1)<<8)|(uint(min(r.age,255.))<<10);
    buffers[buffer].Store(p*16,uint4(r.light,r.seed,asuint(r.W),state));
}
float2 octSign(float2 p){return float2(p.x>=0?1.:-1.,p.y>=0?1.:-1.);}
uint packNormal(float3 n){
    float length1=dot(abs(n),float3(1));n=length1>0?n/length1:float3(0,0,1);
    float2 uv=n.z>=0?n.xy:(1-abs(n.yx))*octSign(n.xy);uint2 q=uint2(saturate(uv*0.5+0.5)*65535+0.5);return q.x|(q.y<<16);
}
float3 unpackNormal(uint p){
    float2 uv=float2(p&65535,p>>16)*(2./65535.)-1;float3 n=float3(uv,1-abs(uv.x)-abs(uv.y));
    if(n.z<0)n.xy=(1-abs(n.yx))*octSign(n.xy);return normalize(n);
}
// Positions, radiance and normalization retain float32. Only the unit geometric
// normal uses oct16; integer owner and two flags share a word (max image <2^30 px).
GIReservoir readGI(uint buffer,uint p){
    uint4 a=buffers[buffer].Load<uint4>(p*48),b=buffers[buffer].Load<uint4>(p*48+16);GIReservoir r;
    r.position=float4(asfloat(a.xyz),float(b.w>>31));r.normal=unpackNormal(a.w);r.owner=b.w&0x3fffffff;
    r.radiance=float4(asfloat(b.xyz),float((b.w>>30)&1));r.stats=buffers[buffer].Load<float4>(p*48+32);return r;
}
void storeGI(uint buffer,uint p,GIReservoir r){
    uint flags=r.owner|(uint(r.position.w>0.5)<<31)|(uint(r.radiance.w>0.5)<<30);
    buffers[buffer].Store(p*48,uint4(asuint(r.position.xyz),packNormal(r.normal)));
    buffers[buffer].Store(p*48+16,uint4(asuint(r.radiance.xyz),flags));buffers[buffer].Store(p*48+32,r.stats);
}
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
    return value;
}
bool layeredMaterial(Surface s){return s.coat>0 || any(s.sheen>0) || s.transmission>0;}
float3 giValue(Surface s,float3 v,GIReservoir r,out float3 direction,out float distance){
    float geometry=1;
    if(r.position.w>0.5){direction=r.position.xyz;distance=10000;}
    else{float3 delta=r.position.xyz-s.position;distance=length(delta);direction=delta/max(distance,1e-6);geometry=abs(dot(r.normal,-direction))/max(distance*distance,1e-12);}
    return r.radiance.rgb*diffuseValue(s,v,direction)*geometry;
}
float reconnectionGeometry(Surface s,GIReservoir r){
    if(r.position.w>0.5)return 1;float3 delta=r.position.xyz-s.position;
    float d2=max(dot(delta,delta),1e-12);return abs(dot(r.normal,-delta*rsqrt(d2)))/d2;
}
bool reconnectable(Surface s,GIReservoir r){
    float ratio=reconnectionGeometry(s,r)/max(r.stats.w,1e-20);
    return r.stats.y>0 && ratio>=0.1 && ratio<=10;
}
// Reuse needs the same mesh and the same geometric plane. Normal-mapped shading
// normals and texel roughness vary pixel to pixel on corrugated or rough surfaces;
// requiring them to match rejected most reuse there and sent those pixels down the
// full-path repair. The receiver re-evaluates every reused target with its own BRDF,
// so only creases (loose shading-normal bound) and large roughness jumps reject.
bool compatible(Frame f,uint2 p,int2 q,bool previous){
    if(any(q<0) || any(q>=int2(f.sizes.xy)))return false;
    uint slot=f.control.x;float4 a=pixel(f,WORLD+slot,p),b=pixel(f,WORLD+(previous?1-slot:slot),q);
    float4 na=pixel(f,NORMAL+slot,p),nb=pixel(f,NORMAL+(previous?1-slot:slot),q);float3 g=extraPixel(f,36,p).xyz;
    bool plane=abs(dot(a.xyz-b.xyz,g))<max(0.01,length(a.xyz-f.camera.xyz)*0.002);
    if(!previous)plane=plane && dot(g,extraPixel(f,36,q).xyz)>0.9;
    return a.w>0 && a.w==b.w && dot(na.xyz,nb.xyz)>0.5 && abs(na.w-nb.w)<0.4 && plane;
}
// Plain pixels rebuild their surface from the primary pass's compact targets; only
// layered ones (tagged AO >= 2) re-evaluate the material. Layer-capable variants
// (atmosphere/layers) used to re-evaluate every pixel in each of four passes.
Surface primarySurface(Frame f,uint2 p,out float3 v){
    float4 w=pixel(f,WORLD+f.control.x,p),h=pixel(f,BASE,p),g=extraPixel(f,36,p);v=-cameraRay(f,float2(p)+0.5);
#ifdef MATERIAL_LAYERS
    [branch]if(g.w>=2){Surface s=surface(f,uint(w.w)-1,uint(h.z),h.xy);if(dot(s.normal,v)<0)s.normal=-s.normal;return s;}
#endif
    Surface s=(Surface)0;float4 a=extraPixel(f,35,p),n=pixel(f,NORMAL+f.control.x,p);
    s.position=w.xyz;s.id=uint(w.w)-1;s.albedo=a.rgb;s.metal=a.w;s.normal=n.xyz;s.roughness=n.w;s.geometricNormal=g.xyz;s.ao=g.w>=2?g.w-2:g.w;s.specular=1;
    s.sheenLut=f.display.z;s.coatRoughness=0.15;s.sheenRoughness=0.5;return s;
}
// Firefly suppression for one-sample specular estimates: cap a sample at sixteen
// display-white units under the current exposure. Converged highlights of real
// emitters keep their energy through accumulation; isolated paths cannot spike.
float3 clampSpecular(Frame f,float3 c){
    float exposure=buffers[NonUniformResourceIndex(f.control.y)].Load<float>(0);float l=dot(c,float3(0.2126,0.7152,0.0722))*exposure;
    return l>16?c*(16/l):c;
}
uint2 gridPhase(Frame f){uint t=f.control.w;uint phase=t^(t>>2)^(t>>4)^(t>>6);return uint2(phase&1,(phase>>1)&1);}
int2 gridPoint(Frame f,int2 p){int2 phase=int2(gridPhase(f));int2 q=max((p-phase)/2,int2(0))*2+phase;return min(q,int2(f.sizes.xy)-1-((int2(f.sizes.xy)-1-phase)&1));}
// Trace independent bounded secondary paths; the reference uses the same twelve
// total surface events, without reservoir reconnection or spatial filtering.
float3 traceSecondaryPath(Frame f,Surface s,float3 v,inout uint rng){
    float3 result=0,beta=1;
    for(uint bounce=0;bounce<11;bounce++){
        result+=beta*direct(f,s,v,rng);
        if(bounce==10)break;
        float3 weight;float3 l=sampleBrdf(s,v,rng,weight);beta*=weight;
        if(any(!isfinite(beta)) || max(max(beta.x,beta.y),beta.z)<1e-7)break;
        if(bounce>=2){float survival=clamp(max(max(beta.x,beta.y),beta.z),0.05,0.95);if(random(rng)>survival)break;beta/=survival;}
        Hit hit;if(!trace(f,rayOrigin(s,l),l,10000,rng,hit)){result+=beta*environment(f,l);break;}
        s=surface(f,hit.id,hit.primitive,hit.bary);if(dot(s.normal,-l)<0)s.normal=-s.normal;v=-l;
    }return result;
}
float3 traceSecondaryRay(Frame f,Surface s,float3 direction,inout uint rng){
    Hit hit;if(!trace(f,rayOrigin(s,direction),direction,10000,rng,hit))return environment(f,direction);
    Surface second=surface(f,hit.id,hit.primitive,hit.bary);if(dot(second.normal,-direction)<0)second.normal=-second.normal;
    return traceSecondaryPath(f,second,-direction,rng);
}
// Independent candidates plus reprojected prior reservoirs. Re-evaluate the target
// at the receiving surface; retain the original proposal normalization and count.
[shader("compute")][numthreads(8,8,1)]
void reservoirInit(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();ReservoirFrame reservoir=reservoirFrame();uint2 p=tid.xy*2+gridPhase(f);if(any(p>=f.sizes.xy))return;uint index=p.y*f.sizes.x+p.x,rng=hash(index+f.control.w*7919);
    DIReservoir dr=(DIReservoir)0;dr.light=0xffffffff;dr.owner=float(index);
    GIReservoir gr=(GIReservoir)0;
    if(pixel(f,WORLD+f.control.x,p).w==0){
        storeDI(reservoir.di.y,index,dr);
        storeGI(reservoir.gi.y,index,gr);return;
    }
    float3 v;Surface s=primarySurface(f,p,v);
    // Emissive triangles are many lights too: one candidate left emitter-lit
    // interiors with a near-random light per reservoir (visible DI mottling).
    uint candidates=buffers[f.scene.x].Load(12)>64 || emitterCount(f)>0?8:1;
    for(uint i=0;i<candidates;i++){
        DIReservoir c=(DIReservoir)0;c.light=chooseLight(f,rng);c.seed=hash(rng^0xa511e9b3u);random(rng);c.owner=float(index);c.visibility=-1;
        LightCandidate light=lightAt(f,s,v,c.light,c.seed);updateDI(dr,c,lightTarget(light.value)/max(light.pdf,1e-20),1,rng);
    }
    float2 uv;int2 at=int2(p);bool history=false;
    if(f.temporal.x==0 && f.temporal.y==0 && (reservoir.gi.w&1)==0){history=reproject(f,p,uv,at);if(history)history=compatible(f,p,at,true);}
    if(history){
        // Short temporal confidence: long-lived reused samples correlate frames, and
        // a still camera's 256-frame denoiser history then cannot average them out.
        DIReservoir old=readDI(reservoir.di.x,uint(at.y)*f.sizes.x+uint(at.x));float m=min(old.M,8.);
        float t=lightTarget(lightAt(f,s,v,old.light,old.seed).value);
        // Only exactly matching receivers reuse visibility; moving or jittered
        // receivers re-test the selected light, and caches expire after eight frames.
        if(length(pixel(f,WORLD+1-f.control.x,at).xyz-s.position)>0.0001 || old.age>=8)old.visibility=-1;
        old.owner=float(index);old.age+=1;updateDI(dr,old,t*old.W*m,m,rng);
    }
    float target=lightTarget(lightAt(f,s,v,dr.light,dr.seed).value);dr.W=target>0?dr.W/max(dr.M*target,1e-20):0;
    storeDI(reservoir.di.y,index,dr);
    if(history){
        GIReservoir old=readGI(reservoir.gi.x,uint(at.y)*f.sizes.x+uint(at.x));
        uint owner=old.owner;float4 source=pixel(f,WORLD+1-f.control.x,int2(owner%f.sizes.x,owner/f.sizes.x));
        bool local=source.w==float(s.id+1) && distance(source.xyz,s.position)<0.025;
        if(!layeredMaterial(s) && (old.radiance.w==0 || local) && old.stats.z<4 && reconnectable(s,old)){
            gr=old;gr.stats.y=min(gr.stats.y,1.);gr.stats.z+=1;gr.stats.w=reconnectionGeometry(s,gr);
        }
    }
    storeGI(reservoir.gi.y,index,gr);

}
// Compact trace dispatch keeps all lanes tracing, rather than leaving three
// quarters idle inside the full-rate direct-reservoir kernel.
[shader("compute")][numthreads(8,8,1)]
void reservoirTrace(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();ReservoirFrame reservoir=reservoirFrame();uint2 p=tid.xy;
    uint stride=max((reservoir.gi.w>>1)/2,1);
    float sampleWeight=f.temporal.x>0 || f.temporal.y>0?4.:4.*stride;
    uint phase=f.control.w^(f.control.w>>2)^(f.control.w>>4);
    if(f.temporal.x==0 && f.temporal.y==0)p.x=p.x*stride+((p.y+phase)&(stride-1));
    p=p*2+gridPhase(f);
    if(any(p>=f.sizes.xy) || pixel(f,WORLD+f.control.x,p).w==0)return;
    uint index=p.y*f.sizes.x+p.x,rng=hash(index+f.control.w*7919);float3 v;Surface s=primarySurface(f,p,v);
    if(layeredMaterial(s)){
        float3 weight;float3 direction=sampleBrdf(s,v,rng,weight);
        extraStore(f,SPEC_RAW,p,float4(clampSpecular(f,traceSecondaryRay(f,s,direction,rng)*weight),sampleWeight));
        GIReservoir empty=(GIReservoir)0;storeGI(reservoir.gi.y,index,empty);return;
    }
    GIReservoir gr=readGI(reservoir.gi.y,index);float3 oldDirection;float oldDistance;
    float target=lightTarget(giValue(s,v,gr,oldDirection,oldDistance));gr.stats.x*=gr.stats.y*target;
    bool fresh=true;float3 specular=0;
    if(fresh && (s.metal<1 || s.roughness>=0.5)){
        float3 weight=1;float proposal;bool shared=s.roughness>=0.5;
        float3 l;if(shared)l=sampleBrdfPdf(s,v,rng,weight,proposal);else{l=cosineDirection(s.normal,rng);proposal=max(dot(s.normal,l),0)/PI;}float omegaProposal=proposal;
        Hit hit;
        GIReservoir c=(GIReservoir)0;c.owner=index;
        if(proposal>0 && any(weight>0) && trace(f,rayOrigin(s,l),l,200,rng,hit)){
            Surface second=surface(f,hit.id,hit.primitive,hit.bary);if(dot(second.normal,-l)<0)second.normal=-second.normal;
            c.position=float4(second.position,0);c.normal=second.geometricNormal;
            c.radiance=float4(traceSecondaryPath(f,second,-l,rng),second.roughness<0.3 || second.metal>0.8?1:0);
            proposal*=abs(dot(second.geometricNormal,-l))/max(dot(second.position-s.position,second.position-s.position),1e-12);
        }else{c.position=float4(l,1);c.radiance=float4(proposal>0 && any(weight>0)?environment(f,l):float3(0),0);}
        if(shared)specular=c.radiance.rgb*max(weight-diffuseValue(s,v,l)/max(omegaProposal,1e-20),float3(0));
        c.stats.w=reconnectionGeometry(s,c);
        float3 direction;float distance;float target=lightTarget(giValue(s,v,c,direction,distance));
        if(s.metal<1)updateGI(gr,c,target/max(proposal,1e-20),1,rng);
    }
    float3 direction;float distance;target=lightTarget(giValue(s,v,gr,direction,distance));gr.stats.x=target>0?gr.stats.x/max(gr.stats.y*target,1e-20):0;
    gr.stats.w=reconnectionGeometry(s,gr);
    storeGI(reservoir.gi.y,index,gr);
    extraStore(f,SPEC_RAW,p,float4(clampSpecular(f,specular),fresh?sampleWeight:0));
}

// Glossy primary pixels need their own full-rate samples; coarse diffuse-path
// candidates cannot carry the view-dependent GGX lobe without visible speckles.
[shader("compute")][numthreads(8,8,1)]
void specularTrace(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 p=tid.xy;if(any(p>=f.sizes.xy) || pixel(f,WORLD+f.control.x,p).w==0 || pixel(f,NORMAL+f.control.x,p).w>=0.5)return;
    uint rng=hash((p.y*f.sizes.x+p.x)^f.control.w*8737);float3 v;Surface s=primarySurface(f,p,v);if(layeredMaterial(s))return;
    float3 h=visibleGgx(s,v,rng),l=reflect(-v,h),result=0;float nl=dot(s.normal,l);
    if(nl>0)result=traceSecondaryRay(f,s,l,rng)*fresnel(max(dot(v,h),0),lerp(float3(0.04*s.specular),s.albedo,s.metal))*smith(nl,pow(s.roughness,4));
    extraStore(f,SPEC_RAW,p,float4(clampSpecular(f,result),1));
}

// Reconnect diffuse paths in area measure; cos(theta_secondary)/distance^2 is
// evaluated at each receiver, providing the reconnection Jacobian automatically.
[shader("compute")][numthreads(8,8,1)]
void reservoirSpatial(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();ReservoirFrame reservoir=reservoirFrame();uint2 p=tid.xy;if(any(p>=f.sizes.xy))return;uint index=p.y*f.sizes.x+p.x,rng=hash(index+f.control.w*4567);
    if(pixel(f,WORLD+f.control.x,p).w==0){extraStore(f,SPEC_RAW,p,float4(0,0,0,1));return;}
    float3 v;Surface s=primarySurface(f,p,v);DIReservoir dr=(DIReservoir)0;dr.light=0xffffffff;GIReservoir gr=(GIReservoir)0;
    for(uint i=0;i<3;i++){
        int2 q=int2(p);if(i>0){float angle=random(rng)*2*PI;float radius=1+random(rng)*(i==1?3:8);q+=int2(cos(angle)*radius,sin(angle)*radius);}q=gridPoint(f,q);
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
        float3 local=risDirect(f,s,v,rng,true),indirect=0;
        // Keep the specular channel's inclusion probability independent of repair.
        // Pixels without any compatible reservoir (disocclusions, thin parts) take
        // the DDGI irradiance: noise-free, and far cheaper than a full traced path.
        if(!layeredMaterial(s) && s.metal<1)indirect=irradiance(f,s.position,s.normal,f.probes.y)*diffuseValue(s,v,s.normal);
        store(f,DIRECT,p,float4(pixel(f,DIRECT,p).rgb+local,1));store(f,RAW,p,float4(indirect/demodulation(f,p),1));
        storeDI(reservoir.di.z,index,dr);storeGI(reservoir.gi.z,index,gr);return;
    }
    LightCandidate selected=lightAt(f,s,v,dr.light,dr.seed);float target=lightTarget(selected.value);
    dr.W=target>0?dr.W/max(dr.M*target,1e-20):0;
    if(dr.W>0 && dr.visibility<0){dr.visibility=occluded(f,rayOrigin(s,selected.direction),selected.direction,selected.distance,rng)?0:1;dr.age=0;}
    dr.owner=float(index);storeDI(reservoir.di.z,index,dr);
    store(f,DIRECT,p,float4(pixel(f,DIRECT,p).rgb+selected.value*dr.W*max(dr.visibility,0),1));
    float3 direction;float distance;float3 gi=giValue(s,v,gr,direction,distance);target=lightTarget(gi);
    gr.stats.x=target>0?gr.stats.x/max(gr.stats.y*target,1e-20):0;
    gr.stats.w=reconnectionGeometry(s,gr);
    if(gr.stats.x>0 && occluded(f,rayOrigin(s,direction),direction,max(distance-0.003,0.001),rng))gi=0;
    storeGI(reservoir.gi.z,index,gr);
    store(f,RAW,p,float4(gi*gr.stats.x/demodulation(f,p),1));
}
