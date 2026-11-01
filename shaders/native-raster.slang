// Native raster fill. Geometry remains local and textures remain compressed.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "look-refract.slangh"
#include "native_sample.slangh"
struct NativeVertex {float3 position;float3 normal;float2 uv;float4 tangent;float2 uv1;uint weights;uint color;};
struct NativeUv {float2 offset;float2 scale;float rotation;uint set;uint2 reserved;};
struct NativeMaterial {Material pbr;NativeUv uv[6];uint4 slots;float4 factors;uint4 layers;};
struct Instance {uint4 meta;float4x4 world;float4x4 previous;float4 nx;float4 ny;float4 nz;};
struct Varying {
    float4 clip:SV_Position;
    float3 world:TEXCOORD0;
    float3 normal:TEXCOORD1;
    float4 tangent:TEXCOORD2;
    float2 uv:TEXCOORD3;
    float2 uv1:TEXCOORD4;
    float4 weights:TEXCOORD5;
    float4 currentClip:TEXCOORD6;
    float4 previousClip:TEXCOORD7;
    nointerpolation uint material:TEXCOORD8;
    nointerpolation uint id:TEXCOORD9;
};
// Forward draws' varyings (nativeForwardVertex): the scene's, and this frame's HDR
// exposure (hdrExposure), loaded per vertex rather than per fragment. Only the
// forward pipelines carry it, so no other stage writes an output nothing reads.
struct ForwardVarying {Varying v;nointerpolation float exposure:TEXCOORD10;float4 color:TEXCOORD11;};
// Vertex-colour draws' varyings (nativePaintVertex: the gbuffer_paint and vertex_color
// PSOs, and the refractive forward PSOs): the scene's and the interpolated linear
// vertex colour. Only those carry it, so ordinary G-buffer draws pay no interpolant.
struct PaintVarying {Varying v;float4 color:TEXCOORD10;};
// Linear RGBA of a vertex colour word (instanced::Vertex::color, red in the low byte).
float4 unpackColor(uint c){return float4(c&255,(c>>8)&255,(c>>16)&255,c>>24)/255.;}
// Shadow draws' light matrix: cascades 0-3 from the frame record, 4+ the
// local-shadow tiles' matrices in the look's table (look::local::FACES), and
// VSM_PAGE_DRAWS+ this frame's virtual shadow pages (look::vsm PAGE_DRAWS).
static const uint VSM_PAGE_DRAWS=1024;
float4x4 shadowMatrix(Frame f,uint cascade){
    if(cascade<4)return frameShadow(cascade);
    if(cascade>=VSM_PAGE_DRAWS)return buffers[NonUniformResourceIndex(look(f).vsm.z)].Load<float4x4>((cascade-VSM_PAGE_DRAWS)*64);
    return buffers[NonUniformResourceIndex(look(f).localShadows.x)].Load<float4x4>(LOCAL_FACES+(cascade-4)*64);
}
// `vertex` is the index itself in indexed draws (Command::DrawIndexedProcedural,
// so the post-transform cache shades shared vertices once); non-indexed draws (the
// wire PSO, which needs each triangle's own corners) pass the corner and load it.
// `color`: the vertex's linear colour, from the instance's paint (Instance::paint:
// row meta.y bits 1.. its buffer, meta.z one plus its first word) or the geometry's;
// entries that ignore it compile its load away.
Varying nativeVertex(uint vertex,uint instance,bool shadow,bool indexed,out float4 color) {
    uint2 draw=buffers[NonUniformResourceIndex(root.flags)].Load2((root.entity+instance)*8);
    Instance i=buffers[NonUniformResourceIndex(root.material)].Load<Instance>(128+draw.x*192);
    uint4 g=buffers[NonUniformResourceIndex(root.mode)].Load4(draw.y*32);
    uint4 previous=buffers[NonUniformResourceIndex(root.mode)].Load4(draw.y*32+16);
    uint index=indexed?vertex:buffers[NonUniformResourceIndex(g.z)].Load(g.w+vertex*4);
    NativeVertex v=buffers[NonUniformResourceIndex(g.x)].Load<NativeVertex>(g.y+index*64);
    NativeVertex p=i.meta.y!=0?buffers[NonUniformResourceIndex(previous.x)].Load<NativeVertex>(previous.y+index*64):v;
    Frame f=frame();Varying o;
    float4 world=mul(i.world,float4(v.position,1));
    o.world=world.xyz;o.clip=mul(shadow?shadowMatrix(f,root.cascade):f.vp,world);
    o.normal=normalize(float3(dot(i.nx.xyz,v.normal),dot(i.ny.xyz,v.normal),dot(i.nz.xyz,v.normal)));
    float3 tangent=mul(i.world,float4(v.tangent.xyz,0)).xyz;
    tangent-=o.normal*dot(tangent,o.normal);
    o.tangent=float4(normalize(tangent),v.tangent.w*asfloat(i.meta.w));
    o.uv=v.uv;o.uv1=v.uv1;
    o.weights=float4(v.weights&255,(v.weights>>8)&255,(v.weights>>16)&255,v.weights>>24)/255.;
    float4x4 vp=buffers[NonUniformResourceIndex(root.material)].Load<float4x4>(0);
    float4x4 previousVp=buffers[NonUniformResourceIndex(root.material)].Load<float4x4>(64);
    o.currentClip=mul(vp,world);o.previousClip=mul(previousVp,mul(i.previous,float4(p.position,1)));
    color=unpackColor(i.meta.z!=0?buffers[NonUniformResourceIndex(i.meta.y>>1)].Load((i.meta.z-1+index)*4):v.color);
    o.material=i.meta.x;o.id=draw.x+1;return o;
}
// The vertex shader of entries that read no vertex colour.
Varying nativeVertex(uint vertex,uint instance,bool shadow,bool indexed){float4 unused;return nativeVertex(vertex,instance,shadow,indexed,unused);}
[shader("vertex")]
Varying nativeGeometryVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){return nativeVertex(vertex,instance,false,true);}
// The forward pipelines' vertices also carry the exposure their fragments apply and
// the vertex colour (forward surfaces always read it: blended draws are few).
[shader("vertex")]
ForwardVarying nativeForwardVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){
    ForwardVarying o;o.v=nativeVertex(vertex,instance,false,true,o.color);Frame f=frame();o.exposure=hdrExposure(f,look(f));return o;
}
[shader("vertex")]
PaintVarying nativePaintVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){PaintVarying o;o.v=nativeVertex(vertex,instance,false,true,o.color);return o;}
// Wireframe view modes (Renderer `Wire.Draw`): the camera's draws again, with the same
// clip position as the G-buffer (so the depth test against it passes on visible
// surfaces). Draws are procedural over each triangle's corners in order, so a vertex's
// corner, and its barycentric, is its index modulo three.
struct WireVarying {float4 clip:SV_Position;float3 bary:TEXCOORD0;};
[shader("vertex")]
WireVarying nativeWireVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){
    Varying v=nativeVertex(vertex,instance,false,false);WireVarying o;o.clip=v.clip;
    uint corner=vertex%3;o.bary=float3(corner==0,corner==1,corner==2);return o;
}
// Edge coverage of a one-pixel line: each triangle covers its half-pixel side of a
// shared edge, falling off linearly to zero one pixel away (one pixel wide in all).
[shader("fragment")]
float4 nativeWireFragment(WireVarying v):SV_Target0{
    float3 pixels=v.bary/max(fwidth(v.bary),1e-6);
    return float4(saturate(1-min(pixels.x,min(pixels.y,pixels.z))),0,0,0);
}
// Editor selection outlines (Renderer `Outline.Mask`, renderer/outline.rs): the selected
// instances again with the G-buffer's clip position, each writing its outline group
// (root word 4): once without a depth test, so the whole silhouette counts, then
// against the frame's depth, adding 128 where that surface is in view.
[shader("vertex")]
float4 nativeOutlineVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID):SV_Position{return nativeVertex(vertex,instance,false,true).clip;}
[shader("fragment")]
float4 nativeOutlineFragment():SV_Target0{return float4(float(root.cascade),0,0,0);}
struct ShadowVarying {float4 clip:SV_Position;float2 uv:TEXCOORD0;float2 uv1:TEXCOORD1;nointerpolation uint material:TEXCOORD2;};
// Translucent sun casters (native_shadow_translucent) also carry their world position,
// normal and instance: the optical depth they add depends on facing and depth.
struct TranslucentVarying {float4 clip:SV_Position;float3 world:TEXCOORD0;float3 normal:TEXCOORD1;float2 uv:TEXCOORD2;float2 uv1:TEXCOORD3;nointerpolation uint material:TEXCOORD4;nointerpolation uint id:TEXCOORD5;};
[shader("vertex")]
TranslucentVarying nativeShadowVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){
    Varying v=nativeVertex(vertex,instance,true,true);TranslucentVarying s;s.clip=v.clip;s.world=v.world;s.normal=v.normal;
    s.uv=v.uv;s.uv1=v.uv1;s.material=v.material;s.id=v.id;return s;
}
// Opaque and masked casters also carry their instance for the facing test.
struct ShadowCasterVarying {float4 clip:SV_Position;float2 uv:TEXCOORD0;float2 uv1:TEXCOORD1;nointerpolation uint material:TEXCOORD2;nointerpolation uint id:TEXCOORD3;};
[shader("vertex")]
ShadowCasterVarying nativeShadowCasterVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){Varying v=nativeVertex(vertex,instance,true,true);ShadowCasterVarying s;s.clip=v.clip;s.uv=v.uv;s.uv1=v.uv1;s.material=v.material;s.id=v.id;return s;}
// Masked casters whose alpha reads vertex colours (the shadow_paint PSO).
struct ShadowPaintVarying {ShadowCasterVarying s;float4 color:TEXCOORD4;};
[shader("vertex")]
ShadowPaintVarying nativeShadowPaintVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){
    ShadowPaintVarying o;Varying v=nativeVertex(vertex,instance,true,true,o.color);
    o.s.clip=v.clip;o.s.uv=v.uv;o.s.uv1=v.uv1;o.s.material=v.material;o.s.id=v.id;return o;
}
NativeMaterial material(uint id){return buffers[NonUniformResourceIndex(root.materials)].Load<NativeMaterial>(id*320);}
float2 coordinates(NativeUv transform,Varying v){float2 p=(transform.set==0?v.uv:v.uv1)*transform.scale;float s,c;sincos(transform.rotation,s,c);return transform.offset+float2(c*p.x-s*p.y,s*p.x+c*p.y);}
// Texture footprint scale of the pass (Frame.upscale.z, root word 4): surface()
// sets it, so upscaled G-buffer and forward fragments choose the display
// resolution's mips (a negative mip bias the temporal resolve reconstructs);
// shadow passes keep 1.
static float footprint=1;
// Resident map `id` (not 0xffffffff) at `uv` with explicit UV gradients: the hardware picks
// the mip and, on the sampler's anisotropy (samplers[0], up to 16x), takes extra taps along
// the footprint's long axis, so grazing surfaces stay sharp where a single isotropic LOD
// blurs. The view holds only the resident levels, so the gradient LOD is already relative
// to the first resident mip (native_sample.slangh ntSampleLevel subtracts info.y for the same
// reason) and clamps to the tail. native_sample.slangh is a byte-identical contract copy
// (ara-texture-stream, ara-render-fidelity), so grad sampling lives here.
float4 ntSampleGrad(uint table,uint id,float2 uv,float2 dx,float2 dy){
    uint image=buffers[NonUniformResourceIndex(table)].Load(id*32);
    return textures[NonUniformResourceIndex(image)].SampleGrad(samplers[0],uv,dx,dy);
}
// A sampled normal texel as ntNormal (native_sample.slangh) decodes it: BC5 stores XY,
// Z is reconstructed before glTF normalScale.
float3 ntNormalTexel(uint table,uint id,float3 texel,float scale){
    float3 n=texel*2-1;
    uint encoding=buffers[NonUniformResourceIndex(table)].Load(id*32+16);
    if(encoding==2)n.z=sqrt(max(0,1-dot(n.xy,n.xy)));
    n.xy*=scale;
    return dot(n,n)>1e-12?normalize(n):float3(0,0,1);
}
// Map `slot` of `m` at the fragment, anisotropically filtered over the pixel's UV
// footprint scaled by `footprint` (gradients scale uniformly, so the mip shifts by
// log2(footprint) and the anisotropy ratio is unchanged).
float4 sampleMap(NativeMaterial m,uint slot,Varying v){
    uint id=slot<4?m.pbr.textures[min(slot,3)]:(slot==4?m.slots.x:m.slots.y);
    if(id==0xffffffff)return float4(slot==1?float3(0,0,1):float3(1),1);
    float2 uv=coordinates(m.uv[slot],v);
    float4 texel=ntSampleGrad(root.sample,id,uv,ddx(uv)*footprint,ddy(uv)*footprint);
    if(slot==1)return float4(ntNormalTexel(root.sample,id,texel.xyz,m.pbr.surface.y),1);
    bool decode=slot==0?(m.pbr.flags.y&(1u<<25))!=0:slot==3 && (m.pbr.flags.y&(1u<<26))!=0;
    if(decode)texel.rgb=ntDecodeColor(texel.rgb);
    return texel;
}
// Unit `v`, or `fallback` for zero-length or non-finite input (degenerate skinned or
// UV-less tangent frames would otherwise turn shading into NaN/inf fireflies).
float3 safeNormalize(float3 v,float3 fallback){float l=dot(v,v);return l>1e-12 && l<1e30?v*rsqrt(l):fallback;}
// A masked material's alpha for its cutout test: the base map at the level of its
// footprint's minor axis, one isotropic sample. Anisotropic filtering averages
// along the major axis, so at a grazing view a grate's holes (a ceiling grating seen
// down a corridor) average shut, where the path tracer, sampling one point, sees
// through them. Colour keeps the filtered sample.
float cutoutAlpha(NativeMaterial m,Varying v){
    uint id=m.pbr.textures[0];if(id==0xffffffff)return m.pbr.color.a;
    float2 uv=coordinates(m.uv[0],v),dx=ddx(uv)*footprint,dy=ddy(uv)*footprint,minor=dot(dx,dx)<dot(dy,dy)?dx:dy;
    return ntSampleGrad(root.sample,id,uv,minor,minor).a*m.pbr.color.a;
}
Surface surfaceMaterial(NativeMaterial m,Varying v){
    Surface s;float4 base=sampleMap(m,0,v)*m.pbr.color;
    s.albedo=max(0,base.rgb);s.alpha=base.a;
    float3 normal=safeNormalize(v.normal,float3(0,1,0)),t=v.tangent.xyz-normal*dot(v.tangent.xyz,normal);float handedness=v.tangent.w;
    if(m.slots.w!=0){
        float2 uv=coordinates(m.uv[1],v),a=ddx(uv),b=ddy(uv);float determinant=a.x*b.y-a.y*b.x;
        if(abs(determinant)>1e-10){float3 e1=ddx(v.world),e2=ddy(v.world);t=(e1*b.y-e2*a.y)/determinant;t-=normal*dot(t,normal);float3 bitangent=(e2*a.x-e1*b.x)/determinant;handedness=dot(cross(normal,t),bitangent)<0?-1:1;}
    }
    t=safeNormalize(t,normalize(cross(normal,abs(normal.y)<0.99?float3(0,1,0):float3(1,0,0))));float3 n=sampleMap(m,1,v).xyz;
    // A degenerate tangent frame (zero handedness) can cancel the mapped normal.
    s.normal=safeNormalize(t*n.x+cross(normal,t)*handedness*n.y+normal*n.z,normal);
    float4 mr=sampleMap(m,2,v);s.roughness=clamp(m.pbr.emissiveRoughness.w*mr.g,0.045,1);s.metal=saturate(m.pbr.surface.x*mr.b);
    s.emissive=max(0,m.pbr.emissiveRoughness.rgb*sampleMap(m,3,v).rgb);
    // Transmission is the dielectric layer's (glTF KHR_materials_transmission): metal
    // reflects whatever its transmission factor says.
    s.ao=lerp(1,sampleMap(m,4,v).r,m.factors.x);s.transmission=saturate(m.factors.y*sampleMap(m,5,v).r)*(1-s.metal);return s;
}
// Material flags.y bits of the vertex-colour paths (ara_render::scene).
static const uint VERTEX_COLOR_TINT=1u<<29,VERTEX_PAINT_LAYERS=1u<<30;
// Vertex-paint layer `i`'s weight at `color` (scene::PaintWeight): flags.z holds a byte
// per layer, two bits per channel R, G, B, A: 1 scales by the channel, 2 by one minus it.
float paintWeight(NativeMaterial m,uint i,float4 color){
    uint byte=(m.pbr.flags.z>>(8*i))&255;float w=1;
    for(uint c=0;c<4;c++){uint term=(byte>>(2*c))&3;float x=component(color,c);w*=term==1?x:term==2?1-x:1;}
    return w;
}
// The layer weights of a paint-layered material (scene::VERTEX_PAINT_LAYERS), past the
// layer count zero; a flattened lerp tree sums to one, else they normalize.
float4 paintWeights(NativeMaterial m,float4 color){
    uint layers=m.slots.z;float4 w=float4(layers>0?paintWeight(m,0,color):0,layers>1?paintWeight(m,1,color):0,layers>2?paintWeight(m,2,color):0,layers>3?paintWeight(m,3,color):0);
    float sum=dot(w,float4(1));return sum>1e-6?w/sum:float4(1,0,0,0);
}
// A vertex-paint layered surface (Unreal's VertexColor lerps between material layers):
// each layer is a whole material, its surface blended by its paint weight. Normals blend
// after each layer's own tangent mapping and renormalize; a layer absent from a whole
// pixel quad is skipped (its samples' derivatives stay valid).
Surface paintSurface(NativeMaterial m,Varying v,float4 color){
    float4 weights=paintWeights(m,color);Surface s=(Surface)0;float3 normal=0;
    for(uint i=0;i<m.slots.z;i++){
        float w=component(weights,i);
        [branch]if(max(max(w,QuadReadAcrossX(w)),max(QuadReadAcrossY(w),QuadReadAcrossDiagonal(w)))<=0)continue;
        Surface l=surfaceMaterial(material(component(m.layers,i)),v);
        s.albedo+=l.albedo*w;s.alpha+=l.alpha*w;normal+=l.normal*w;s.roughness+=l.roughness*w;
        s.metal+=l.metal*w;s.emissive+=l.emissive*w;s.ao+=l.ao*w;s.transmission+=l.transmission*w;
    }
    s.normal=safeNormalize(normal,safeNormalize(v.normal,float3(0,1,0)));return s;
}
// The opacity a vertex-colour draw's alpha test sees: the paint layers' blended base
// alpha (or the material's), times the vertex alpha under a tint.
float paintAlpha(NativeMaterial m,Varying v,float4 color){
    float alpha;
    if((m.pbr.flags.y&VERTEX_PAINT_LAYERS)!=0){
        float4 weights=paintWeights(m,color);alpha=0;
        for(uint i=0;i<m.slots.z;i++){NativeMaterial l=material(component(m.layers,i));alpha+=component(weights,i)*sampleMap(l,0,v).a*l.pbr.color.a;}
    }else alpha=sampleMap(m,0,v).a*m.pbr.color.a;
    return (m.pbr.flags.y&VERTEX_COLOR_TINT)!=0?alpha*color.a:alpha;
}
// Terrain projection weights from the surface normal: the top projection (world
// X/Z, the layers' UVs) alone on gentle slopes; past about 26 degrees the world Z/Y
// and X/Y side projections fade in.
float3 terrainProjection(float3 n){
    float3 w=pow(abs(n),4);w/=dot(w,1);
    w.xz*=saturate((w.x+w.z-0.05)*10);w.y=1-w.x-w.z;
    return w;
}
// A terrain layer map through the projections `w`. `sides` (uniform per pixel quad,
// so texture derivatives stay valid) adds the side samples: maps keep their scale
// on steep slopes instead of stretching into streaks down them.
float4 terrainMap(NativeMaterial m,uint slot,Varying v,float3 w,bool sides){
    float4 c=sampleMap(m,slot,v);
    [branch]if(sides){
        Varying x=v;x.uv=x.uv1=v.world.zy;Varying z=v;z.uv=z.uv1=v.world.xy;
        c=c*w.y+sampleMap(m,slot,x)*w.x+sampleMap(m,slot,z)*w.z;
    }
    return c;
}
// A layer's base colour map at `v`, flattened: scaled by the map's mean luminance
// over its tile-scale mean there (`tile`, its 8x8 mip), so a tile's own low
// frequencies (shading baked across it) stop repeating as a grid.
float4 terrainFlat(NativeMaterial m,Varying v,float mean,float tile,bool decode){
    float4 c=sampleMap(m,0,v);
    float local=luminance(ntSampleColorLevel(root.sample,m.pbr.textures[0],coordinates(m.uv[0],v),tile,decode).rgb);
    c.rgb*=clamp(mean/max(local,1e-4),0.5,2);
    return c;
}
// A layer's base colour through the projections (as terrainMap), each flattened.
float4 terrainColor(NativeMaterial m,Varying v,float3 w,bool sides){
    uint id=m.pbr.textures[0];
    if(id==0xffffffff)return terrainMap(m,0,v,w,sides);
    uint4 info=ntInfo(root.sample,id);float tile=max(0,log2(float(max(info.z,info.w)))-3);
    bool decode=(m.pbr.flags.y&(1u<<25))!=0;
    float mean=luminance(ntSampleColorLevel(root.sample,id,float2(0.5,0.5),64.,decode).rgb);
    float4 c=terrainFlat(m,v,mean,tile,decode);
    [branch]if(sides){
        Varying x=v;x.uv=x.uv1=v.world.zy;Varying z=v;z.uv=z.uv1=v.world.xy;
        c=c*w.y+terrainFlat(m,x,mean,tile,decode)*w.x+terrainFlat(m,z,mean,tile,decode)*w.z;
    }
    return c;
}
// Hashed value noise over integer lattice `p` in [0, 1).
float terrainHash(int2 p){
    uint h=uint(p.x)*0x8da6b343u^uint(p.y)*0xd8163841u;h=(h^(h>>16))*0x7feb352du;h^=h>>15;
    return float(h&0xffffffu)/16777216.;
}
float terrainNoise(float2 p){
    float2 i=floor(p),t=p-i;t=t*t*(3-2*t);int2 c=int2(i);
    return lerp(lerp(terrainHash(c),terrainHash(c+int2(1,0)),t.x),lerp(terrainHash(c+int2(0,1)),terrainHash(c+int2(1,1)),t.x),t.y);
}
// Tile breakup: a smooth world-space brightness variation (two noise octaves, 31 m
// and 11 m), so open ground does not repeat once tiles are flattened.
float terrainBreakup(float3 world){
    return lerp(0.8,1.2,terrainNoise(world.xz/31.)*0.65+terrainNoise(world.xz/11.+17.)*0.35);
}
// A terrain splat: its layers blend colour, metal/roughness and tangent-space
// normals in one tangent frame (the splat's), transformed once; a layer absent
// from a whole pixel quad is skipped. With SIDES, layer maps project from the side
// too on steep slopes (terrainMap; normal detail fades there); without, they keep
// the top projection. Colour tiles are flattened (terrainColor) under a world-space
// breakup (terrainBreakup), at every distance. Beyond the look's terrain detail
// distance (blended over a quarter more, decided per quad so derivatives stay
// valid) layers keep colour maps and factors only.
Surface terrainSurface<let SIDES:int>(NativeMaterial m,Varying v){
    // Layers past the count weigh nothing (a mask, not a runtime index: see component).
    uint layers=m.slots.z;float4 weights=v.weights*float4(layers>0,layers>1,layers>2,layers>3);
    float sum=dot(weights,float4(1));weights=sum>1e-8?weights/sum:float4(1,0,0,0);
    // The layers share the splat geometry; the frame follows the first layer's
    // normal-map coordinates (layers differ in tiling scale only).
    NativeMaterial first=material(m.layers[0]);
    float3 normal=safeNormalize(v.normal,float3(0,1,0)),t=v.tangent.xyz-normal*dot(v.tangent.xyz,normal);float handedness=v.tangent.w;
    if(first.slots.w!=0){
        float2 uv=coordinates(first.uv[1],v),a=ddx(uv),b=ddy(uv);float determinant=a.x*b.y-a.y*b.x;
        if(abs(determinant)>1e-10){float3 e1=ddx(v.world),e2=ddy(v.world);t=(e1*b.y-e2*a.y)/determinant;t-=normal*dot(t,normal);float3 bitangent=(e2*a.x-e1*b.x)/determinant;handedness=dot(cross(normal,t),bitangent)<0?-1:1;}
    }
    t=safeNormalize(t,normalize(cross(normal,abs(normal.y)<0.99?float3(0,1,0):float3(1,0,0))));
    Frame f=frame();float range=look(f).terrain.x;
    float detail=range>0?1-saturate((length(v.world-f.camera.xyz)-range)/(0.25*range)):0;
    float quad=max(max(detail,QuadReadAcrossX(detail)),max(QuadReadAcrossY(detail),QuadReadAcrossDiagonal(detail)));
    Surface s=(Surface)0;float3 tangentNormal=0;
    float3 projection=terrainProjection(normal);float side=projection.x+projection.z;
    bool sides=SIDES!=0 && max(max(side,QuadReadAcrossX(side)),max(QuadReadAcrossY(side),QuadReadAcrossDiagonal(side)))>0;
    for(uint i=0;i<m.slots.z;i++){
        float w=component(weights,i);
        [branch]if(max(max(w,QuadReadAcrossX(w)),max(QuadReadAcrossY(w),QuadReadAcrossDiagonal(w)))<=0)continue;
        NativeMaterial layer=material(component(m.layers,i));
        float4 base=terrainColor(layer,v,projection,sides)*layer.pbr.color;
        // Far away, the metal/roughness map's coarsest mip (its mean) scales the
        // factors: glTF factors are often one, with the map holding the values.
        float4 mr=ntSampleLevel(root.sample,layer.pbr.textures[2],float2(0.5,0.5),64.);float3 n=float3(0,0,1);
        if(quad>0){
            mr=lerp(mr,terrainMap(layer,2,v,projection,sides),detail);
            n=lerp(float3(0,0,1),sampleMap(layer,1,v).xyz,detail*projection.y);
        }
        float roughness=layer.pbr.emissiveRoughness.w*mr.g,metal=layer.pbr.surface.x*mr.b;
        s.albedo+=max(0,base.rgb)*w;s.alpha+=base.a*w;tangentNormal+=n*w;
        s.roughness+=clamp(roughness,0.045,1)*w;s.metal+=saturate(metal)*w;
        s.emissive+=max(0,layer.pbr.emissiveRoughness.rgb*terrainMap(layer,3,v,projection,sides).rgb)*w;
        s.ao+=lerp(1,terrainMap(layer,4,v,projection,sides).r,layer.factors.x)*w;
    }
    s.albedo*=terrainBreakup(v.world);
    s.normal=safeNormalize(t*tangentNormal.x+cross(normal,t)*handedness*tangentNormal.y+normal*tangentNormal.z,normal);
    return s;
}
// A native fragment's surface. SIDES and PAINT are compile-time choices: code in the
// G-buffer shader costs every pixel registers even where nothing uses it. Presets
// without terrain detail bind a PSO without the side projections
// (nativeGeometryTopFragment); only vertex-colour draws bind PAINT (`color`: their
// interpolated vertex colour): paint layers and the vertex-colour tint, never terrain.
Surface surface<let SIDES:int,let PAINT:int>(Varying v,float4 color){
    footprint=root.cascade==0?1:asfloat(root.cascade);NativeMaterial m=material(v.material);
    // Guessed loading bounds must not cover the view or form stair-like bands
    // across pending foliage. Keep only short one-pixel corner brackets; tiny
    // distant bounds disappear. Failed/missing bounds remain opaque magenta.
    if((m.pbr.flags.y&(1u<<27))!=0 && m.pbr.flags.x==2){
        float2 step=max(fwidth(v.uv),float2(1e-6));
        float2 edge=min(v.uv,1-v.uv)/step;
        if(min(edge.x,edge.y)>1 || max(edge.x,edge.y)>8 || max(step.x,step.y)>0.125)discard;
    }
    Surface s;
    if(PAINT!=0){
        s=(m.pbr.flags.y&VERTEX_PAINT_LAYERS)!=0?paintSurface(m,v,color):surfaceMaterial(m,v);
        if((m.pbr.flags.y&VERTEX_COLOR_TINT)!=0){s.albedo*=color.rgb;s.alpha*=color.a;}
    }else s=m.slots.z>0?terrainSurface<SIDES>(m,v):surfaceMaterial(m,v);
    if(m.pbr.flags.x==1 && (PAINT==0 && m.slots.z==0?cutoutAlpha(m,v):s.alpha)<m.pbr.surface.w)discard;
    float3 du=ddx(s.normal),dv=ddy(s.normal);float kernel=min(2*0.15915494*(dot(du,du)+dot(dv,dv)),0.18);
    s.roughness=sqrt(sqrt(saturate(pow(s.roughness,4)+kernel)));return s;
}
float2 motion(Varying v){float2 now=v.currentClip.xy/v.currentClip.w,previous=v.previousClip.xy/v.previousClip.w;return (now-previous)*float2(0.5,-0.5);}
struct GBuffer {float4 albedo:SV_Target0;float4 normal:SV_Target1;float4 material:SV_Target2;float4 emissive:SV_Target3;float id:SV_Target4;float2 velocity:SV_Target5;};
// glTF doubleSided (ara_render::scene::DOUBLE_SIDED in material flags.y).
static const uint DOUBLE_SIDED=1u<<28;
// Whether a G-buffer fragment is a double-sided material's back face, lit on its own
// side (glTF: back faces reverse their normals before lighting). The rasterizer's
// winding decides (SV_IsFrontFace: counter-clockwise front, as -fvk-invert-y keeps
// glTF's), corrected for mirrored instances whose transform reverses it (the instance
// row's determinant sign, meta.w); the interpolated normal must agree it faces away,
// so a front face is never flipped and a silhouette never misjudged.
bool backFace(Varying v,bool front){
    if((material(v.material).pbr.flags.y&DOUBLE_SIDED)==0)return false;
    bool mirrored=asfloat(buffers[NonUniformResourceIndex(root.material)].Load(128+(v.id-1)*192+12))<0;
    return front==mirrored && dot(v.normal,frame().camera.xyz-v.world)<0;
}
GBuffer geometry<let SIDES:int,let PAINT:int>(Varying v,float4 color,bool front){Surface s=surface<SIDES,PAINT>(v,color);if(backFace(v,front))s.normal=-s.normal;GBuffer g;g.albedo=float4(s.albedo,s.ao);g.normal=gbufferEncodeNormal(s.normal,1);g.material=gbufferEncodeMaterial(s.roughness,s.metal,material(v.material).pbr.flags.x==1?0:1,1);g.emissive=gbufferEncodeEmissive(s.emissive,1);g.id=float(v.id);g.velocity=motion(v);return g;}
[shader("fragment")]
GBuffer nativeGeometryFragment(Varying v,bool front:SV_IsFrontFace){return geometry<1,0>(v,1,front);}
// The G-buffer for presets without terrain detail: terrain keeps its top projection.
[shader("fragment")]
GBuffer nativeGeometryTopFragment(Varying v,bool front:SV_IsFrontFace){return geometry<0,0>(v,1,front);}
// The G-buffer of draws that read vertex colours (NativePipelines::gbuffer_paint).
[shader("fragment")]
GBuffer nativeGeometryPaintFragment(PaintVarying p,bool front:SV_IsFrontFace){return geometry<0,1>(p.v,p.color,front);}
// The vertex-colour views (ViewMode::VertexColor*, Frame.temporal.z 11-15: RGB, then R,
// G, B, A alone as grey): the colour as unlit albedo, through the sRGB albedo target
// decoded so the view shows the stored value (0.5 mid-grey). Masked cutouts stay holes.
[shader("fragment")]
GBuffer nativeVertexColorFragment(PaintVarying p,bool front:SV_IsFrontFace){
    Varying v=p.v;footprint=root.cascade==0?1:asfloat(root.cascade);NativeMaterial m=material(v.material);
    if(m.pbr.flags.x==1 && paintAlpha(m,v,p.color)<m.pbr.surface.w)discard;
    uint channel=frame().temporal.z-11;float3 c=saturate(channel==0?p.color.rgb:float3(component(p.color,channel-1)));
    float3 n=safeNormalize(v.normal,float3(0,1,0));if(backFace(v,front))n=-n;
    GBuffer g;g.albedo=float4(ntDecodeColor(c),1);g.normal=gbufferEncodeNormal(n,1);g.material=gbufferEncodeMaterial(1,0,1,1);
    g.emissive=gbufferEncodeEmissive(0,1);g.id=float(v.id);g.velocity=motion(v);return g;
}
// Shadow casters, as Unreal's shadow depth pass culls them (SetupShadowCullMode): a
// single-sided material casts only from the faces turned towards the light, a
// double-sided one from both. Casting every back face let open shells (terrain sheets,
// one-sided walls) block light arriving from behind them. The winding test is
// backFace's: counter-clockwise front, reversed for mirrored instances.
// PAINT (the shadow_paint PSO) alpha-tests through the vertex colour (paintAlpha).
void shadowCaster<let PAINT:int>(ShadowCasterVarying shadow,bool front,float4 color){
    Varying v=(Varying)0;v.uv=shadow.uv;v.uv1=shadow.uv1;NativeMaterial m=material(shadow.material);
    if((m.pbr.flags.y&DOUBLE_SIDED)==0){
        bool mirrored=asfloat(buffers[NonUniformResourceIndex(root.material)].Load(128+(shadow.id-1)*192+12))<0;
        if(front==mirrored)discard;
    }
    if(m.pbr.flags.x==1 && (PAINT!=0?paintAlpha(m,v,color):sampleMap(m,0,v).a*m.pbr.color.a)<m.pbr.surface.w)discard;
}
[shader("fragment")]
void nativeShadowFragment(ShadowCasterVarying shadow,bool front:SV_IsFrontFace){shadowCaster<0>(shadow,front,1);}
[shader("fragment")]
void nativeShadowPaintFragment(ShadowPaintVarying p,bool front:SV_IsFrontFace){shadowCaster<1>(p.s,front,p.color);}
// The overdraw view (ViewMode::Overdraw, Renderer `Overdraw.Count`): the camera's
// draws again without the depth test, each fragment adding one to the count target
// through premultiplied blending (alpha 0). Masked materials discard by the casters'
// alpha test, so only fragments the raster keeps count.
[shader("vertex")]
ShadowVarying nativeOverdrawVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){Varying v=nativeVertex(vertex,instance,false,true);ShadowVarying s;s.clip=v.clip;s.uv=v.uv;s.uv1=v.uv1;s.material=v.material;return s;}
[shader("fragment")]
float4 nativeOverdrawFragment(ShadowVarying shadow):SV_Target0{Varying v=(Varying)0;v.uv=shadow.uv;v.uv1=shadow.uv1;NativeMaterial m=material(shadow.material);if(m.pbr.flags.x==1 && sampleMap(m,0,v).a*m.pbr.color.a<m.pbr.surface.w)discard;return float4(1,0,0,0);}
// Translucent sun casters (look-translucent.slangh): the optical depth (-ln of the
// transmittance) each face adds along the sun's rays, blended additively (zero alpha
// under ONE, ONE_MINUS_SRC_ALPHA) over a map cleared to zero, so the map holds the sum
// over every film and glass face: their transmittances' product. A face passes
// glassTransmittance (blended films their coverage's complement; transmissive
// surfaces their tint less Fresnel). A volume's medium absorbs along the chord
// between where the sun's ray enters and leaves it: faces towards the sun subtract
// their absorption times their depth along the rays, faces away add it, so each
// enter/leave pair adds the absorption over the depth between them, as the path
// tracer's shadow rays take it. Depths are measured from the instance's origin.
[shader("fragment")]
float4 nativeShadowTranslucentFragment(TranslucentVarying t):SV_Target0{
    Varying v=(Varying)0;v.uv=t.uv;v.uv1=t.uv1;NativeMaterial m=material(t.material);
    float4 base=sampleMap(m,0,v)*m.pbr.color;float coverage=m.pbr.flags.x==2?saturate(base.a):1;
    float metal=saturate(m.pbr.surface.x*sampleMap(m,2,v).b),transmission=saturate(m.factors.y*sampleMap(m,5,v).r)*(1-metal);
    RefractVolume volume=refractVolume(uint2(asuint(m.factors.z),asuint(m.factors.w)));
    float3 sun=frame().sunDirection.xyz,n=safeNormalize(t.normal,sun);
    float3 depth=-log(max(glassTransmittance(coverage,transmission,max(base.rgb,0),volume.ior,dot(n,sun)),1e-6));
    if(volume.thickness>0 && transmission>0){
        float4x4 world=buffers[NonUniformResourceIndex(root.material)].Load<float4x4>(128+(t.id-1)*192+16);
        float z=dot(t.world-float3(world[0].w,world[1].w,world[2].w),-sun);
        depth+=volume.absorption*coverage*transmission*(dot(n,sun)>0?-z:z);
    }
    return float4(depth,0);
}
struct Forward {float4 color:SV_Target0;float id:SV_Target1;float4 albedo:SV_Target2;float4 normal:SV_Target3;float4 material:SV_Target4;float4 emissive:SV_Target5;float4 velocity:SV_Target6;};
// Whether a forward fragment faces the viewer, by its interpolated normal rather than
// the rasterizer's winding: the back- and front-face passes classify every fragment
// alike, so each draws exactly once.
bool facesViewer(Varying v){return dot(v.normal,frame().camera.xyz-v.world)>=0;}
// A forward fragment's surface. Double-sided transparency lights each side as seen
// (glTF: back faces reverse their normals before lighting), as a path tracer does;
// lit from outside, a film's far side caught sunlight its near side never sees.
// FACES is a compile-time pass choice: 0 both sides, 1 back faces only,
// 2 front faces only. Lit frames draw a double-sided blended volume's back faces
// before its front faces (NativePipelines::forward_lit_back/front), so a closed film
// composites far under near side instead of in triangle order (blocks along the
// tessellation); every other blended draw takes one pass over both sides.
// The sun is a disk, not a point: forward surfaces (often thin, glossy and lit at
// grazing angles) widen their lobe to at least its angular radius (GGX alpha), so a
// mirror-like rim shows the sun's area-filtered highlight rather than a sub-pixel peak.
// `color`: the vertex colour (blended surfaces tint and blend paint layers by it).
Surface forwardSurface<let FACES:int>(Varying v,float4 color){
    bool front=facesViewer(v);if((FACES==1 && front)||(FACES==2 && !front))discard;
    Surface s=surface<0,1>(v,color);if(!front)s.normal=-s.normal;
    s.roughness=max(s.roughness,sqrt(max(look(frame()).sunLux.w,0)));return s;
}
// The forward pass; `high` selects the High/Epic tier's code at compile time.
Forward nativeForward(ForwardVarying i,bool high){Varying v=i.v;Surface s=forwardSurface<0>(v,i.color);Frame f=frame();Look l=look(f);Forward o;o.color=forwardColour(f,l,s,v.world,int2(v.clip.xy),i.exposure,high);o.id=float(v.id);o.albedo=float4(s.albedo,s.alpha);o.normal=gbufferEncodeNormal(s.normal,s.alpha);o.material=gbufferEncodeMaterial(s.roughness,s.metal,1,s.alpha);o.emissive=gbufferEncodeEmissive(s.emissive,s.alpha);o.velocity=float4(motion(v),0,1);return o;}
[shader("fragment")]
Forward nativeForwardFragment(ForwardVarying i){return nativeForward(i,false);}
// Lit frames' forward pass: nothing reads the G-buffer after it (only the debug
// views do, through nativeForwardFragment), so it writes colour, id and motion only;
// `high` selects the High/Epic tier's code at compile time.
struct ForwardLit {float4 color:SV_Target0;float id:SV_Target1;float4 velocity:SV_Target2;};
ForwardLit nativeForwardLit<let FACES:int>(ForwardVarying i,bool high){Varying v=i.v;Surface s=forwardSurface<FACES>(v,i.color);Frame f=frame();Look l=look(f);ForwardLit o;o.color=forwardColour(f,l,s,v.world,int2(v.clip.xy),i.exposure,high);o.id=float(v.id);o.velocity=float4(motion(v),0,1);return o;}
[shader("fragment")]
ForwardLit nativeForwardLitFragment(ForwardVarying i){return nativeForwardLit<0>(i,false);}
[shader("fragment")]
ForwardLit nativeForwardLitFrontFragment(ForwardVarying i){return nativeForwardLit<2>(i,false);}
[shader("fragment")]
ForwardLit nativeForwardLitBackFragment(ForwardVarying i){return nativeForwardLit<1>(i,false);}
[shader("fragment")]
ForwardLit nativeForwardHighFragment(ForwardVarying i){return nativeForwardLit<0>(i,true);}
[shader("fragment")]
ForwardLit nativeForwardHighFrontFragment(ForwardVarying i){return nativeForwardLit<2>(i,true);}
[shader("fragment")]
ForwardLit nativeForwardHighBackFragment(ForwardVarying i){return nativeForwardLit<1>(i,true);}
// Transmissive forward draws (NativePipelines::refract): the surface composites the
// opaque scene it refracts (look-refract.slangh) instead of blending a film. A
// volume's back faces are its far side, which its front faces already refract
// through; a thin wall shades both sides, facing the viewer.
ForwardLit nativeRefract(PaintVarying i,bool high){
    Varying v=i.v;
    NativeMaterial m=material(v.material);uint2 volume=uint2(asuint(m.factors.z),asuint(m.factors.w));
    bool front=facesViewer(v);if(!front && (volume.x&0xffff)!=0)discard;
    Surface s=forwardSurface<0>(v,i.color);float3 geometric=safeNormalize(v.normal,float3(0,1,0))*(front?1:-1);
    // The instance's scale (the cube root of its determinant) for the mesh-space thickness.
    float4x4 world=buffers[NonUniformResourceIndex(root.material)].Load<float4x4>(128+(v.id-1)*192+16);
    float scale=pow(abs(determinant((float3x3)world)),1./3.);
    Frame f=frame();Look l=look(f);ForwardLit o;o.color=refractColour(f,l,s,volume,scale,v.world,geometric,int2(v.clip.xy),high);
    o.id=float(v.id);o.velocity=float4(motion(v),0,1);return o;
}
[shader("fragment")]
ForwardLit nativeForwardRefractFragment(PaintVarying i){return nativeRefract(i,false);}
[shader("fragment")]
ForwardLit nativeForwardRefractHighFragment(PaintVarying i){return nativeRefract(i,true);}
