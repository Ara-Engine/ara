// Native raster fill. Geometry remains local and textures remain compressed.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "look-refract.slangh"
#include "native_sample.slangh"
struct NativeVertex {float3 position;float3 normal;float2 uv;float4 tangent;float2 uv1;uint weights;uint reserved;};
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
// Shadow draws' light matrix: cascades 0-3 from the frame record, 4+ the
// local-shadow tiles' matrices in the look's table (look::local::FACES), and
// VSM_PAGE_DRAWS+ this frame's virtual shadow pages (look::vsm PAGE_DRAWS).
static const uint VSM_PAGE_DRAWS=1024;
float4x4 shadowMatrix(Frame f,uint cascade){
    if(cascade<4)return f.shadows[cascade];
    if(cascade>=VSM_PAGE_DRAWS)return buffers[NonUniformResourceIndex(look(f).vsm.z)].Load<float4x4>((cascade-VSM_PAGE_DRAWS)*64);
    return buffers[NonUniformResourceIndex(look(f).localShadows.x)].Load<float4x4>(LOCAL_FACES+(cascade-4)*64);
}
// `vertex` is the index itself in indexed draws (Command::DrawIndexedProcedural,
// so the post-transform cache shades shared vertices once); non-indexed draws (the
// wire PSO, which needs each triangle's own corners) pass the corner and load it.
Varying nativeVertex(uint vertex,uint instance,bool shadow,bool indexed) {
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
    o.material=i.meta.x;o.id=draw.x+1;return o;
}
[shader("vertex")]
Varying nativeGeometryVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){return nativeVertex(vertex,instance,false,true);}
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
// Edge coverage: the distance to the nearest edge in pixels, antialiased over 1.5.
[shader("fragment")]
float4 nativeWireFragment(WireVarying v):SV_Target0{
    float3 edge=smoothstep(float3(0),fwidth(v.bary)*1.5,v.bary);
    return float4(1-min(edge.x,min(edge.y,edge.z)),0,0,0);
}
struct ShadowVarying {float4 clip:SV_Position;float2 uv:TEXCOORD0;float2 uv1:TEXCOORD1;nointerpolation uint material:TEXCOORD2;};
[shader("vertex")]
ShadowVarying nativeShadowVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){Varying v=nativeVertex(vertex,instance,true,true);ShadowVarying s;s.clip=v.clip;s.uv=v.uv;s.uv1=v.uv1;s.material=v.material;return s;}
NativeMaterial material(uint id){return buffers[NonUniformResourceIndex(root.materials)].Load<NativeMaterial>(id*320);}
float2 coordinates(NativeUv transform,Varying v){float2 p=(transform.set==0?v.uv:v.uv1)*transform.scale;float s,c;sincos(transform.rotation,s,c);return transform.offset+float2(c*p.x-s*p.y,s*p.x+c*p.y);}
// Texture footprint scale of the pass (Frame.upscale.z, root word 4): surface()
// sets it, so upscaled G-buffer and forward fragments choose the display
// resolution's mips (a negative mip bias the temporal resolve reconstructs);
// shadow passes keep 1.
static float footprint=1;
float4 sampleMap(NativeMaterial m,uint slot,Varying v){
    uint id=slot<4?m.pbr.textures[min(slot,3)]:(slot==4?m.slots.x:m.slots.y);
    if(id==0xffffffff)return float4(slot==1?float3(0,0,1):float3(1),1);
    float2 uv=coordinates(m.uv[slot],v);
    float lod=ntDesiredLod(root.sample,id,ddx(uv)*footprint,ddy(uv)*footprint);
    if(slot==1)return float4(ntNormal(root.sample,id,uv,lod,m.pbr.surface.y),1);
    bool decode=slot==0?(m.pbr.flags.y&(1u<<25))!=0:slot==3 && (m.pbr.flags.y&(1u<<26))!=0;
    return ntSampleColorLevel(root.sample,id,uv,lod,decode);
}
// Unit `v`, or `fallback` for zero-length or non-finite input (degenerate skinned or
// UV-less tangent frames would otherwise turn shading into NaN/inf fireflies).
float3 safeNormalize(float3 v,float3 fallback){float l=dot(v,v);return l>1e-12 && l<1e30?v*rsqrt(l):fallback;}
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
    float4 weights=v.weights;for(uint i=m.slots.z;i<4;i++)weights[i]=0;
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
        float w=weights[i];
        [branch]if(max(max(w,QuadReadAcrossX(w)),max(QuadReadAcrossY(w),QuadReadAcrossDiagonal(w)))<=0)continue;
        NativeMaterial layer=material(m.layers[i]);
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
// A native fragment's surface. SIDES is a compile-time choice: the side projections
// cost every G-buffer pixel registers even when no terrain uses them, so presets
// without terrain detail bind a PSO compiled without (nativeGeometryTopFragment).
Surface surface<let SIDES:int>(Varying v){
    footprint=root.cascade==0?1:asfloat(root.cascade);NativeMaterial m=material(v.material);
    // Guessed loading bounds must not cover the view or form stair-like bands
    // across pending foliage. Keep only short one-pixel corner brackets; tiny
    // distant bounds disappear. Failed/missing bounds remain opaque magenta.
    if((m.pbr.flags.y&(1u<<27))!=0 && m.pbr.flags.x==2){
        float2 step=max(fwidth(v.uv),float2(1e-6));
        float2 edge=min(v.uv,1-v.uv)/step;
        if(min(edge.x,edge.y)>1 || max(edge.x,edge.y)>8 || max(step.x,step.y)>0.125)discard;
    }
    Surface s=m.slots.z>0?terrainSurface<SIDES>(m,v):surfaceMaterial(m,v);
    if(m.pbr.flags.x==1 && s.alpha<m.pbr.surface.w)discard;
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
GBuffer geometry<let SIDES:int>(Varying v,bool front){Surface s=surface<SIDES>(v);if(backFace(v,front))s.normal=-s.normal;GBuffer g;g.albedo=float4(s.albedo,s.ao);g.normal=gbufferEncodeNormal(s.normal,1);g.material=gbufferEncodeMaterial(s.roughness,s.metal,material(v.material).pbr.flags.x==1?0:1,1);g.emissive=gbufferEncodeEmissive(s.emissive,1);g.id=float(v.id);g.velocity=motion(v);return g;}
[shader("fragment")]
GBuffer nativeGeometryFragment(Varying v,bool front:SV_IsFrontFace){return geometry<1>(v,front);}
// The G-buffer for presets without terrain detail: terrain keeps its top projection.
[shader("fragment")]
GBuffer nativeGeometryTopFragment(Varying v,bool front:SV_IsFrontFace){return geometry<0>(v,front);}
[shader("fragment")]
void nativeShadowFragment(ShadowVarying shadow){Varying v=(Varying)0;v.uv=shadow.uv;v.uv1=shadow.uv1;NativeMaterial m=material(shadow.material);if(m.pbr.flags.x==1 && sampleMap(m,0,v).a*m.pbr.color.a<m.pbr.surface.w)discard;}
// Translucent sun casters' film opacity (look-translucent.slangh): coverage times the
// share the film stops, blended ONE, ONE_MINUS_SRC_ALPHA from a zero colour over a
// map cleared to one, so the map keeps the product of (1 - opacity) over every film.
[shader("fragment")]
float4 nativeShadowTranslucentFragment(ShadowVarying shadow):SV_Target0{
    Varying v=(Varying)0;v.uv=shadow.uv;v.uv1=shadow.uv1;NativeMaterial m=material(shadow.material);
    float alpha=saturate(sampleMap(m,0,v).a*m.pbr.color.a)*(1-saturate(m.factors.y*sampleMap(m,5,v).r));
    return float4(0,0,0,alpha);
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
Surface forwardSurface<let FACES:int>(Varying v){
    bool front=facesViewer(v);if((FACES==1 && front)||(FACES==2 && !front))discard;
    Surface s=surface<1>(v);if(!front)s.normal=-s.normal;
    s.roughness=max(s.roughness,sqrt(max(look(frame()).sunLux.w,0)));return s;
}
// The forward pass; `high` selects the High/Epic tier's code at compile time.
Forward nativeForward(Varying v,bool high){Surface s=forwardSurface<0>(v);Frame f=frame();Look l=look(f);Forward o;o.color=forwardColour(f,l,s,v.world,int2(v.clip.xy),high);o.id=float(v.id);o.albedo=float4(s.albedo,s.alpha);o.normal=gbufferEncodeNormal(s.normal,s.alpha);o.material=gbufferEncodeMaterial(s.roughness,s.metal,1,s.alpha);o.emissive=gbufferEncodeEmissive(s.emissive,s.alpha);o.velocity=float4(motion(v),0,1);return o;}
[shader("fragment")]
Forward nativeForwardFragment(Varying v){return nativeForward(v,false);}
// Lit frames' forward pass: nothing reads the G-buffer after it (only the debug
// views do, through nativeForwardFragment), so it writes colour, id and motion only;
// `high` selects the High/Epic tier's code at compile time.
struct ForwardLit {float4 color:SV_Target0;float id:SV_Target1;float4 velocity:SV_Target2;};
ForwardLit nativeForwardLit<let FACES:int>(Varying v,bool high){Surface s=forwardSurface<FACES>(v);Frame f=frame();Look l=look(f);ForwardLit o;o.color=forwardColour(f,l,s,v.world,int2(v.clip.xy),high);o.id=float(v.id);o.velocity=float4(motion(v),0,1);return o;}
[shader("fragment")]
ForwardLit nativeForwardLitFragment(Varying v){return nativeForwardLit<0>(v,false);}
[shader("fragment")]
ForwardLit nativeForwardLitFrontFragment(Varying v){return nativeForwardLit<2>(v,false);}
[shader("fragment")]
ForwardLit nativeForwardLitBackFragment(Varying v){return nativeForwardLit<1>(v,false);}
[shader("fragment")]
ForwardLit nativeForwardHighFragment(Varying v){return nativeForwardLit<0>(v,true);}
[shader("fragment")]
ForwardLit nativeForwardHighFrontFragment(Varying v){return nativeForwardLit<2>(v,true);}
[shader("fragment")]
ForwardLit nativeForwardHighBackFragment(Varying v){return nativeForwardLit<1>(v,true);}
// Transmissive forward draws (NativePipelines::refract): the surface composites the
// opaque scene it refracts (look-refract.slangh) instead of blending a film. A
// volume's back faces are its far side, which its front faces already refract
// through; a thin wall shades both sides, facing the viewer.
ForwardLit nativeRefract(Varying v,bool high){
    NativeMaterial m=material(v.material);uint2 volume=uint2(asuint(m.factors.z),asuint(m.factors.w));
    bool front=facesViewer(v);if(!front && (volume.x&0xffff)!=0)discard;
    Surface s=forwardSurface<0>(v);float3 geometric=safeNormalize(v.normal,float3(0,1,0))*(front?1:-1);
    // The instance's scale (the cube root of its determinant) for the mesh-space thickness.
    float4x4 world=buffers[NonUniformResourceIndex(root.material)].Load<float4x4>(128+(v.id-1)*192+16);
    float scale=pow(abs(determinant((float3x3)world)),1./3.);
    Frame f=frame();Look l=look(f);ForwardLit o;o.color=refractColour(f,l,s,volume,scale,v.world,geometric,int2(v.clip.xy),high);
    o.id=float(v.id);o.velocity=float4(motion(v),0,1);return o;
}
[shader("fragment")]
ForwardLit nativeForwardRefractFragment(Varying v){return nativeRefract(v,false);}
[shader("fragment")]
ForwardLit nativeForwardRefractHighFragment(Varying v){return nativeRefract(v,true);}
