// Ara baseline v1. Column-major matrices, metres, linear HDR, reversed-Z.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
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
// ara_render::scene::{BASE_COLOR_DECODE_SRGB,EMISSIVE_DECODE_SRGB}.
// Native graph sampleColor filters raw UNORM, then decodes; glTF views decode in hardware.
float3 materialLinear(float3 x){return lerp(x/12.92,pow((max(x,0)+0.055)/1.055,2.4),step(0.04045,x));}
Surface surface(Varying v) {
    Material m=buffers[NonUniformResourceIndex(root.materials)].Load<Material>(root.material*80);
    Surface s;s.ao=1;s.transmission=0;float4 base=tex(m.textures.x,v.uv);
    if((m.flags.y&(1u<<25))!=0)base.rgb=materialLinear(base.rgb);
    base*=m.color;
    if(m.flags.x==1 && base.a<m.surface.w)discard;
    s.albedo=base.rgb;s.alpha=base.a;
    float3 n=normalize(v.normal);float3 t=normalize(v.tangent.xyz-n*dot(v.tangent.xyz,n));float3 b=cross(n,t)*v.tangent.w;
    float3 normal=tex(m.textures.y,v.uv).xyz*2-1;normal.xy*=m.surface.y;s.normal=normalize(t*normal.x+b*normal.y+n*normal.z);
    float4 mr=tex(m.textures.z,v.uv);s.roughness=clamp(m.emissiveRoughness.w*mr.g,0.045,1);s.metal=saturate(m.surface.x*mr.b);
    // Geometric specular AA (Tokuyoshi & Kaplanyan 2019): widen GGX alpha^2 by the
    // pixel's shading-normal variance so sub-pixel normal detail cannot sparkle.
    float3 du=ddx(s.normal),dv=ddy(s.normal);float kernel=min(2*0.15915494*(dot(du,du)+dot(dv,dv)),0.18);
    s.roughness=sqrt(sqrt(saturate(pow(s.roughness,4)+kernel)));
    float3 emission=tex(m.textures.w,v.uv).rgb;
    if((m.flags.y&(1u<<26))!=0)emission=materialLinear(emission);
    s.emissive=emission*m.emissiveRoughness.rgb;return s;
}
struct GBuffer {float4 albedo:SV_Target0;float4 normal:SV_Target1;float4 material:SV_Target2;float4 world:SV_Target3;float id:SV_Target4;};
[shader("fragment")]
GBuffer geometryFragment(Varying v) {
    Surface s=surface(v);GBuffer g;g.albedo=float4(s.albedo,s.ao);g.normal=float4(s.normal,s.roughness);g.material=float4(s.emissive,s.metal);g.world=float4(v.world,1);g.id=float(root.entity);return g;
}
[shader("fragment")]
void shadowFragment(Varying v) {Surface s=surface(v);}
struct FullscreenVarying {float4 clip:SV_Position;};
[shader("vertex")]
FullscreenVarying fullscreenVertex(uint id:SV_VertexID){FullscreenVarying o;float2 uv=float2((id<<1)&2,id&2);o.clip=float4(uv*2-1,0,1);return o;}
[shader("fragment")]
float4 lightingFragment(FullscreenVarying v):SV_Target0 {
    Frame f=frame();Look l=look(f);int2 p=int2(v.clip.xy);float2 uv=(float2(p)+0.5)/float2(f.extent.xy);float4 world=pixel(f.gbuffer.w,p);
    if(world.w==0)return float4(background(f,l,viewRay(f,float2(p)+0.5),uv),1);
    Surface s;float4 a=pixel(f.gbuffer.x,p);float4 n=pixel(f.gbuffer.y,p);float4 m=pixel(f.gbuffer.z,p);s.albedo=a.rgb;s.alpha=1;s.transmission=0;s.ao=min(a.a,screenOcclusion(f,l,p,length(world.xyz-f.camera.xyz)));s.normal=n.xyz;s.roughness=n.w;s.metal=m.w;s.emissive=m.xyz;
    return float4(applyFog(f,l,shade(f,l,s,world.xyz,p,true),world.xyz,uv),1);
}
struct ForwardOut {float4 color:SV_Target0;float id:SV_Target1;float4 albedo:SV_Target2;float4 normal:SV_Target3;float4 material:SV_Target4;float4 world:SV_Target5;};
[shader("fragment")]
ForwardOut forwardFragment(Varying v){Frame f=frame();Surface s=surface(v);ForwardOut o;Look l=look(f);o.color=forwardColour(f,l,s,v.world,int2(v.clip.xy));o.id=float(root.entity);o.albedo=float4(s.albedo,s.alpha);o.normal=float4(s.normal,s.roughness);o.material=float4(s.emissive,s.metal);o.world=float4(v.world,1);return o;}
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
// Temporal antialiasing of the lit HDR in `root.materials` (the renderer's HDR, or
// a composite handed to `Renderer::finish`) into the history image `f.post.z`.
// History reprojects with the native motion texture (Frame.look.w: unjittered
// current-minus-previous UV) when present, otherwise with the static world position,
// and is clamped to the current 3x3 neighbourhood.
[shader("compute")]
[numthreads(8,8,1)]
void taaMain(uint3 t:SV_DispatchThreadID){
    Frame f=frame();if(any(t.xy>=f.extent.xy))return;int2 p=int2(t.xy);uint input=root.materials;
    float3 current=pixel(input,p).rgb;float depth=pixel(f.extra.x,p).x;float3 result=current;
    if(f.temporal.x!=0 && f.temporal.y==0){
        float2 uv;bool valid;
        if(f.look.w!=0xffffffffu){
            uv=(float2(p)+0.5)/float2(f.extent.xy)-pixel(f.look.w,p).xy;valid=all(uv>0)&&all(uv<1);
        }else{
            float4 world=pixel(f.gbuffer.w,p);float4 previous=mul(f.previousVp,float4(world.xyz,1));uv=float2(previous.x/previous.w*0.5+0.5,0.5-previous.y/previous.w*0.5);
            float4 history=historyTex(f.post.y,uv);
            valid=world.w>0 && previous.w>0 && all(uv>0)&&all(uv<1)&&abs(history.w-previous.z/previous.w)<0.002;
        }
        if(valid){
            float3 low=current,high=current;
            for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){float3 c=pixel(input,clamp(p+int2(x,y),int2(0),int2(f.extent.xy)-1)).rgb;low=min(low,c);high=max(high,c);}
            result=lerp(current,clamp(historyTex(f.post.y,uv).rgb,low,high),0.9);
        }
    }
    images[NonUniformResourceIndex(f.post.z)][p]=float4(result,depth);
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
