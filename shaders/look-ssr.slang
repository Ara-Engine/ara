// Traced reflections (look/passes.rs), noise-free: one mirror ray per
// half-resolution pixel (no stochastic lobe sampling). First on screen: marched
// against the G-buffer depth with a binary refinement, then shaded from last frame's
// resolved colour, roughness widening it through a colour pyramid instead of
// jittered rays. What the screen cannot show (the ray leaves it, or its hit fades at
// the edge) continues through the reflection captures around the surface
// (look-captures.slangh captureTrace), so a floor reflects the ceiling above and
// behind the camera. Lighting blends the result over capture, probe and sky
// reflections by its confidence; a negative confidence marks a result mostly from
// the captures (the reflection debug view's sources).
#include "raster.slangh"
#include "look.slangh"
#include "look-captures.slangh"
// Pixel position (and reversed-Z depth) of a world point in the current frame.
float3 ssrProject(Frame f,float3 world){
    float4 c=mul(f.vp,float4(world,1));float3 n=c.xyz/c.w;return float3((n.x*0.5+0.5)*f.extent.x,(0.5-n.y*0.5)*f.extent.y,n.z);
}
// Colour pyramid of the previous resolved frame: levels 1..SSR_LEVELS of the render size
// packed side by side (as the bloom atlas), sampled between levels by roughness.
static const uint SSR_LEVELS=6;
uint2 ssrLevelSize(Frame f,uint level){return max(f.extent.xy>>level,uint2(1));}
uint2 ssrLevelOrigin(Frame f,uint level){uint x=0;for(uint j=1;j<level;j++)x+=max(f.extent.x>>j,1u);return uint2(x,0);}
float3 ssrLevel(Frame f,Look l,uint level,float2 uv){
    float2 size=float2(ssrLevelSize(f,level)),atlas=float2(f.extent.x,max(f.extent.y/2,1u));
    return historyTex(l.ssrImages.x,(float2(ssrLevelOrigin(f,level))+clamp(uv*size,0.5,size-0.5))/atlas).rgb;
}
// Level 0: last frame's full-resolution history itself (root word 1, pre-exposed), so a
// near-mirror lobe reflects at full detail instead of the half-size level 1.
float3 ssrSharp(Frame f,Look l,float2 uv){return historyTex(root.materials,uv).rgb/exposureScale(f,exposureState(l).y);}
// The colour pyramid at a fractional level (0: the sharp history).
float3 ssrPyramid(Frame f,Look l,float2 uv,float level){
    uint lo=uint(level);
    return level<1?lerp(ssrSharp(f,l,uv),ssrLevel(f,l,1,uv),level):lerp(ssrLevel(f,l,lo,uv),ssrLevel(f,l,min(lo+1,SSR_LEVELS),uv),frac(level));
}
// Root word 2: pass (0 pyramid level 1 from the history, 1..5 levels 2..SSR_LEVELS).
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
// The reflection along `r` from `world` (normal `n`) the first capture around it
// that can tell resolves, in blending order: rgb radiance, a confidence (zero when
// none can). Unbaked captures, those not reaching `world` and those that cannot see
// it (a capture below a raised floor sees only its underside) are skipped; a ray
// that only passes behind what a capture saw moves on to the next.
float4 ssrCaptures(Look l,float3 world,float3 n,float3 r,float roughness,float reach){
    uint mask=l.captureInfo[1].z,tried=0;
    while(mask!=0&&tried<3){
        uint i=firstbitlow(mask);mask&=mask-1;
        CaptureCull c=captureCull(l,i);float3 local;if(captureInfluence(c,world,local)<=0)continue;
        float4 ratio=captureRatio(l,i);if(ratio.w<=0)continue;
        CaptureShade d=captureShade(l,i);
        float3 q=world-d.origin.xyz;float far=max(length(q),1e-4);
        if(far>captureSeen(l,d,q/far,0)+max(0.1,far*28.8/float(d.tile.z)))continue;
        tried++;
        // Start two of the capture's texels off the surface, past its own distance noise.
        float3 o=world+n*(0.02+length(world-d.origin.xyz)*7.2/float(d.tile.z));
        float t=captureTrace(l,d,o,r,reach);
        if(t>-1.5)return float4(captureTraced(l,d,ratio,o,r,t,reach,roughness),1);
    }
    return 0;
}
// rgb reflected radiance, a confidence (negative: mostly from the captures); half
// resolution (the full-resolution pixel 2p).
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
        float reach=l.ssr.y;
        float ahead=dot(world.xyz-f.camera.xyz,f.forward.xyz),toward=dot(r,f.forward.xyz);
        if(toward<0)reach=min(reach,max(ahead-f.camera.w*2,0)/-toward);
        float3 a=ssrProject(f,world.xyz+n*0.01),b=ssrProject(f,world.xyz+r*reach);
        // Then end it at the screen's edge (depth is affine in screen space, so the
        // clipped end stays on the ray): every step lands on a pixel that can hit.
        // A floor's reflection of a ceiling ahead travels several times the floor's own
        // distance, so the ray is never cut shorter than the scene it can see.
        float2 span=b.xy-a.xy,bound=float2(f.extent.xy)-0.5;float keep=1;
        if(span.x>0)keep=min(keep,(bound.x-a.x)/span.x);else if(span.x<0)keep=min(keep,(0.5-a.x)/span.x);
        if(span.y>0)keep=min(keep,(bound.y-a.y)/span.y);else if(span.y<0)keep=min(keep,(0.5-a.y)/span.y);
        b=lerp(a,b,saturate(keep));
        uint steps=uint(l.ssr.z);float jitter=0.5;
        // A crossing whose refined point lies far behind the depth it passed (the
        // ray went behind a foreground object, not into it) is no hit: the march
        // continues until the ray comes out in front again and can meet what lies
        // beyond. Stopping there left hard-edged holes, every reflection a nearer
        // pillar or arch shadows on screen.
        float previous=0;bool hit=false,behind=false;float3 at=a;
        for(uint i=1;i<=steps && !hit;i++){
            float t=(float(i)-1+jitter)/float(steps);float3 q=lerp(a,b,t);
            if(any(q.xy<0)||any(q.xy>=float2(f.extent.xy)))break;
            float scene=pixel(f.extra.x,int2(q.xy)).x;
            // Reversed-Z: the ray is behind the surface when its depth is smaller.
            if(scene>0 && q.z<scene){
                if(behind)continue;
                float lo=previous,hi=t;
                for(uint k=0;k<5;k++){float m=(lo+hi)*0.5;float3 c=lerp(a,b,m);float s=pixel(f.extra.x,int2(c.xy)).x;if(c.z<s)hi=m;else lo=m;}
                at=lerp(a,b,hi);float s=pixel(f.extra.x,int2(at.xy)).x;
                // Reject hits behind thick geometry: compare world distances.
                float3 surface=worldAtDepth(f,floor(at.xy)+0.5,s),ray=worldAtDepth(f,at.xy,at.z);
                hit=length(surface-ray)<max(0.1,length(surface-f.camera.xyz)*0.02);
                behind=!hit;
            }else{behind=false;previous=t;}
        }
        if(hit){
            // Last frame's colour at the hit, reprojected.
            float3 hitWorld=worldAtDepth(f,floor(at.xy)+0.5,gbufferDepth(f,int2(at.xy)));
            float4 h=mul(f.previousVp,float4(hitWorld,1));float2 uv=float2(h.x/h.w*0.5+0.5,0.5-h.y/h.w*0.5);
            if(h.w>0 && all(uv>0) && all(uv<1)){
                // Cone footprint: the GGX lobe (half-angle ~ alpha) spreads over the
                // travelled distance times alpha at the hit, but anisotropically: within
                // the plane of incidence fully, across it only by N.V, so a floor seen at
                // a grazing angle reflects sharp side to side and streaks towards the
                // viewer. The narrow width in pixels picks the pyramid level (level k
                // texels span 2^k pixels); five taps along the projected long axis
                // stretch it. Lobes wider than the pyramid read its coarsest level:
                // still the hit's own surroundings, which the probe radiance only
                // approximates.
                float alpha=roughness*roughness,spread=length(hitWorld-world.xyz)*alpha;
                float3 inPlane=n-r*dot(n,r);
                inPlane=dot(inPlane,inPlane)>1e-6?normalize(inPlane):normalize(cross(r,abs(r.y)<0.99?float3(0,1,0):float3(1,0,0)));
                float3 across=cross(r,inPlane);float facing=max(saturate(-dot(n,v)),0.05);
                float2 centre=ssrProject(f,hitWorld).xy;
                float wide=2*length(ssrProject(f,hitWorld+across*spread*facing).xy-centre);
                float2 along=ssrProject(f,hitWorld+inPlane*spread).xy-centre;
                along*=min(1,f.extent.y*0.25/max(length(along),1e-3))/float2(f.extent.xy);
                float level=clamp(log2(max(wide,1)),0,SSR_LEVELS);
                float3 colour=0;float total=0;
                for(int k=-2;k<=2;k++){float w=exp(-float(k*k)*0.5);colour+=ssrPyramid(f,l,uv+along*(float(k)*0.5),level)*w;total+=w;}
                colour/=total;
                float2 edge=min(uv,1-uv)*8;
                float fade=saturate(min(edge.x,edge.y))*(1-saturate((roughness/l.ssr.x-0.8)/0.2));
                result=float4(colour,fade);
            }
        }

        // Whatever the screen leaves open, the captures trace on.
        if(result.a<0.999&&l.captureInfo[1].z!=0){
            float4 captured=ssrCaptures(l,world.xyz,n,r,roughness,reach)*(1-saturate((roughness/l.ssr.x-0.8)/0.2));
            float share=captured.a*(1-result.a),total=result.a+share;
            if(total>0)result=float4((result.rgb*result.a+captured.rgb*share)/total,share>result.a?-total:total);
        }
    }
    images[NonUniformResourceIndex(l.ssrImages.w)][tid.xy]=result;
}
