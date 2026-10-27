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
struct GBuffer {float4 albedo:SV_Target0;float4 normal:SV_Target1;float4 material:SV_Target2;float4 emissive:SV_Target3;float id:SV_Target4;};
[shader("fragment")]
GBuffer geometryFragment(Varying v) {
    Surface s=surface(v);GBuffer g;g.albedo=float4(s.albedo,s.ao);g.normal=gbufferEncodeNormal(s.normal,1);g.material=gbufferEncodeMaterial(s.roughness,s.metal,1,1);g.emissive=gbufferEncodeEmissive(s.emissive,1);g.id=float(root.entity);return g;
}
[shader("fragment")]
void shadowFragment(Varying v) {Surface s=surface(v);}
struct FullscreenVarying {float4 clip:SV_Position;};
[shader("vertex")]
FullscreenVarying fullscreenVertex(uint id:SV_VertexID){FullscreenVarying o;float2 uv=float2((id<<1)&2,id&2);o.clip=float4(uv*2-1,0,1);return o;}
// Deferred lighting; `high` selects the High/Epic tier's code at compile time,
// `rays` the hybrid path's ray-traced reflections (renderer::rt, D29).
float4 lighting(FullscreenVarying v,bool high,bool rays=false){
    Frame f=frame();Look l=look(f);int2 p=int2(v.clip.xy);float2 uv=(float2(p)+0.5)/float2(f.extent.xy);bool covered;float3 world=gbufferWorld(f,p,covered);
    if(!covered)return float4(background(f,l,viewRay(f,float2(p)+0.5),uv),1);
    Surface s;float4 a=gbufferAlbedo(f,p);float2 m=gbufferMaterial(f,p);s.albedo=a.rgb;s.alpha=1;s.transmission=0;s.ao=min(a.a,screenOcclusion(f,l,p,length(world-f.camera.xyz),high));s.normal=gbufferNormal(f,p);s.roughness=m.x;s.metal=m.y;s.emissive=gbufferEmissive(f,p);
    return float4(applyFog(f,l,shade(f,l,s,world,p,true,high,rays),world,uv),1);
}
[shader("fragment")]
float4 lightingFragment(FullscreenVarying v):SV_Target0 {return lighting(v,false);}
[shader("fragment")]
float4 lightingHighFragment(FullscreenVarying v):SV_Target0 {return lighting(v,true);}
// High/Epic with ray-traced reflections (only bound on frames that trace them).
[shader("fragment")]
float4 lightingHighRtFragment(FullscreenVarying v):SV_Target0 {return lighting(v,true,true);}
struct ForwardOut {float4 color:SV_Target0;float id:SV_Target1;float4 albedo:SV_Target2;float4 normal:SV_Target3;float4 material:SV_Target4;float4 emissive:SV_Target5;};
[shader("fragment")]
ForwardOut forwardFragment(Varying v){Frame f=frame();Surface s=surface(v);ForwardOut o;Look l=look(f);o.color=forwardColour(f,l,s,v.world,int2(v.clip.xy));o.id=float(root.entity);o.albedo=float4(s.albedo,s.alpha);o.normal=gbufferEncodeNormal(s.normal,s.alpha);o.material=gbufferEncodeMaterial(s.roughness,s.metal,1,s.alpha);o.emissive=gbufferEncodeEmissive(s.emissive,s.alpha);return o;}
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
void tileMain(uint3 t:SV_DispatchThreadID){Frame f=frame();uint tilesX=(f.extent.x+15)/16;uint tilesY=(f.extent.y+15)/16;if(t.x>=tilesX*tilesY)return;uint offset=t.x*TILE_STRIDE;uint count=0;float2 tileMin=float2(t.x%tilesX,t.x/tilesX)*16;float2 tileMax=tileMin+16;
    for(uint i=0;i<f.extent.z;i++){
        Light l=buffers[NonUniformResourceIndex(f.lighting.w)].Load<Light>(i*64);float4 clip=mul(f.vp,float4(l.positionRange.xyz,1));float range=l.positionRange.w;
        bool include=clip.w<=range;
        if(!include){float2 centre=float2(clip.x/clip.w*0.5+0.5,0.5-clip.y/clip.w*0.5)*float2(f.extent.xy);float radius=range*max(abs(f.vp[0][0]),abs(f.vp[1][1]))*max(f.extent.x,f.extent.y)/max(clip.w-range,0.1);include=all(centre+radius>=tileMin)&&all(centre-radius<=tileMax);}
        if(include && count<TILE_LIGHTS){buffers[NonUniformResourceIndex(f.lighting.z)].Store((offset+1+count)*4,i);count++;}
    }
    buffers[NonUniformResourceIndex(f.lighting.z)].Store(offset*4,count);
}
float3 toYCoCg(float3 c){return float3(dot(c,float3(0.25,0.5,0.25)),dot(c,float3(0.5,0,-0.5)),dot(c,float3(-0.25,0.5,-0.25)));}
float3 fromYCoCg(float3 c){return float3(c.x+c.y-c.z,c.x+c.z,c.x-c.y-c.z);}
// History through a Catmull-Rom filter (five bilinear taps): reprojection keeps
// detail that plain bilinear fetches would blur a little more every frame.
float3 historyCatmullRom(Frame f,float2 uv){
    float2 size=float2(f.extent.xy),position=uv*size,centre=floor(position-0.5)+0.5,t=position-centre;
    float2 w0=t*(-0.5+t*(1-0.5*t)),w1=1+t*t*(-2.5+1.5*t),w2=t*(0.5+t*(2-1.5*t)),w3=t*t*(-0.5+0.5*t);
    float2 w12=w1+w2,p0=(centre-1)/size,p3=(centre+2)/size,p12=(centre+w2/w12)/size;
    float3 sum=historyTex(f.post.y,float2(p12.x,p0.y)).rgb*(w12.x*w0.y)+historyTex(f.post.y,float2(p0.x,p12.y)).rgb*(w0.x*w12.y)
        +historyTex(f.post.y,p12).rgb*(w12.x*w12.y)+historyTex(f.post.y,float2(p3.x,p12.y)).rgb*(w3.x*w12.y)
        +historyTex(f.post.y,float2(p12.x,p3.y)).rgb*(w12.x*w3.y);
    return max(sum/(w12.x*w0.y+w0.x*w12.y+w12.x*w12.y+w3.x*w12.y+w12.x*w3.y),0);
}
// Temporal antialiasing and upscaling of the lit HDR in `root.materials` (the
// renderer's HDR, or a composite handed to `Renderer::finish`) into the history
// image `f.post.z`. Runs with the display frame record: the input, depth and motion
// are at the render size f.sizes.xy, the history at the display extent.
// History reprojects with the native motion texture (Frame.look.w: unjittered
// current-minus-previous UV) when present, otherwise with the static world position;
// it is clipped to the 3x3 render neighbourhood's YCoCg mean and variance and
// blended with exposed-luminance weights (no flicker from bright subpixel detail).
// Upscaled, the current frame is reconstructed from the jittered samples around the
// pixel (Frame.upscale.xy places them): a Blackman-Harris-like Gaussian in display
// pixels, each sample weighted by its exposed luminance so lone HDR points (lamps,
// sparks) cannot spill over the window. It blends in by how near its nearest
// sample lies, so history gathers the detail between samples. Native frames take
// their own pixel, as plain TAA. Resampling softens history a little every frame, so
// moving pixels (fully from four display pixels a frame, and pixels without history)
// lean on the current frame (weight up to 0.4) reconstructed with a kernel twice as
// wide in area, so render-pixel steps do not show; pixels at rest keep the long
// accumulation.
// History is RGBA16F, pre-exposed (look.slangh exposureState); w keeps the depth.
[shader("compute")]
[numthreads(8,8,1)]
void taaMain(uint3 t:SV_DispatchThreadID){
    Frame f=frame();if(any(t.xy>=f.extent.xy))return;int2 p=int2(t.xy);uint input=root.materials;
    float4 state=exposureState(look(f));float exposure=exposureScale(f,state.x),previous=exposureScale(f,state.y);
    int2 size=int2(f.sizes.xy);bool upscaled=any(size<int2(f.extent.xy));
    // This pixel's centre in render pixels, and the render pixel whose jittered
    // sample lies nearest it (the pixel itself when native).
    float2 uv=(float2(p)+0.5)/float2(f.extent.xy),at=uv*float2(size),toDisplay=float2(f.extent.xy)/float2(size);
    int2 c=clamp(int2(floor(at-f.upscale.xy)),int2(0),size-1);
    float3 current=pixel(input,c).rgb;float depth=pixel(f.extra.x,c).x;float3 result=current;
    bool temporal=f.temporal.x!=0 && f.temporal.y==0;
    if(temporal || upscaled){
        // History position first: how far this pixel moved shapes both blends below.
        float2 hv=0;bool valid=false;
        if(temporal && f.look.w!=0xffffffffu){
            hv=uv-pixel(f.look.w,c).xy;valid=all(hv>0)&&all(hv<1);
        }else if(temporal){
            // The sample's world point, and how far it moved from where last
            // frame's (unjittered) history saw it.
            float2 s=(float2(c)+0.5)/float2(size);float4 h=mul(f.inverseVp,float4(s.x*2-1,1-s.y*2,depth,1));
            float4 prior=mul(f.previousVp,float4(h.xyz/h.w,1));
            hv=uv-(s+f.upscale.xy/float2(size))+float2(prior.x/prior.w*0.5+0.5,0.5-prior.y/prior.w*0.5);
            float4 history=historyTex(f.post.y,hv);
            valid=depth>0 && prior.w>0 && all(hv>0)&&all(hv<1)&&abs(history.w-prior.z/prior.w)<0.002;
        }
        // 0 at rest, 1 from four display pixels a frame (or without history): every
        // frame's resampling softens history, so moving pixels lean on the current
        // frame, reconstructed wider so render-pixel steps do not show.
        float moving=valid?saturate(length((uv-hv)*float2(f.extent.xy))/4):1;
        float3 mean=0,square=0,sum=0;float total=0;float spread=lerp(5.56,2.78,moving);
        for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
            int2 q=clamp(c+int2(x,y),int2(0),size-1);float3 s=pixel(input,q).rgb,v=toYCoCg(s);mean+=v;square+=v*v;
            float2 d=(float2(q)+0.5+f.upscale.xy-at)*toDisplay;float w=exp(-spread*dot(d,d))/(1+luminance(s)*exposure);sum+=s*w;total+=w;
        }
        mean/=9;float3 sigma=sqrt(max(square/9-mean*mean,0));
        if(upscaled)current=sum/total;
        result=current;
        if(valid){
            float3 history=toYCoCg(historyCatmullRom(f,hv)/previous),low=mean-1.25*sigma,high=mean+1.25*sigma;
            // Clip towards the neighbourhood mean (not a per-channel clamp).
            float3 delta=history-mean,extent=max(high-mean,1e-5);float3 scale=abs(delta)/extent;float m=max(scale.x,max(scale.y,scale.z));
            history=fromYCoCg(m>1?mean+delta/m:history);
            // Upscaled, the nearest sample's distance in display pixels sets how much
            // the current frame counts; motion raises it.
            float2 near=(float2(c)+0.5+f.upscale.xy-at)*toDisplay;
            float alpha=lerp(upscaled?0.15:0.1,0.4,moving)*(upscaled?exp(-2.29*dot(near,near)):1);
            float wc=alpha/(1+luminance(current)*exposure),wh=(1-alpha)/(1+luminance(history)*exposure);
            result=(current*wc+history*wh)/(wc+wh);
        }
    }
    images[NonUniformResourceIndex(f.post.z)][p]=float4(min(result*exposure,65000),depth);
}
float3 displayCurve(Frame f,float3 x){uint curve=uint(f.forward.w);return curve>=3?agxCurve(x,curve-3):curve==1?neutralCurve(x):curve==2?filmicCurve(x):tonemap(x);}
[shader("compute")]
[numthreads(8,8,1)]
void tonemapMain(uint3 t:SV_DispatchThreadID){Frame f=frame();if(any(t.xy>=f.extent.xy))return;int2 p=int2(t.xy);float3 color=pixel(root.materials,p).rgb;
    // Debug views read the render-size G-buffer pixel `r` under this display pixel.
    Frame g=f;g.extent.xy=f.sizes.xy;int2 r=int2((float2(p)+0.5)*float2(f.sizes.xy)/float2(f.extent.xy));
    if(f.temporal.z==1)color=gbufferAlbedo(g,r).rgb;
    else if(f.temporal.z==2)color=gbufferDepth(g,r)>0?gbufferNormal(g,r)*0.5+0.5:0.5;
    else if(f.temporal.z==3)color=float3(pow(pixel(f.extra.x,r).x,0.25));
    else if(f.temporal.z==4)color=float3(gbufferMaterial(g,r),0);
    else if(f.temporal.z==5){uint id=uint(pixel(f.extra.y,r).x);color=id==0?float3(0):frac(float3(id)*float3(0.6180339,0.381966,0.7548776));}
    else if(f.temporal.z==7)color=gbufferDepth(g,r)>0?float3(0.055):float3(0.025);
    else if(f.temporal.z==6){bool covered;float3 w=gbufferWorld(g,r,covered);uint c=cascadeIndex(f,w);color=!covered?float3(0):(c==0?float3(0.85,0.2,0.1):c==1?float3(0.2,0.8,0.25):c==2?float3(0.2,0.35,0.9):float3(0.8,0.3,0.8))*visibility(f,look(f),w,gbufferNormal(g,r));}
    if(f.temporal.z==0)color=displayCurve(f,max(color,0)*f.sunColorExposure.w);
    images[NonUniformResourceIndex(f.post.w)][p]=float4(srgb(saturate(color)),1);
}
// Wireframe view modes: the wire mask (root word 1, render size, nativeWireFragment)
// over the display output (Frame.post.w, sRGB). Over the lit image (root word 5 set)
// edges take one colour; over the wireframe view's background each entity has its
// own, from the ID target (root word 2).
[shader("compute")]
[numthreads(8,8,1)]
void wireCompositeMain(uint3 t:SV_DispatchThreadID){
    Frame f=frame();if(any(t.xy>=f.extent.xy))return;int2 p=int2(t.xy);float2 uv=(float2(p)+0.5)/float2(f.extent.xy);
    float wire=historyTex(root.materials,uv).r;if(wire<=0.002)return;
    uint id=uint(pixel(root.material,int2(uv*float2(f.sizes.xy))).x);
    float3 colour=root.mode!=0?float3(1,0.62,0.18):id==0?float3(0.75):0.4+0.6*frac(float3(id)*float3(0.6180339,0.381966,0.7548776));
    float4 under=images[NonUniformResourceIndex(f.post.w)][p];
    images[NonUniformResourceIndex(f.post.w)][p]=float4(lerp(under.rgb,srgb(colour),saturate(wire)),1);
}
