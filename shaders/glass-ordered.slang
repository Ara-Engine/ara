// The nearest rasterized glass interface starts a world-space ordered path.
// Deeper raster layers discard, because the path already evaluates them.
#include "native-raster.slang"
#include "glass-path.slangh"
struct GlassVarying {float4 clip:SV_Position;float3 world:TEXCOORD0;float4 currentClip:TEXCOORD1;float4 previousClip:TEXCOORD2;nointerpolation uint id:TEXCOORD3;};
[shader("vertex")]
GlassVarying glassOrderedVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){
    Varying v=nativeVertex(vertex,instance,false,true);GlassVarying o;
    o.clip=v.clip;o.world=v.world;o.currentClip=v.currentClip;o.previousClip=v.previousClip;o.id=v.id;return o;
}
[shader("fragment")]
ForwardLit glassOrderedFragment(GlassVarying i){
    Varying v=(Varying)0;v.clip=i.clip;v.world=i.world;v.currentClip=i.currentClip;v.previousClip=i.previousClip;v.id=i.id;
    Frame f=frame();Look l=look(f);
    float3 direction=normalize(v.world-f.camera.xyz);TraceHit first;
    if(!reflectClosest(l,f.camera.xyz,direction,l.rtParams[0].y,REFLECT_ALL,first) || first.user!=v.id-1)discard;
    // Back faces reached through the same volume's near face are not primary.
    if(abs(first.t-length(v.world-f.camera.xyz))>max(0.002,first.t*0.0001))discard;
    float3 c=glassPathRadiance(f,l,f.camera.xyz,direction,0);
    ForwardLit o;o.color=float4(exposeHdr(f,l,c),1);o.id=float(v.id);o.velocity=float4(motion(v),0,1);return o;
}
