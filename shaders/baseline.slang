// Ara baseline v1. Column-major matrices, metres, linear HDR, reversed-Z.
struct Root { uint frame; uint materials; uint material; uint entity; uint cascade; uint mode; uint sample; uint flags; };
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
BIND(3) RWTexture2D<float4> images[64] : register(u0,space3);
static const float PI=3.14159265359;
struct Frame {
    float4x4 vp;
    float4x4 previousVp;
    float4x4 inverseVp;
    float4x4 shadows[4];
    float4 camera;
    float4 forward;
    float4 sunDirection;
    float4 sunColorExposure;
    float4 splits;
    uint4 extent;
    uint4 gbuffer;
    uint4 extra;
    uint4 lighting;
    uint4 post;
    uint4 temporal;
};
struct Material { float4 color; float4 emissiveRoughness; float4 surface; uint4 textures; uint4 flags; };
struct Light {float4 positionRange;float4 colorLumens;float4 directionOuter;float4 parameters;};
Frame frame() {return buffers[NonUniformResourceIndex(root.frame)].Load<Frame>(0);}
float4 tex(uint index,float2 uv) {return textures[NonUniformResourceIndex(index)].SampleLevel(samplers[0],uv,0);}
float4 envTex(uint index,float2 uv) {return textures[NonUniformResourceIndex(index)].SampleLevel(samplers[1],uv,0);}
float4 historyTex(uint index,float2 uv) {return textures[NonUniformResourceIndex(index)].SampleLevel(samplers[2],uv,0);}
float4 pixel(uint index,int2 p) {return textures[NonUniformResourceIndex(index)].Load(int3(p,0));}
float2 environmentUv(float3 d) {return float2(atan2(d.z,d.x)/(2*PI)+0.5,acos(clamp(d.y,-1.,1.))/PI);}
float3 environmentDirection(float2 uv) {float phi=(uv.x-0.5)*2*PI;float theta=uv.y*PI;return float3(cos(phi)*sin(theta),cos(theta),sin(phi)*sin(theta));}
struct Vertex {float3 position:POSITION;float3 normal:NORMAL;float2 uv:TEXCOORD0;float4 tangent:TANGENT;};
struct Varying {float4 clip:SV_Position;float3 world:TEXCOORD0;float3 normal:TEXCOORD1;float2 uv:TEXCOORD2;float4 tangent:TEXCOORD3;};
[shader("vertex")]
Varying geometryVertex(Vertex v) {
    Varying o;Frame f=frame();o.clip=mul(f.vp,float4(v.position,1));o.world=v.position;o.normal=v.normal;o.uv=v.uv;o.tangent=v.tangent;return o;
}
[shader("vertex")]
Varying shadowVertex(Vertex v) {
    Varying o;Frame f=frame();o.clip=mul(f.shadows[root.cascade],float4(v.position,1));o.world=v.position;o.normal=v.normal;o.uv=v.uv;o.tangent=v.tangent;return o;
}
struct Surface {float3 albedo;float alpha;float3 normal;float roughness;float metal;float3 emissive;};
Surface surface(Varying v) {
    Material m=buffers[NonUniformResourceIndex(root.materials)].Load<Material>(root.material*80);
    Surface s;float4 base=tex(m.textures.x,v.uv)*m.color;
    if(m.flags.x==1 && base.a<m.surface.w)discard;
    s.albedo=base.rgb;s.alpha=base.a;
    float3 n=normalize(v.normal);float3 t=normalize(v.tangent.xyz-n*dot(v.tangent.xyz,n));float3 b=cross(n,t)*v.tangent.w;
    float3 normal=tex(m.textures.y,v.uv).xyz*2-1;normal.xy*=m.surface.y;s.normal=normalize(t*normal.x+b*normal.y+n*normal.z);
    float4 mr=tex(m.textures.z,v.uv);s.roughness=clamp(m.emissiveRoughness.w*mr.g,0.045,1);s.metal=saturate(m.surface.x*mr.b);
    s.emissive=tex(m.textures.w,v.uv).rgb*m.emissiveRoughness.rgb;return s;
}
struct GBuffer {float4 albedo:SV_Target0;float4 normal:SV_Target1;float4 material:SV_Target2;float4 world:SV_Target3;float id:SV_Target4;};
[shader("fragment")]
GBuffer geometryFragment(Varying v) {
    Surface s=surface(v);GBuffer g;g.albedo=float4(s.albedo,s.alpha);g.normal=float4(s.normal,s.roughness);g.material=float4(s.emissive,s.metal);g.world=float4(v.world,1);g.id=float(root.entity);return g;
}
[shader("fragment")]
void shadowFragment(Varying v) {Surface s=surface(v);}
uint cascadeIndex(Frame f,float3 world){float d=dot(world-f.camera.xyz,f.forward.xyz);return d<f.splits.x?0:d<f.splits.y?1:d<f.splits.z?2:3;}
float visibility(Frame f,float3 world,float3 normal) {
    uint c=cascadeIndex(f,world);float4 p=mul(f.shadows[c],float4(world+normal*0.015,1));float3 q=p.xyz/p.w;
    float2 uv=float2(q.x*0.5+0.5,0.5-q.y*0.5);
    if(any(uv<0)||any(uv>1)||q.z<0||q.z>1)return 1;
    int2 origin=int2(c%2,c/2)*512;int2 at=origin+int2(uv*511);float sum=0;
    for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
        int2 sample=clamp(at+int2(x,y),origin,origin+511);
        float depth=pixel(f.extra.z,sample).x;sum+=(q.z+0.0015>=depth)?1.:0.;
    }
    return sum/9;
}
float3 fresnel(float cosine,float3 f0){return f0+(1-f0)*pow(1-saturate(cosine),5);}
float3 brdf(Surface s,float3 v,float3 l) {
    float3 h=normalize(v+l);float nl=saturate(dot(s.normal,l));float nv=max(dot(s.normal,v),0.001);float nh=saturate(dot(s.normal,h));float vh=saturate(dot(v,h));
    float a=s.roughness*s.roughness;float a2=a*a;float d=a2/(PI*pow(nh*nh*(a2-1)+1,2));
    float k=pow(s.roughness+1,2)/8;float g=nv/(nv*(1-k)+k)*nl/(nl*(1-k)+k);
    float3 F=fresnel(vh,lerp(float3(0.04),s.albedo,s.metal));
    return ((1-F)*(1-s.metal)*s.albedo/PI+d*g*F/max(4*nv*nl,0.001))*nl;
}
float3 ibl(Frame f,Surface s,float3 v) {
    float nv=saturate(dot(s.normal,v));float3 F=fresnel(nv,lerp(float3(0.04),s.albedo,s.metal));
    float3 diffuse=envTex(f.lighting.x,environmentUv(s.normal)).rgb*s.albedo*(1-s.metal)*(1-F)/PI;
    float3 reflected=reflect(-v,s.normal);float2 uv=environmentUv(reflected);float level=s.roughness*4;float low=floor(level);float high=min(low+1,4);
    float3 spec=lerp(envTex(f.lighting.y,float2(uv.x,(uv.y*127+0.5+low*128)/640)).rgb,envTex(f.lighting.y,float2(uv.x,(uv.y*127+0.5+high*128)/640)).rgb,frac(level));
    // Analytic split-sum environment BRDF fit (Lazarov, Epic 2014).
    float4 r=s.roughness*float4(-1,-0.0275,-0.572,0.022)+float4(1,0.0425,1.04,-0.04);
    float a004=min(r.x*r.x,exp2(-9.28*nv))*r.x+r.y;float2 ab=float2(-1.04,1.04)*a004+r.zw;
    return diffuse+spec*(F*ab.x+ab.y);
}
float3 shade(Frame f,Surface s,float3 world,int2 p) {
    float3 v=normalize(f.camera.xyz-world);float3 result=s.emissive+ibl(f,s,v);
    result+=brdf(s,v,f.sunDirection.xyz)*f.sunColorExposure.rgb*visibility(f,world,s.normal);
    uint tilesX=(f.extent.x+15)/16;uint tile=(uint(p.y)/16*tilesX+uint(p.x)/16)*65;
    uint count=buffers[NonUniformResourceIndex(f.lighting.z)].Load(tile*4);
    for(uint i=0;i<count;i++) {
        uint index=buffers[NonUniformResourceIndex(f.lighting.z)].Load((tile+1+i)*4);
        Light light=buffers[NonUniformResourceIndex(f.lighting.w)].Load<Light>(index*64);
        float3 delta=light.positionRange.xyz-world;float d2=max(dot(delta,delta),0.01);float d=sqrt(d2);float3 l=delta/d;
        float fade=pow(saturate(1-pow(d/light.positionRange.w,4)),2);float spot=1;
        if(light.parameters.x>0.5)spot=smoothstep(light.directionOuter.w,light.parameters.y,dot(-l,light.directionOuter.xyz));
        float solidAngle=light.parameters.x>0.5?2*PI*(1-light.directionOuter.w):4*PI;
        float3 illuminance=light.colorLumens.rgb*(light.colorLumens.w/(solidAngle*d2))*fade*spot;
        result+=brdf(s,v,l)*illuminance;
    }
    return result;
}
struct FullscreenVarying {float4 clip:SV_Position;};
[shader("vertex")]
FullscreenVarying fullscreenVertex(uint id:SV_VertexID){FullscreenVarying o;float2 uv=float2((id<<1)&2,id&2);o.clip=float4(uv*2-1,0,1);return o;}
[shader("fragment")]
float4 lightingFragment(FullscreenVarying v):SV_Target0 {
    Frame f=frame();int2 p=int2(v.clip.xy);float4 world=pixel(f.gbuffer.w,p);
    if(world.w==0){float2 uv=(float2(p)+0.5)/float2(f.extent.xy);float4 far=mul(f.inverseVp,float4(uv.x*2-1,1-uv.y*2,0.00001,1));float3 d=normalize(far.xyz/far.w-f.camera.xyz);return float4(envTex(f.extra.w,environmentUv(d)).rgb,1);}
    Surface s;float4 a=pixel(f.gbuffer.x,p);float4 n=pixel(f.gbuffer.y,p);float4 m=pixel(f.gbuffer.z,p);s.albedo=a.rgb;s.alpha=a.a;s.normal=n.xyz;s.roughness=n.w;s.metal=m.w;s.emissive=m.xyz;
    return float4(shade(f,s,world.xyz,p),1);
}
struct ForwardOut {float4 color:SV_Target0;float id:SV_Target1;float4 albedo:SV_Target2;float4 normal:SV_Target3;float4 material:SV_Target4;float4 world:SV_Target5;};
[shader("fragment")]
ForwardOut forwardFragment(Varying v){Frame f=frame();Surface s=surface(v);ForwardOut o;o.color=float4(shade(f,s,v.world,int2(v.clip.xy))*s.alpha,s.alpha);o.id=float(root.entity);o.albedo=float4(s.albedo,s.alpha);o.normal=float4(s.normal,s.roughness);o.material=float4(s.emissive,s.metal);o.world=float4(v.world,1);return o;}
float radicalInverse(uint bits){bits=(bits<<16)|(bits>>16);bits=((bits&0x55555555)<<1)|((bits&0xAAAAAAAA)>>1);bits=((bits&0x33333333)<<2)|((bits&0xCCCCCCCC)>>2);bits=((bits&0x0F0F0F0F)<<4)|((bits&0xF0F0F0F0)>>4);bits=((bits&0x00FF00FF)<<8)|((bits&0xFF00FF00)>>8);return float(bits)*2.3283064365386963e-10;}
float3 basisSample(float3 n,float3 local){float3 t=normalize(cross(abs(n.y)<0.99?float3(0,1,0):float3(1,0,0),n));return t*local.x+cross(n,t)*local.y+n*local.z;}
[shader("compute")]
[numthreads(8,8,1)]
void irradianceMain(uint3 t:SV_DispatchThreadID){if(t.x>=32||t.y>=16)return;Frame f=frame();float3 n=environmentDirection((float2(t.xy)+0.5)/float2(32,16));float3 sum=0;
    for(uint i=0;i<1024;i++){float2 u=float2((i+0.5)/1024.,radicalInverse(i));float phi=2*PI*u.x;float r=sqrt(u.y);float3 d=basisSample(n,float3(cos(phi)*r,sin(phi)*r,sqrt(1-u.y)));sum+=envTex(f.extra.w,environmentUv(d)).rgb;}
    images[NonUniformResourceIndex(root.materials)][t.xy]=float4(sum*PI/1024,1);
}
[shader("compute")]
[numthreads(8,8,1)]
void prefilterMain(uint3 t:SV_DispatchThreadID){if(t.x>=256||t.y>=640)return;Frame f=frame();uint level=t.y/128;float roughness=max(float(level)/4,0.045);float3 n=environmentDirection((float2(t.x,t.y%128)+0.5)/float2(256,128));float3 sum=0;float weight=0;
    for(uint i=0;i<1024;i++){float2 u=float2((i+0.5)/1024.,radicalInverse(i));float phi=2*PI*u.x;float a=roughness*roughness;float cosine=sqrt((1-u.y)/(1+(a*a-1)*u.y));float sine=sqrt(max(0.,1-cosine*cosine));float3 h=basisSample(n,float3(cos(phi)*sine,sin(phi)*sine,cosine));float3 l=reflect(-n,h);float nl=saturate(dot(n,l));if(nl>0){sum+=envTex(f.extra.w,environmentUv(l)).rgb*nl;weight+=nl;}}
    images[NonUniformResourceIndex(root.materials)][t.xy]=float4(sum/max(weight,0.001),1);
}
[shader("compute")]
[numthreads(64,1,1)]
void tileMain(uint3 t:SV_DispatchThreadID){Frame f=frame();uint tilesX=(f.extent.x+15)/16;uint tilesY=(f.extent.y+15)/16;if(t.x>=tilesX*tilesY)return;uint offset=t.x*65;uint count=0;float2 tileMin=float2(t.x%tilesX,t.x/tilesX)*16;float2 tileMax=tileMin+16;
    for(uint i=0;i<f.extent.z;i++){
        Light l=buffers[NonUniformResourceIndex(f.lighting.w)].Load<Light>(i*64);float4 clip=mul(f.vp,float4(l.positionRange.xyz,1));float range=l.positionRange.w;
        bool include=clip.w<=range;
        if(!include){float2 centre=float2(clip.x/clip.w*0.5+0.5,0.5-clip.y/clip.w*0.5)*float2(f.extent.xy);float radius=range*max(abs(f.vp[0][0]),abs(f.vp[1][1]))*max(f.extent.x,f.extent.y)/max(clip.w-range,0.1);include=all(centre+radius>=tileMin)&&all(centre-radius<=tileMax);}
        if(include){buffers[NonUniformResourceIndex(f.lighting.z)].Store((offset+1+count)*4,i);count++;}
    }
    buffers[NonUniformResourceIndex(f.lighting.z)].Store(offset*4,count);
}
[shader("compute")]
[numthreads(8,8,1)]
void taaMain(uint3 t:SV_DispatchThreadID){Frame f=frame();if(any(t.xy>=f.extent.xy))return;int2 p=int2(t.xy);float3 current=pixel(f.post.x,p).rgb;float depth=pixel(f.extra.x,p).x;float3 result=current;
    if(f.temporal.x!=0 && f.temporal.y==0){float4 world=pixel(f.gbuffer.w,p);float4 previous=mul(f.previousVp,float4(world.xyz,1));float2 uv=float2(previous.x/previous.w*0.5+0.5,0.5-previous.y/previous.w*0.5);float4 history=historyTex(f.post.y,uv);
        if(world.w>0 && previous.w>0 && all(uv>0)&&all(uv<1)&&abs(history.w-previous.z/previous.w)<0.002){float3 low=current;float3 high=current;
            for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){float3 c=pixel(f.post.x,clamp(p+int2(x,y),int2(0),int2(f.extent.xy)-1)).rgb;low=min(low,c);high=max(high,c);}
            result=lerp(current,clamp(history.rgb,low,high),0.9);
        }
    }
    images[NonUniformResourceIndex(f.post.z)][p]=float4(result,depth);
}
float3 srgb(float3 x){return lerp(12.92*x,1.055*pow(max(x,0),1/2.4)-0.055,step(0.0031308,x));}
// Display curves shared with the fidelity post pass (forward.w: 0 ACES fit, 1 PBR
// Neutral, 2 fitted ACES filmic pre-scaled to keep 18% grey).
float3 tonemap(float3 x){return saturate((x*(2.51*x+0.03))/(x*(2.43*x+0.59)+0.14));}
float3 neutralCurve(float3 c){
    float minimum=min(c.x,min(c.y,c.z));float offset=minimum<=0.08?minimum-6.25*minimum*minimum:0.04;c-=offset;
    float peak=max(c.x,max(c.y,c.z));if(peak<=0.76)return c;
    float compressed=1-0.0576/(peak-0.52);float blend=1-1/(1+0.15*(peak-compressed));return lerp(c*(compressed/peak),float3(compressed),blend);
}
float3 filmicCurve(float3 c){
    c*=1.464;
    float3 v=float3(dot(float3(0.59719,0.35458,0.04823),c),dot(float3(0.07600,0.90834,0.01566),c),dot(float3(0.02840,0.13383,0.83777),c));
    v=(v*(v+0.0245786)-0.000090537)/(v*(0.983729*v+0.4329510)+0.238081);
    return saturate(float3(dot(float3(1.60475,-0.53108,-0.07367),v),dot(float3(-0.10208,1.10813,-0.00605),v),dot(float3(-0.00327,-0.07276,1.07602),v)));
}
float3 displayCurve(Frame f,float3 x){uint curve=uint(f.forward.w);return curve==1?neutralCurve(x):curve==2?filmicCurve(x):tonemap(x);}
[shader("compute")]
[numthreads(8,8,1)]
void tonemapMain(uint3 t:SV_DispatchThreadID){Frame f=frame();if(any(t.xy>=f.extent.xy))return;int2 p=int2(t.xy);float3 color=pixel(root.materials,p).rgb;
    if(f.temporal.z==1)color=pixel(f.gbuffer.x,p).rgb;
    else if(f.temporal.z==2)color=pixel(f.gbuffer.y,p).xyz*0.5+0.5;
    else if(f.temporal.z==3)color=float3(pow(pixel(f.extra.x,p).x,0.25));
    else if(f.temporal.z==4){float4 n=pixel(f.gbuffer.y,p);float4 m=pixel(f.gbuffer.z,p);color=float3(n.w,m.w,0);}
    else if(f.temporal.z==5){uint id=uint(pixel(f.extra.y,p).x);color=id==0?float3(0):frac(float3(id)*float3(0.6180339,0.381966,0.7548776));}
    else if(f.temporal.z==6){float4 w=pixel(f.gbuffer.w,p);uint c=cascadeIndex(f,w.xyz);color=w.w==0?float3(0):(c==0?float3(0.85,0.2,0.1):c==1?float3(0.2,0.8,0.25):c==2?float3(0.2,0.35,0.9):float3(0.8,0.3,0.8))*visibility(f,w.xyz,pixel(f.gbuffer.y,p).xyz);}
    if(f.temporal.z==0)color=displayCurve(f,max(color,0)*f.sunColorExposure.w);
    images[NonUniformResourceIndex(f.post.w)][p]=float4(srgb(saturate(color)),1);
}
