// Ara native binding ABI v1. Particle simulation has no wall-clock inputs.
struct Root {uint particles;uint lists;uint control;uint frame;uint tick;uint emitter;uint sortK;uint sortJ;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(5) Texture2D<float4> textures[] : register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[64] : register(u0,space2);
BIND(2) SamplerState samplers[3] : register(s0,space0);
struct Config {uint4 partition;uint4 lists;uint4 scratch;uint4 render;uint4 atlas;float4 surface;float4 boundsMin;float4 boundsMax;float4 lightColor;float4 lightPosition;uint4 appearance;float4 motion;};
struct Frame {float4x4 vp;float4x4 inverseVp;float4 camera;float4 right;float4 up;float4 forward;uint4 extent;float4 clock;uint4 inputs;uint4 demands[16];uint4 visuals;float4 ambient;float4 sunDirection;float4 sunColor;float4 probeOrigin;float4 probeSpacing;uint4 probeCounts;};
struct Particle {float3 position;float age;float3 velocity;float life;float4 color;float size;uint seed;uint birth;};
Frame frame(){return buffers[NonUniformResourceIndex(root.frame)].Load<Frame>(0);}
Config config(uint e){return buffers[NonUniformResourceIndex(frame().inputs.x)].Load<Config>(e*192);}
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
// Scene depth in the effect's local units: sunDirection.w is world metres per
// local unit (Placement.scale), applied to linear and reversed-Z depth alike.
float sceneDepth(uint2 pixel){Frame f=frame();float z=textures[NonUniformResourceIndex(f.inputs.y)].Load(int3(pixel,0)).r;float s=f.sunDirection.w;return (f.visuals.z&1u)!=0&&z>0?f.clock.z*s/z:z*s;}
void collideDepth(inout Particle p,float3 previous,float restitution,float thickness,inout uint events){
    Frame f=frame();if(f.inputs.z==0)return;
    float4 clip=mul(f.vp,float4(p.position,1));if(clip.w<=0)return;
    float2 uv=float2(clip.x/clip.w*.5+.5,.5-clip.y/clip.w*.5);if(any(uv<=0)||any(uv>=1))return;
    uint2 pixel=min(uint2(uv*f.extent.xy),f.extent.xy-1);
    float z=sceneDepth(pixel);if(z<=0)return;
    float3 contact=reconstruct(uv,z);
    float3 dx=reconstruct(uv+float2(1./f.extent.x,0),sceneDepth(min(pixel+uint2(1,0),f.extent.xy-1)))-contact;
    float3 dy=reconstruct(uv+float2(0,1./f.extent.y),sceneDepth(min(pixel+uint2(0,1),f.extent.xy-1)))-contact;
    float3 n=normalize(cross(dx,dy));if(dot(n,f.camera.xyz-contact)<0)n=-n;
    float distance=dot(p.position-contact,n);
    if(distance<0&&(distance>=-thickness||dot(previous-contact,n)>=0)){
        p.position-=distance*n;float vn=dot(p.velocity,n);if(vn<0){p.velocity-=(1+restitution)*vn*n;events|=1u;}
    }
}
