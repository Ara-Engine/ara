// Traced reflections (look/passes.rs), noise-free: one mirror ray per
// half-resolution pixel (no stochastic lobe sampling). First on screen: traced through
// the Hi-Z pyramid of the G-buffer depth (look-screen.slangh hizTrace), then shaded from last frame's
// resolved colour, roughness widening it through a colour pyramid instead of
// jittered rays. What the screen cannot show (the ray leaves it, or its hit fades at
// the edge) continues through the reflection captures around the surface
// (look-captures.slangh captureTrace), so a floor reflects the ceiling above and
// behind the camera. The resolve (ssrResolveMain) then spreads each pixel's confidence
// over its lobe's footprint, so where the mirror ray's hit gives way to a miss (an
// object's edge seen in a rough floor) the reflection fades over the lobe's width
// instead of printing the edge. Lighting blends the result over capture, probe and
// sky reflections by its confidence; a negative confidence marks a result mostly from
// the captures (the reflection debug view's sources).
#include "raster.slangh"
#include "look.slangh"
#include "look-captures.slangh"
#include "look-screen.slangh"
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
        if(t>-1.5)return float4(captureTraced(l,d,ratio,o,r,t,reach,roughness),ratio.w);
    }
    return 0;
}
// The GGX footprint of a reflection that travelled `t` from `world` (normal `n`,
// mirror direction `r`, unit view direction `v` from the camera, perceptual
// `roughness`) as the reflection appears on screen, for the resolve's tent: half of
// GGX's reflected directions lie within 2 alpha of the mirror direction, and half a
// tent's weight within half its radius, so the tent spans 4 t alpha in the plane of
// incidence (4 t alpha N.V across it), mirrored to the virtual image along the camera
// ray, t beyond the surface. Two semi-axes in half-resolution pixels (xy in-plane, zw
// across), each at most an eighth of the render height.
float4 ssrFootprint(Frame f,float3 world,float3 n,float3 v,float3 r,float t,float roughness){
    float spread=4*t*roughness*roughness,facing=max(saturate(-dot(n,v)),0.05);
    float3 inPlane=n-r*dot(n,r);
    inPlane=dot(inPlane,inPlane)>1e-6?normalize(inPlane):normalize(cross(r,abs(r.y)<0.99?float3(0,1,0):float3(1,0,0)));
    float3 across=cross(r,inPlane),image=world+v*t;
    // Mirrored about the surface plane.
    float3 a=inPlane*spread,b=across*spread*facing;a-=2*n*dot(n,a);b-=2*n*dot(n,b);
    float2 c=ssrProject(f,image).xy,u=(ssrProject(f,image+a).xy-c)*0.5,w=(ssrProject(f,image+b).xy-c)*0.5;
    float cap=f.extent.y/8.;
    u*=min(1,cap/max(length(u),1e-4));w*=min(1,cap/max(length(w),1e-4));
    return float4(u,w);
}
// rgb reflected radiance, a confidence (negative: mostly from the captures); half
// resolution (the full-resolution pixel 2p). The footprint image gets the pixel's
// reflection footprint (ssrFootprint; zero where nothing is traced).
[shader("compute")][numthreads(8,8,1)]
void ssrTraceMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 p=int2(tid.xy)*2;bool covered;float3 world=gbufferWorld(f,p,covered);float roughness=gbufferMaterial(f,p).x;float4 result=0;
    // Hybrid frames' traced reflections own surfaces well below their cut
    // (look-rt.slang rtFilterMain): no march there (rtInfo[1].w bit 0, look-rt.slangh).
    bool traced=(l.rtInfo[1].w&1)!=0 && roughness<0.8*l.rtParams[0].x;float4 footprint=0;
    if(covered && roughness<l.ssr.x && !traced){
        float3 n=normalize(gbufferNormal(f,p)),v=normalize(world.xyz-f.camera.xyz),r=reflect(v,n);
        // The ray's screen segment (look-screen.slangh screenSegment: clipped in front of
        // the near plane, where a segment reaching behind the camera has no linear
        // projection, then at the screen's edge), traversed through the Hi-Z pyramid: a
        // few dozen steps reach any length exactly, where a linear march skipped thin
        // geometry between its samples. A crossing far behind the depth it passed (the
        // ray went behind a foreground object, not into it) is no hit: the traversal
        // crawls on behind it until the ray comes out in front again and can meet what
        // lies beyond (no hard-edged holes behind every nearer pillar or arch).
        float3 a,b;float reach=screenSegment(f,world.xyz+n*0.01,r,l.ssr.y,a,b);
        float s=hizTrace(f,l,a,b-a,uint(l.ssr.z),uint(l.screenParams[0].y),0.02);
        bool hit=s>=0;float3 at=hit?lerp(a,b,s):a;
        // A miss travelled to where its segment leaves the screen (or its reach).
        float travelled=length(worldAtDepth(f,b.xy,b.z)-world.xyz);
        if(hit){
            // Last frame's colour at the hit, reprojected.
            float3 hitWorld=worldAtDepth(f,floor(at.xy)+0.5,gbufferDepth(f,int2(at.xy)));travelled=length(hitWorld-world.xyz);
            float4 h=mul(f.previousVp,float4(hitWorld,1));float2 uv=float2(h.x/h.w*0.5+0.5,0.5-h.y/h.w*0.5);
            if(h.w>0 && all(uv>0) && all(uv<1)){
                // Cone footprint: the GGX lobe (half-angle ~ alpha) spreads over the
                // travelled distance times alpha at the hit, but anisotropically: within
                // the plane of incidence fully, across it only by N.V, so a floor seen at
                // a grazing angle reflects sharp side to side and streaks towards the
                // viewer. The narrow width in pixels picks the pyramid level (level k
                // texels span 2^k pixels, at least the taps' spacing); five taps along the
                // projected long axis stretch it. Lobes wider than the pyramid read its coarsest level:
                // still the hit's own surroundings, which the probe radiance only
                // approximates.
                float alpha=roughness*roughness,spread=length(hitWorld-world.xyz)*alpha;
                float3 inPlane=n-r*dot(n,r);
                inPlane=dot(inPlane,inPlane)>1e-6?normalize(inPlane):normalize(cross(r,abs(r.y)<0.99?float3(0,1,0):float3(1,0,0)));
                float3 across=cross(r,inPlane);float facing=max(saturate(-dot(n,v)),0.05);
                float2 centre=ssrProject(f,hitWorld).xy;
                float wide=2*length(ssrProject(f,hitWorld+across*spread*facing).xy-centre);
                float2 along=ssrProject(f,hitWorld+inPlane*spread).xy-centre;
                along*=min(1,f.extent.y*0.25/max(length(along),1e-3));
                // The taps lie half the long axis apart: a level whose texels span less
                // would print five sharp copies of the hit (a sphere's reflection
                // ghosted down a rough floor) instead of streaking it.
                float level=clamp(log2(max(max(wide,length(along)*0.5),1)),0,SSR_LEVELS);along/=float2(f.extent.xy);
                float3 colour=0;float total=0;
                for(int k=-2;k<=2;k++){float w=exp(-float(k*k)*0.5);colour+=ssrPyramid(f,l,uv+along*(float(k)*0.5),level)*w;total+=w;}
                colour/=total;
                // Emission the host promoted to emitter lights (brighter than
                // screenParams[1].x) already reaches this surface's specular as a light:
                // reflect what surrounds the emitter, not its glow again (as captures and
                // screen probes drop it).
                float3 emitted=gbufferEmissive(f,int2(at.xy));
                if(luminance(emitted)>l.screenParams[1].x)colour=max(colour-emitted,0);
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
        // Bounded by the view distance: a miss beside a silhouette, which travelled
        // to the screen's edge, and the hit beside it then spread alike (the on-screen
        // blur hardly grows past t = view distance), while contact reflections keep
        // their short, sharp footprints.
        footprint=ssrFootprint(f,world.xyz,n,v,r,min(travelled,length(world.xyz-f.camera.xyz)),roughness);
    }
    images[NonUniformResourceIndex(l.ssrImages.w)][tid.xy]=result;
    images[NonUniformResourceIndex(l.ssrResolve.z)][tid.xy]=footprint;
}
// Taps of the resolve's footprint gather: a golden-angle spiral over the unit disc.
static const uint SSR_RESOLVE_TAPS=24;
// Half resolution, after the trace: each pixel's confidence becomes its mean over the
// pixel's own footprint (ssrFootprint), tent-weighted, counting only taps on its own
// surface (within 2% of the view distance of its tangent plane, normals within some 18
// degrees: a curved mirror's footprint is not a plane's). Its colour stays its own
// hit's (with its direct neighbours'), which the trace already filtered over the lobe's cone (ssrTraceMain's pyramid
// footprint): averaging the neighbours' cones again blurred every glossy reflection
// twice (a mirror floor smeared like a rough one). Only where its own ray found little
// (a miss beside a hit, the fringe at a silhouette) does the confidence-weighted mean of
// the footprint's hits fill in, so the hit/miss edge fades over the lobe's width
// instead of printing it. Lobes narrower than a pixel pass through unchanged.
[shader("compute")][numthreads(8,8,1)]
void ssrResolveMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);int2 half=int2((f.extent.xy+1)/2),p=int2(tid.xy);if(any(p>=half))return;
    float4 own=pixel(l.ssrResolve.x,p),footprint=pixel(l.ssrResolve.y,p);
    if(max(dot(footprint.xy,footprint.xy),dot(footprint.zw,footprint.zw))<0.5){
        images[NonUniformResourceIndex(l.ssrResolve.w)][p]=own;return;
    }
    bool covered;float3 world=gbufferWorld(f,p*2,covered),n=normalize(gbufferNormal(f,p*2));
    float tolerance=0.02*max(dot(world-f.camera.xyz,f.forward.xyz),0.1);
    float confidence=abs(own.a),captured=own.a<0?confidence:0,weights=1;float3 colour=own.rgb*confidence;
    for(uint i=0;i<SSR_RESOLVE_TAPS;i++){
        float radius=sqrt((float(i)+0.5)/SSR_RESOLVE_TAPS),angle=float(i)*2.39996323;
        float2 offset=footprint.xy*(radius*cos(angle))+footprint.zw*(radius*sin(angle));
        int2 q=clamp(p+int2(round(offset)),int2(0),half-1);
        float3 at=gbufferWorld(f,q*2,covered);
        if(!covered||abs(dot(at-world,n))>tolerance||dot(gbufferNormal(f,q*2),n)<0.95)continue;
        float w=1-radius;float4 t=pixel(l.ssrResolve.x,q);float c=abs(t.a);
        confidence+=c*w;captured+=(t.a<0?c:0)*w;colour+=t.rgb*c*w;weights+=w;
    }
    // The own hit's colour, tent-filtered with its four neighbours on the same surface
    // (a full-resolution pixel or two): no half-resolution blocks where on-screen hits
    // meet capture traces.
    float ownWeight=abs(own.a)*2;float3 near=own.rgb*ownWeight;
    for(uint k=0;k<4;k++){
        int2 q=clamp(p+int2(k==0?1:k==1?-1:0,k==2?1:k==3?-1:0),int2(0),half-1);
        float3 at=gbufferWorld(f,q*2,covered);
        if(!covered||abs(dot(at-world,n))>tolerance||dot(gbufferNormal(f,q*2),n)<0.95)continue;
        float4 t=pixel(l.ssrResolve.x,q);near+=t.rgb*abs(t.a);ownWeight+=abs(t.a);
    }
    float a=confidence/weights;float3 c=lerp(colour/max(confidence,1e-5),near/max(ownWeight,1e-5),saturate(abs(own.a)*4));
    images[NonUniformResourceIndex(l.ssrResolve.w)][p]=float4(c,captured*2>confidence?-a:a);
}
