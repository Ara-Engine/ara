// Ara native binding ABI v1. Particle simulation has no wall-clock inputs.
struct Root {uint particles;uint lists;uint control;uint frame;uint tick;uint emitter;uint sortK;uint sortJ;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(0) Texture2D<float4> textures[128] : register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[64] : register(u0,space2);
BIND(2) SamplerState samplers[3] : register(s0,space0);
struct Config {uint4 partition;uint4 lists;uint4 scratch;uint4 render;uint4 atlas;float4 surface;float4 boundsMin;float4 boundsMax;float4 lightColor;float4 lightPosition;};
struct Frame {float4x4 vp;float4x4 inverseVp;float4 camera;float4 right;float4 up;float4 forward;uint4 extent;float4 clock;uint4 inputs;uint4 demands[16];};
struct Particle {float3 position;float age;float3 velocity;float life;float4 color;float size;uint seed;uint birth;};
Frame frame(){return buffers[NonUniformResourceIndex(root.frame)].Load<Frame>(0);}
Config config(uint e){return buffers[NonUniformResourceIndex(frame().inputs.x)].Load<Config>(e*160);}
uint state(uint e,uint offset){return buffers[NonUniformResourceIndex(root.control)].Load(e*64+offset);}
void storeState(uint e,uint offset,uint value){buffers[NonUniformResourceIndex(root.control)].Store(e*64+offset,value);}
uint list(uint offset){return buffers[NonUniformResourceIndex(root.lists)].Load(offset);}
void storeList(uint offset,uint value){buffers[NonUniformResourceIndex(root.lists)].Store(offset,value);}
uint hash(uint x){x^=x>>16;x*=0x7feb352du;x^=x>>15;x*=0x846ca68bu;return x^(x>>16);}
float random(inout uint rng){rng=hash(rng+0x9e3779b9u);return float(rng>>8)/16777216.;}
float3 random3(inout uint rng){float x=random(rng);float y=random(rng);float z=random(rng);return float3(x,y,z);}
Particle loadParticle(uint slot){
    uint n=frame().extent.z;Particle p;
    float4 a=buffers[NonUniformResourceIndex(root.particles)].Load<float4>(slot*16);
    float4 b=buffers[NonUniformResourceIndex(root.particles)].Load<float4>((slot+n)*16);
    p.position=a.xyz;p.age=a.w;p.velocity=b.xyz;p.life=b.w;
    p.color=buffers[NonUniformResourceIndex(root.particles)].Load<float4>((slot+2*n)*16);
    uint4 s=buffers[NonUniformResourceIndex(root.particles)].Load<uint4>((slot+3*n)*16);
    p.size=asfloat(s.x);p.seed=s.y;p.birth=s.z;return p;
}
void storeParticle(uint slot,Particle p,uint events){
    uint n=frame().extent.z;
    buffers[NonUniformResourceIndex(root.particles)].Store<float4>(slot*16,float4(p.position,p.age));
    buffers[NonUniformResourceIndex(root.particles)].Store<float4>((slot+n)*16,float4(p.velocity,p.life));
    buffers[NonUniformResourceIndex(root.particles)].Store<float4>((slot+2*n)*16,p.color);
    buffers[NonUniformResourceIndex(root.particles)].Store<uint4>((slot+3*n)*16,uint4(asuint(p.size),p.seed,p.birth,events));
}
float3 reconstruct(float2 uv,float z){float4 w=mul(frame().inverseVp,float4(uv.x*2-1,1-uv.y*2,z,1));return w.xyz/w.w;}
void collideDepth(inout Particle p,float3 previous,float restitution,float thickness,inout uint events){
    Frame f=frame();if(f.inputs.z==0)return;
    float4 clip=mul(f.vp,float4(p.position,1));if(clip.w<=0)return;
    float2 uv=float2(clip.x/clip.w*.5+.5,.5-clip.y/clip.w*.5);if(any(uv<=0)||any(uv>=1))return;
    uint2 pixel=min(uint2(uv*f.extent.xy),f.extent.xy-1);
    float z=textures[NonUniformResourceIndex(f.inputs.y)].Load(int3(pixel,0)).r;if(z<=0)return;
    float3 contact=reconstruct(uv,z);
    float3 dx=reconstruct(uv+float2(1./f.extent.x,0),textures[NonUniformResourceIndex(f.inputs.y)].Load(int3(min(pixel+uint2(1,0),f.extent.xy-1),0)).r)-contact;
    float3 dy=reconstruct(uv+float2(0,1./f.extent.y),textures[NonUniformResourceIndex(f.inputs.y)].Load(int3(min(pixel+uint2(0,1),f.extent.xy-1),0)).r)-contact;
    float3 n=normalize(cross(dx,dy));if(dot(n,f.camera.xyz-contact)<0)n=-n;
    float distance=dot(p.position-contact,n);
    if(distance<0&&(distance>=-thickness||dot(previous-contact,n)>=0)){
        p.position-=distance*n;float vn=dot(p.velocity,n);if(vn<0){p.velocity-=(1+restitution)*vn*n;events|=1u;}
    }
}
struct AraMaterialInput {
    float2 uv;
    float3 worldPosition;
    float3 worldNormal;
    float3 worldTangent;
    float3 worldBitangent;
    float3 viewDirection;
};
struct AraSurface {
    float3 baseColor;
    float metallic;
    float roughness;
    float3 normal;
    float3 emissive;
    float opacityMask;
    float clearCoat;
    float clearCoatRoughness;
    float transmission;
    float3 color;
    float opacity;
    float clip;
};

float4 ara_load(uint offset) {
    return buffers[NonUniformResourceIndex(frame().inputs.w)].Load<float4>((root.emitter*64+offset)*16);
}
float2 ara_normalize(float2 v) { return v * rsqrt(max(dot(v, v), 1e-20)); }
float3 ara_normalize(float3 v) { return v * rsqrt(max(dot(v, v), 1e-20)); }
float4 ara_normalize(float4 v) { return v * rsqrt(max(dot(v, v), 1e-20)); }
float ara_linear_lane(float v) {
    return v <= 0.04045 ? v / 12.92 : pow(max((v + 0.055) / 1.055, 0.0), 2.4);
}
float3 ara_linear(float3 v) {
    return float3(ara_linear_lane(v.x), ara_linear_lane(v.y), ara_linear_lane(v.z));
}
float4 ara_sample_data(uint index, float2 uv) {
    return textures[NonUniformResourceIndex(index)].Sample(samplers[0], uv);
}
float3 ara_sample_color(uint index, float2 uv) { return ara_linear(ara_sample_data(index, uv).rgb); }
float3 ara_sample_normal(uint index, float2 uv) { return ara_normalize(ara_sample_data(index, uv).rgb * 2.0 - 1.0); }
float3 ara_normal_blend(float3 a, float3 b) {
    float3 t = a + float3(0, 0, 1);
    float3 u = b * float3(-1, -1, 1);
    return ara_normalize(t * dot(t, u) / max(t.z, 1e-5) - u);
}
float3 ara_normal_strength(float3 n, float strength) { return ara_normalize(float3(n.xy * strength, n.z)); }
float ara_hash(float2 p) { return frac(sin(dot(p, float2(127.1, 311.7))) * 43758.5453); }
float ara_noise(float2 p) {
    float2 i = floor(p);
    float2 f = frac(p);
    float2 u = f * f * (3.0 - 2.0 * f);
    return lerp(lerp(ara_hash(i), ara_hash(i + float2(1, 0)), u.x),
                lerp(ara_hash(i + float2(0, 1)), ara_hash(i + float2(1, 1)), u.x), u.y);
}
float3 ara_triplanar(uint index, float3 world, float3 normal, float scale) {
    float3 weights = abs(normal);
    weights /= max(weights.x + weights.y + weights.z, 1e-5);
    return ara_sample_color(index, world.yz * scale) * weights.x
         + ara_sample_color(index, world.zx * scale) * weights.y
         + ara_sample_color(index, world.xy * scale) * weights.z;
}
float2 ara_parallax(uint index, float2 uv, float3 tangentView, float scale) {
    float height = ara_sample_data(index, uv).r - 0.5;
    return uv - height * scale * tangentView.xy / max(abs(tangentView.z), 0.1);
}
float ara_fresnel(float3 n, float3 v, float power) { return pow(saturate(1.0 - dot(ara_normalize(n), ara_normalize(v))), max(power, 0.0)); }
float2 ara_rotate_uv(float2 uv, float radians) {
    float c = cos(radians), s = sin(radians);
    float2 p = uv - 0.5;
    return float2(c * p.x - s * p.y, s * p.x + c * p.y) + 0.5;
}
float ara_rounded_panel(float2 uv, float2 size, float radius) {
    radius = clamp(radius, 0.0, max(min(size.x, size.y) * 0.5, 0.0));
    float2 q = abs((uv - 0.5) * size) - max(size * 0.5 - radius, 0.0);
    float distance = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
    return saturate(0.5 - distance / max(fwidth(distance), 1e-4));
}

// Lighting is engine-owned. All renderer passes consume the same AraSurface;
// material functions cannot replace the BRDF or introduce independent bindings.
float3 ara_ggx(float3 f0, float roughness, float nDotL, float nDotV, float nDotH, float vDotH) {
    float alpha = max(roughness * roughness, 0.002025);
    float a2 = alpha * alpha;
    float d = nDotH * nDotH * (a2 - 1.0) + 1.0;
    float distribution = a2 / max(3.14159265 * d * d, 1e-6);
    float k = (roughness + 1.0) * (roughness + 1.0) * 0.125;
    float visibility = 1.0 / max((nDotV * (1.0 - k) + k) * (nDotL * (1.0 - k) + k) * 4.0, 1e-5);
    float3 fresnel = f0 + (1.0 - f0) * pow(1.0 - vDotH, 5.0);
    return distribution * visibility * fresnel;
}
float3 ara_engine_light(AraSurface s, AraMaterialInput input, float3 lightDirection, float3 radiance, float3 indirect) {
    float3 n = ara_normalize(input.worldTangent * s.normal.x + input.worldBitangent * s.normal.y + input.worldNormal * s.normal.z);
    float3 v = ara_normalize(input.viewDirection);
    float3 l = ara_normalize(lightDirection);
    float3 h = ara_normalize(v + l);
    float nl = saturate(dot(n, l)), nv = saturate(dot(n, v));
    float nh = saturate(dot(n, h)), vh = saturate(dot(v, h));
    float metallic = saturate(s.metallic), coat = saturate(s.clearCoat);
    float3 f0 = lerp(float3(0.04), s.baseColor, metallic);
    float3 fresnel = f0 + (1.0 - f0) * pow(1.0 - vh, 5.0);
    float3 diffuse = s.baseColor * (1.0 - metallic) * (1.0 - fresnel) / 3.14159265;
    float3 specular = ara_ggx(f0, saturate(s.roughness), nl, nv, nh, vh);
    float3 clearCoat = ara_ggx(float3(0.04), saturate(s.clearCoatRoughness), nl, nv, nh, vh) * coat;
    float3 transmitted = s.baseColor * saturate(s.transmission) * saturate(dot(-n, l));
    return ((diffuse + specular) * (1.0 - coat * 0.04) + clearCoat) * radiance * nl
         + transmitted * radiance + s.baseColor * indirect * (1.0 - metallic) + s.emissive;
}
// VFX content 52fef00069159ecab563ab6a7f52142f6b5646f6c81c96ddd092e4792911b3e4
void initializeParticle(uint emitter, inout Particle p) { uint rng=p.seed; switch(emitter) {
case 0u: {
{
p.life=2.0; p.size=0.03500000014901161; p.color=float4(8.0,2.799999952316284,0.25,0.8999999761581421);
}
{
p.position=float3(0.0,0.699999988079071,0.0);
}
{
p.velocity=float3(0.0,3.0,0.0)+(2*random3(rng)-1)*float3(2.5999999046325684,2.0,2.5999999046325684);
}
break;}
}}
void updateParticle(uint emitter,inout Particle p,float3 previous,float dt,inout uint events) {float t=saturate(p.age/p.life);switch(emitter){
case 0u: {
{
p.velocity+=float3(0.0,-9.8100004196167,0.0)*dt;
}
{
collideDepth(p,previous,0.6000000238418579, 0.5,events);
}
break;}
}}
uint2 eventTarget(uint source,uint trigger) {switch(source){
case 0u: {
break;}
}return uint2(0xffffffffu,0);}
float3 meshVertex(uint emitter,uint vertex){switch(emitter){
}return float3(0);}
// sparks / Spawn / rate: rate
// sparks / Initialize / lifetime: lifetime
// sparks / Initialize / source: point
// sparks / Initialize / spray: velocity
// sparks / Update / gravity: gravity
// sparks / Update / floor: depth_collision
// sparks / Render / sprite: sprite

AraSurface evaluate0(AraMaterialInput input) {
    AraSurface result;
    result.baseColor = float3(1);
    result.metallic = 0;
    result.roughness = 0.5;
    result.normal = float3(0, 0, 1);
    result.emissive = float3(0);
    result.opacityMask = 1;
    result.clearCoat = 0;
    result.clearCoatRoughness = 0.1;
    result.transmission = 0;
    result.color = float3(1);
    result.opacity = 1;
    result.clip = 1;
    float3 n_f303e65886bb76767142 = float3(1.0, 1.0, 1.0);
    result.color = n_f303e65886bb76767142;
    return result;
}

AraSurface evaluateMaterial(uint emitter,AraMaterialInput input){switch(emitter){
case 0u:return evaluate0(input);
}return evaluate0(input);}
// All lists are stable by slot. No unordered atomic append enters replay state.
[shader("compute")][numthreads(1,1,1)]
void prepareMain(){uint e=root.emitter;uint requested=frame().demands[e].x+state(e,12);uint emitted=min(state(e,4),requested);storeState(e,48,state(e,48)+requested-emitted);}
[shader("compute")][numthreads(256,1,1)]
void spawnMain(uint3 id:SV_DispatchThreadID){
    uint e=root.emitter;Config c=config(e);uint j=id.x;uint regular=frame().demands[e].x;
    uint count=min(state(e,4),regular+state(e,12));if(j>=count)return;
    uint slot=list(c.lists.y+j*4);Particle p;p.position=0;p.velocity=0;p.age=0;p.life=1;p.color=1;p.size=.1;p.birth=root.tick;
    p.seed=hash(frame().demands[e].y^hash(e)^hash(root.tick)^hash(j));initializeParticle(e,p);
    if(j>=regular)p.position+=buffers[NonUniformResourceIndex(root.lists)].Load<float4>(c.scratch.z+(j-regular)*16).xyz;
    storeParticle(c.partition.x+slot,p,0);
}
[shader("compute")][numthreads(256,1,1)]
void updateMain(uint3 id:SV_DispatchThreadID){
    uint e=root.emitter;Config c=config(e);if(id.x>=c.partition.y)return;
    uint slot=c.partition.x+id.x;Particle p=loadParticle(slot);uint events=0;
    if(p.age>=0){float3 previous=p.position;p.age+=frame().clock.x;p.position+=p.velocity*frame().clock.x;
        updateParticle(e,p,previous,frame().clock.x,events);if(p.age>=p.life){events|=2u;p.age=-1;}}
    storeParticle(slot,p,events);
}
groupshared uint aliveScan[256];
groupshared uint eventScan[256];
groupshared float areaScan[256];
[shader("compute")][numthreads(256,1,1)]
void scanMain(uint3 id:SV_DispatchThreadID,uint3 local:SV_GroupThreadID,uint3 group:SV_GroupID){
    Config c=config(root.emitter);uint l=local.x;uint i=id.x;uint a=0;uint events=0;float area=0;
    if(i<c.partition.y){Particle p=loadParticle(c.partition.x+i);a=p.age>=0?1:0;uint flags=buffers[NonUniformResourceIndex(root.particles)].Load((c.partition.x+i+3*frame().extent.z)*16+12);events=countbits(flags&3u);
        if(a!=0){Frame f=frame();float diameter=p.size;if(c.render.x==1)diameter=max(c.surface.z,length(c.boundsMax.xyz-c.boundsMin.xyz));if(c.render.x==2)diameter*=asfloat(f.demands[root.emitter].z);
            float distance=max(f.clock.z,dot(p.position-f.camera.xyz,f.forward.xyz)-diameter*.5);float projected=diameter*f.clock.w/(2*distance);area=max(1./float(f.extent.x*f.extent.y),min(1.,projected*projected*float(f.extent.y)/f.extent.x));}}
    aliveScan[l]=a;eventScan[l]=events;areaScan[l]=area;GroupMemoryBarrierWithGroupSync();
    for(uint stride=1;stride<256;stride*=2){uint pa=l>=stride?aliveScan[l-stride]:0;uint pe=l>=stride?eventScan[l-stride]:0;float ps=l>=stride?areaScan[l-stride]:0;GroupMemoryBarrierWithGroupSync();aliveScan[l]+=pa;eventScan[l]+=pe;areaScan[l]+=ps;GroupMemoryBarrierWithGroupSync();}
    buffers[NonUniformResourceIndex(root.lists)].Store<uint4>(c.lists.z+i*16,uint4(aliveScan[l],eventScan[l],asuint(areaScan[l]),0));
    if(l==255)buffers[NonUniformResourceIndex(root.lists)].Store<uint4>(c.lists.w+group.x*16,uint4(aliveScan[l],eventScan[l],asuint(areaScan[l]),0));
}
[shader("compute")][numthreads(256,1,1)]
void prefixMain(uint3 local:SV_GroupThreadID){
    uint e=root.emitter;Config c=config(e);uint lane=local.x;
    // Contiguous chunks preserve slot order. At 1M particles each lane scans 16 groups.
    uint span=(c.partition.w+255)/256;uint start=lane*span;uint end=min(start+span,c.partition.w);
    uint2 sum=0;float area=0;
    for(uint g=start;g<end;g++){uint4 count=buffers[NonUniformResourceIndex(root.lists)].Load<uint4>(c.lists.w+g*16);sum+=count.xy;area+=asfloat(count.z);}
    aliveScan[lane]=sum.x;eventScan[lane]=sum.y;areaScan[lane]=area;GroupMemoryBarrierWithGroupSync();
    for(uint stride=1;stride<256;stride*=2){uint pa=lane>=stride?aliveScan[lane-stride]:0;uint pe=lane>=stride?eventScan[lane-stride]:0;float ps=lane>=stride?areaScan[lane-stride]:0;GroupMemoryBarrierWithGroupSync();aliveScan[lane]+=pa;eventScan[lane]+=pe;areaScan[lane]+=ps;GroupMemoryBarrierWithGroupSync();}
    uint2 total=uint2(aliveScan[255],eventScan[255]);float totalArea=areaScan[255];
    uint2 prefix=lane>0?uint2(aliveScan[lane-1],eventScan[lane-1]):uint2(0);
    area=lane>0?areaScan[lane-1]:0;uint selected=0;
    GroupMemoryBarrierWithGroupSync();
    for(uint g=start;g<end;g++){
        uint4 count=buffers[NonUniformResourceIndex(root.lists)].Load<uint4>(c.lists.w+g*16);
        buffers[NonUniformResourceIndex(root.lists)].Store<uint2>(c.scratch.x+g*16,prefix);
        float groupArea=asfloat(count.z);
        if(area+groupArea<=c.surface.w)selected=prefix.x+count.x;
        else if(area<c.surface.w){for(uint slot=0;slot<256;slot++){uint4 value=buffers[NonUniformResourceIndex(root.lists)].Load<uint4>(c.lists.z+(g*256+slot)*16);if(area+asfloat(value.z)>c.surface.w)break;selected=prefix.x+value.x;}}
        prefix+=count.xy;area+=groupArea;
    }
    aliveScan[lane]=selected;GroupMemoryBarrierWithGroupSync();
    for(uint stride=128;stride>0;stride/=2){if(lane<stride)aliveScan[lane]=max(aliveScan[lane],aliveScan[lane+stride]);GroupMemoryBarrierWithGroupSync();}
    if(lane==0){storeState(e,0,total.x);storeState(e,4,c.partition.y-total.x);storeState(e,8,min(total.y,c.atlas.z));storeState(e,52,state(e,52)+(total.y>c.atlas.z?total.y-c.atlas.z:0));storeState(e,56,aliveScan[0]);storeState(e,60,asuint(totalArea));}
}
[shader("compute")][numthreads(256,1,1)]
void scatterMain(uint3 id:SV_DispatchThreadID,uint3 group:SV_GroupID){
    uint e=root.emitter;Config c=config(e);uint i=id.x;if(i>=c.partition.y)return;
    uint slot=c.partition.x+i;Particle p=loadParticle(slot);
    uint2 local=buffers[NonUniformResourceIndex(root.lists)].Load<uint2>(c.lists.z+i*16);
    uint2 prefix=buffers[NonUniformResourceIndex(root.lists)].Load<uint2>(c.scratch.x+group.x*16);
    if(p.age>=0)storeList(c.lists.x+(prefix.x+local.x-1)*4,i);
    else storeList(c.lists.y+(i-prefix.x-local.x)*4,i);
    uint flags=buffers[NonUniformResourceIndex(root.particles)].Load((slot+3*frame().extent.z)*16+12)&3u;
    uint cursor=prefix.y+local.y-countbits(flags);
    for(uint trigger=1;trigger<=2;trigger*=2){if((flags&trigger)!=0){if(cursor<c.atlas.z){uint offset=c.scratch.y+cursor*32;buffers[NonUniformResourceIndex(root.lists)].Store<float4>(offset,float4(p.position,0));buffers[NonUniformResourceIndex(root.lists)].Store<uint4>(offset+16,uint4(trigger,e,i,root.tick));}cursor++;}}
}
[shader("compute")][numthreads(1,1,1)]
void routeMain(){
    uint dest=root.emitter;Config target=config(dest);uint routed=0;uint demand=0;
    for(uint source=0;source<frame().extent.w;source++){Config c=config(source);uint count=state(source,8);
        for(uint event=0;event<count;event++){uint offset=c.scratch.y+event*32;uint trigger=list(offset+16);uint2 route=eventTarget(source,trigger);
            if(route.x==dest){float4 position=buffers[NonUniformResourceIndex(root.lists)].Load<float4>(offset);demand+=route.y;
                for(uint child=0;child<route.y&&routed<target.atlas.z;child++){buffers[NonUniformResourceIndex(root.lists)].Store<float4>(target.scratch.z+routed*16,position);routed++;}}}}
    storeState(dest,12,routed);storeState(dest,52,state(dest,52)+demand-routed);
}
float sortKey(uint slot,Config c){if(slot==0xffffffffu)return -3.402823e38;Particle p=loadParticle(c.partition.x+slot);return c.render.w==1?dot(p.position-frame().camera.xyz,frame().forward.xyz):p.age;}
[shader("compute")][numthreads(256,1,1)]
void sortPrepareMain(uint3 id:SV_DispatchThreadID){Config c=config(root.emitter);if(id.x<c.scratch.w&&id.x>=state(root.emitter,56))storeList(c.lists.x+id.x*4,0xffffffffu);}
[shader("compute")][numthreads(256,1,1)]
void sortMain(uint3 id:SV_DispatchThreadID){
    Config c=config(root.emitter);uint i=id.x;uint j=i^root.sortJ;if(j<=i||j>=c.scratch.w)return;
    uint a=list(c.lists.x+i*4);uint b=list(c.lists.x+j*4);float ka=sortKey(a,c);float kb=sortKey(b,c);
    bool greater=ka>kb||(ka==kb&&a<b);bool descending=(i&root.sortK)==0;
    if(greater!=descending){storeList(c.lists.x+i*4,b);storeList(c.lists.x+j*4,a);}
}
[shader("compute")][numthreads(1,1,1)]
void argsMain(){
    uint e=root.emitter;Config c=config(e);uint alive=state(e,0);uint visible=frame().demands[e].w;
    uint rendered=visible!=0?min(alive,state(e,56)):0;
    if(c.render.x==1)rendered=rendered>1?rendered-1:0;
    buffers[NonUniformResourceIndex(root.control)].Store<uint4>(e*64+16,uint4(c.render.y,rendered,0,0));
    buffers[NonUniformResourceIndex(root.control)].Store<uint3>(e*64+32,uint3((state(e,4)+255)/256,1,1));
    storeState(e,56,rendered);
}
struct Screen {float4 clip:SV_Position;float2 uv:TEXCOORD0;};
[shader("vertex")]
Screen screenVertex(uint id:SV_VertexID){Screen o;float2 uv=float2((id<<1)&2,id&2);o.clip=float4(uv*2-1,0,1);o.uv=float2(uv.x,1-uv.y);return o;}
struct Backdrop {float4 color:SV_Target0;float depth:SV_Target1;};
[shader("fragment")]
Backdrop backdropFragment(Screen input){
    Frame f=frame();float3 ray=normalize(reconstruct(input.uv,0.001)-f.camera.xyz);Backdrop o;
    o.depth=0;o.color=float4(lerp(float3(.008,.013,.025),float3(.025,.04,.065),saturate(1-input.uv.y)),0);
    if(ray.y<-.0001){float distance=-f.camera.y/ray.y;if(distance>0){float3 world=f.camera.xyz+ray*distance;
        float grid=1-smoothstep(.01,.025,min(abs(frac(world.x+.5)-.5),abs(frac(world.z+.5)-.5)));
        float3 color=lerp(float3(.025,.035,.05),float3(.045,.06,.08),grid*.4);
        for(uint e=0;e<f.extent.w;e++){Config c=config(e);if(c.lightColor.w>0&&state(e,0)>0){float3 delta=c.lightPosition.xyz-world;float d2=max(dot(delta,delta),.05);float attenuation=saturate(1-sqrt(d2)/c.lightPosition.w);color+=c.lightColor.rgb*c.lightColor.w/(12.56637*d2)*attenuation*attenuation*.025;}}
        float4 clip=mul(f.vp,float4(world,1));o.depth=clip.z/clip.w;o.color=float4(color,0);}}
    return o;
}
struct ParticleVarying {float4 clip:SV_Position;float3 world:TEXCOORD0;float2 uv:TEXCOORD1;float4 color:TEXCOORD2;float3 normal:TEXCOORD3;float age:TEXCOORD4;nointerpolation uint emitter:TEXCOORD5;};
[shader("vertex")]
ParticleVarying particleVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){
    uint e=root.emitter;Config c=config(e);Frame f=frame();uint local=list(c.lists.x+instance*4);Particle p=loadParticle(c.partition.x+local);
    static const float2 corners[6]={float2(-1,-1),float2(1,-1),float2(-1,1),float2(-1,1),float2(1,-1),float2(1,1)};
    ParticleVarying o;float2 corner=corners[vertex%6];float3 world;float3 normal=-f.forward.xyz;
    if(c.render.x==1){uint next=list(c.lists.x+(instance+1)*4);Particle q=loadParticle(c.partition.x+next);float3 tangent=normalize(q.position-p.position+float3(1e-8,0,0));float3 side=normalize(cross(tangent,f.camera.xyz-p.position));world=lerp(p.position,q.position,(corner.y+1)*.5)+side*corner.x*c.surface.z*.5;}
    else if(c.render.x==2){float3 forward=length(p.velocity)>.001?normalize(p.velocity):float3(0,0,1);float3 right=normalize(cross(abs(forward.y)>.99?float3(1,0,0):float3(0,1,0),forward));float3 up=cross(forward,right);float3 vert=meshVertex(e,vertex);world=p.position+p.size*(right*vert.x+up*vert.y+forward*vert.z);normal=normalize(right*vert.x+up*vert.y+forward*vert.z+float3(1e-8,0,0));}
    else world=p.position+(f.right.xyz*corner.x+f.up.xyz*corner.y)*p.size*.5;
    o.clip=mul(f.vp,float4(world,1));o.world=world;o.uv=(corner+1)*.5;o.color=p.color;o.normal=normal;o.age=p.age;o.emitter=e;return o;
}
[shader("fragment")]
float4 particleFragment(ParticleVarying input):SV_Target0{
    if(input.age<0)discard;
    Config c=config(input.emitter);Frame f=frame();float2 disc=input.uv*2-1;
    float coverage=c.render.x==2?1:pow(saturate(1-dot(disc,disc)),1.6);if(coverage<.001)discard;
    float2 uv=input.uv;uint frameIndex=uint(max(0,input.age*c.surface.y))%(c.atlas.x*c.atlas.y);
    uv=(uv+float2(frameIndex%c.atlas.x,frameIndex/c.atlas.x))/float2(c.atlas.xy);
    float fade=1;if(f.inputs.z!=0){float z=textures[NonUniformResourceIndex(f.inputs.y)].Load(int3(uint2(input.clip.xy),0)).r;if(z>input.clip.z)discard;if(c.surface.x>0&&z>0)fade=saturate((f.clock.z/z-f.clock.z/input.clip.z)/c.surface.x);}
    AraMaterialInput m;m.uv=uv;m.worldPosition=input.world;m.worldNormal=input.normal;m.worldTangent=f.right.xyz;m.worldBitangent=f.up.xyz;m.viewDirection=normalize(f.camera.xyz-input.world);
    AraSurface s=evaluateMaterial(input.emitter,m);if(s.opacityMask<.5||s.clip<.5)discard;
    float3 color;
    if(c.render.z==0){s.baseColor*=input.color.rgb;color=ara_engine_light(s,m,normalize(float3(.4,1,.3)),float3(1.5,1.3,1.1),float3(.12,.17,.24));
        for(uint e=0;e<f.extent.w;e++){Config light=config(e);if(light.lightColor.w>0&&state(e,0)>0){float3 d=light.lightPosition.xyz-input.world;float d2=max(dot(d,d),.05);float a=saturate(1-sqrt(d2)/light.lightPosition.w);color+=ara_engine_light(s,m,normalize(d),light.lightColor.rgb*(light.lightColor.w*a*a/(12.56637*d2)),float3(0));}}}
    else color=s.color*input.color.rgb;
    float alpha=saturate(input.color.a*s.opacity*coverage*fade);return float4(color*alpha,alpha);
}
struct Display {float4 color:SV_Target0;float id:SV_Target1;};
[shader("fragment")]
Display tonemapFragment(Screen input){
    float4 hdr=textures[NonUniformResourceIndex(root.sortK)].Load(int3(uint2(input.uv*frame().extent.xy),0));float3 c=hdr.rgb*frame().clock.y;
    c=(c*(2.51*c+.03))/(c*(2.43*c+.59)+.14);c=saturate(c);c=pow(c,1./2.2);Display o;o.color=float4(c,1);o.id=hdr.a>0.00001?1:0;return o;
}
