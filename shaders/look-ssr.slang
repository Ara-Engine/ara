// Screen-space reflections (look/passes.rs), noise-free: one mirror ray per
// half-resolution pixel (no stochastic lobe sampling), marched against the
// G-buffer depth with a binary refinement, then shaded from last frame's resolved
// colour. Roughness widens the reflection by reading a coarser level of a colour
// pyramid instead of jittering rays; lighting blends it over probe and environment
// reflections by its confidence.
#include "raster.slangh"
#include "look.slangh"
// Pixel position (and reversed-Z depth) of a world point in the current frame.
float3 ssrProject(Frame f,float3 world){
    float4 c=mul(f.vp,float4(world,1));float3 n=c.xyz/c.w;return float3((n.x*0.5+0.5)*f.extent.x,(0.5-n.y*0.5)*f.extent.y,n.z);
}
// Colour pyramid of the previous resolved frame: levels 1..4 of the render size
// packed side by side (as the bloom atlas), sampled between levels by roughness.
uint2 ssrLevelSize(Frame f,uint level){return max(f.extent.xy>>level,uint2(1));}
uint2 ssrLevelOrigin(Frame f,uint level){uint x=0;for(uint j=1;j<level;j++)x+=max(f.extent.x>>j,1u);return uint2(x,0);}
float3 ssrLevel(Frame f,Look l,uint level,float2 uv){
    float2 size=float2(ssrLevelSize(f,level)),atlas=float2(f.extent.x,max(f.extent.y/2,1u));
    return historyTex(l.ssrImages.x,(float2(ssrLevelOrigin(f,level))+clamp(uv*size,0.5,size-0.5))/atlas).rgb;
}
// Root word 2: pass (0 pyramid level 1 from the history, 1..3 levels 2..4).
[shader("compute")][numthreads(8,8,1)]
void ssrPyramidMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint level=root.material+1;uint2 size=ssrLevelSize(f,level),p=tid.xy;if(any(p>=size))return;
    float3 c;
    // Last frame's history is pre-exposed (look.slangh exposureState).
    if(level==1){float2 uv=(float2(p)+0.5)/float2(size);c=historyTex(root.materials,uv).rgb/exposureScale(f,exposureState(l).y);}
    else{uint2 o=ssrLevelOrigin(f,level-1);uint2 s=ssrLevelSize(f,level-1);c=0;
        for(uint k=0;k<4;k++){uint2 q=min(p*2+uint2(k&1,k>>1),s-1);c+=images[NonUniformResourceIndex(l.ssrImages.z)][o+q].rgb;}c/=4;}
    images[NonUniformResourceIndex(l.ssrImages.z)][ssrLevelOrigin(f,level)+p]=float4(min(c,float3(60000)),1);
}
// rgb reflected radiance, a confidence; half resolution (the full-resolution pixel 2p).
[shader("compute")][numthreads(8,8,1)]
void ssrTraceMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 p=int2(tid.xy)*2;bool covered;float3 world=gbufferWorld(f,p,covered);float roughness=gbufferMaterial(f,p).x;float4 result=0;
    // Hybrid frames' traced reflections own surfaces well below their cut
    // (look-rt.slang rtFilterMain): no march there (rtInfo[1].w bit 0, look-rt.slangh).
    bool traced=(l.rtInfo[1].w&1)!=0 && roughness<0.8*l.rtParams[0].x;
    if(covered && roughness<l.ssr.x && !traced){
        float3 n=normalize(gbufferNormal(f,p)),v=normalize(world.xyz-f.camera.xyz),r=reflect(v,n);
        // Clip the ray in world space so it ends in front of the near plane
        // (camera.w), then march its screen-space projection. A segment reaching
        // behind the camera has no linear projection: clipping it after projecting
        // swept arbitrary pixels and reported false hits for rays reflected back
        // towards the camera (dark discs on mirror spheres).
        float distance=length(world.xyz-f.camera.xyz),reach=min(l.ssr.y,distance*2);
        float ahead=dot(world.xyz-f.camera.xyz,f.forward.xyz),toward=dot(r,f.forward.xyz);
        if(toward<0)reach=min(reach,max(ahead-f.camera.w*2,0)/-toward);
        float3 a=ssrProject(f,world.xyz+n*0.01),b=ssrProject(f,world.xyz+r*reach);
        uint steps=uint(l.ssr.z);float2 delta=(b.xy-a.xy)/float(steps);float jitter=0.5;
        float previous=0;bool hit=false;float3 at=a;
        for(uint i=1;i<=steps;i++){
            float t=(float(i)-1+jitter)/float(steps);float3 q=lerp(a,b,t);
            if(any(q.xy<0)||any(q.xy>=float2(f.extent.xy)))break;
            float scene=pixel(f.extra.x,int2(q.xy)).x;
            // Reversed-Z: the ray is behind the surface when its depth is smaller.
            if(scene>0 && q.z<scene){
                float lo=previous,hi=t;
                for(uint k=0;k<5;k++){float m=(lo+hi)*0.5;float3 c=lerp(a,b,m);float s=pixel(f.extra.x,int2(c.xy)).x;if(c.z<s)hi=m;else lo=m;}
                at=lerp(a,b,hi);float s=pixel(f.extra.x,int2(at.xy)).x;
                // Reject hits behind thick geometry: compare world distances.
                float3 surface=worldAtDepth(f,floor(at.xy)+0.5,s),ray=worldAtDepth(f,at.xy,at.z);
                hit=length(surface-ray)<max(0.1,length(surface-f.camera.xyz)*0.02);
                break;
            }
            previous=t;
        }
        if(hit){
            // Last frame's colour at the hit, reprojected.
            float3 hitWorld=worldAtDepth(f,floor(at.xy)+0.5,gbufferDepth(f,int2(at.xy)));
            float4 h=mul(f.previousVp,float4(hitWorld,1));float2 uv=float2(h.x/h.w*0.5+0.5,0.5-h.y/h.w*0.5);
            if(h.w>0 && all(uv>0) && all(uv<1)){
                // Cone footprint: the GGX lobe (half-angle ~ alpha) spreads over twice
                // the travelled distance times alpha at the hit; its projected size in
                // pixels picks the pyramid level (level k texels span 2^k pixels).
                // Lobes wider than the pyramid fade to the prefiltered environment.
                float alpha=roughness*roughness,footprint=2*length(hitWorld-world.xyz)*alpha;
                float3 toHit=normalize(hitWorld-f.camera.xyz);
                float3 side=normalize(cross(toHit,abs(toHit.y)<0.99?float3(0,1,0):float3(1,0,0)));
                float pixels=length(ssrProject(f,hitWorld+side*footprint).xy-ssrProject(f,hitWorld).xy);
                float level=clamp(log2(max(pixels,2)),1,4);uint lo=uint(level);
                float3 colour=lerp(ssrLevel(f,l,lo,uv),ssrLevel(f,l,min(lo+1,4),uv),frac(level));
                float2 edge=min(uv,1-uv)*8;
                float fade=saturate(min(edge.x,edge.y))*saturate(5-log2(max(pixels,1)))*(1-saturate((roughness/l.ssr.x-0.7)/0.3));
                result=float4(colour,fade);
            }
        }
    }
    images[NonUniformResourceIndex(l.ssrImages.w)][tid.xy]=result;
}
