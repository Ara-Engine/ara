// Planar reflections (look/planar.rs, renderer/views/planar.rs): after the mirror view
// rendered (its camera mirrored below the floor plane, native draws clipped to what
// lies above it), `planarCopyMain` turns its lit HDR into atlas level 0 and
// `planarMipMain` reduces levels 1..PLANAR_LEVELS (look-planar.slangh layout).
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
// Root words: frame the mirror view's record, materials its HDR (sampled), material the
// atlas (storage), entity the level (planarMipMain), cascade the plane height (f32
// bits), mode and sample level 0's width and height.
RWTexture2D<float4> planarAtlas(){return images[NonUniformResourceIndex(root.material)];}
// Level 0: absolute radiance (the HDR target is pre-exposed: look.slangh hdrExposure),
// and how far the reflected ray travelled from the plane to what the texel shows. Open
// sky is re-evaluated without the sun disk and emission the host promoted to emitter
// lights comes off (as reflection capture faces do): shading adds both lights'
// specular analytically.
[shader("compute")][numthreads(8,8,1)]
void planarCopyMain(uint3 tid:SV_DispatchThreadID){
    uint2 size=uint2(root.mode,root.sample);if(any(tid.xy>=size))return;
    Frame f=frame();Look l=look(f);int2 p=int2(tid.xy);
    float3 c=pixel(root.materials,p).rgb/hdrExposure(f,l);
    float depth=pixel(f.extra.x,p).x;bool open=depth<=0;
    float3 x=worldAtDepth(f,float2(p)+0.5,open?1e-7:depth),d=normalize(x-f.camera.xyz);
    if(open&&physicalSky(l))c=skyRadiance(f,l,d,false);
    float3 e=gbufferEmissive(f,p);if(!open&&luminance(e)>l.screenParams[1].x)c=max(c-e,0);
    // The mirror camera sits below the plane: its ray crosses the plane first, where
    // the reflection leaves the floor.
    float height=asfloat(root.cascade),enter=(height-f.camera.y)/max(d.y,1e-4);
    float travelled=open?PLANAR_SKY:min(max(length(x-f.camera.xyz)-enter,0),PLANAR_SKY);
    planarAtlas()[tid.xy]=float4(all(c==c)?clamp(c,0,60000):0,travelled);
}
// Level `root.entity` (1..PLANAR_LEVELS) from the level above: the 2x2 box's mean
// radiance and nearest distance.
[shader("compute")][numthreads(8,8,1)]
void planarMipMain(uint3 tid:SV_DispatchThreadID){
    uint2 base=uint2(root.mode,root.sample);uint k=root.entity;
    uint2 size=planarSize(base,k),above=planarSize(base,k-1);if(any(tid.xy>=size))return;
    uint2 from=planarOrigin(base,k-1);float3 c=0;float t=PLANAR_SKY;
    for(uint i=0;i<4;i++){float4 s=planarAtlas()[from+min(tid.xy*2+uint2(i&1,i>>1),above-1)];c+=s.rgb;t=min(t,s.a);}
    planarAtlas()[planarOrigin(base,k)+tid.xy]=float4(c*0.25,t);
}
