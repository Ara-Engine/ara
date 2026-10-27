// Ray-traced reflections of the hybrid High/Epic path (D29; renderer::rt), noise
// free: one deterministic ray per pixel along the GGX lobe's dominant direction,
// for every surface below the roughness cut, shaded from the lighting caches
// (look-rt.slangh). Roughness widens the result by a fixed Gaussian whose width
// follows the lobe's footprint at the hit and which never crosses the reflector's
// or the reflected scene's silhouettes; never by stochastic sampling or temporal
// accumulation.
// The filter's second axis composites the traced reflections over screen-space ones
// into the image the lighting_high_rt variant reads instead of screenReflection.
// Below the cut the traced result owns the pixel, so screen-space confidence edges
// (half resolution, jittered every frame) never show there; screen-space
// reflections only fill the band near the cut where traced ones fade out.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "native_sample.slangh"
#include "look-rt.slangh"
// The trace's alpha: the reflected hit distance in metres, RT_SKY for a miss (the
// sky, already prefiltered) and -1 where nothing was traced.
static const float RT_SKY=65000;
// The blur sigma in render pixels of a reflector `distance` from the camera whose
// ray travelled `t`: the reflected lobe's spread at the hit (a Gaussian sigma of
// about 0.7 alpha rad) as the camera sees it across the reflector. Lobes within the
// G-buffer's roughness floor and geometric specular-AA widening (alpha 0.0064) stay
// mirrors: that widening fights highlight aliasing, not reflected detail, which the
// resolve antialiases. Misses (prefiltered) and untraced pixels need none.
float rtSigma(Look l,float roughness,float distance,float t){
    if(t<0 || t>=RT_SKY)return 0;
    return 0.7*max(roughness*roughness-0.0064,0)*t/max((distance+t)*l.rtParams[0].z,1e-6);
}
// The GGX lobe's dominant direction (Lagarde & de Rousiers 2014) for normal `n`,
// view `v` and alpha `a`, kept above the surface where a normal map tilts it below.
float3 rtDominant(float3 n,float3 v,float a){
    float3 d=normalize(lerp(n,reflect(-v,n),(1-a)*(sqrt(1-a)+a)));
    float below=dot(d,n);return below<0.01?normalize(d+n*(0.01-below)):d;
}
// Root word 1: output (storage): rgb reflected radiance, a the hit distance (above).
// Render resolution.
[shader("compute")][numthreads(8,8,1)]
void rtReflectMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=f.extent.xy))return;int2 p=int2(tid.xy);
    float4 result=float4(0,0,0,-1);bool covered;float3 world=gbufferWorld(f,p,covered);float roughness=gbufferMaterial(f,p).x;
    [branch]if(covered && rtEnabled(l) && roughness<l.rtParams[0].x){
        float3 n=normalize(gbufferNormal(f,p)),v=normalize(f.camera.xyz-world);
        float distance=length(world-f.camera.xyz),alpha=roughness*roughness,spread=l.rtParams[0].z;
        float3 d=rtDominant(n,v,alpha);
        // Biased along the normal by the depth reconstruction's and coarser raster
        // LODs' error, which grows with distance.
        float3 origin=world+n*(l.rtParams[1].x+l.rtParams[1].y*distance);
        RtHit hit;float3 radiance;float travelled=RT_SKY;
        if(rtTrace(f,l,origin,d,l.rtParams[0].y,hit,RT_OPAQUE|RT_MASKED,spread)){
            // The ray cone: the pixel's footprint growing with the path plus the lobe.
            float width=spread*(distance+hit.distance)+2*alpha*hit.distance;
            float3 position,geometric;Surface s=rtSurface(l,hit,d,width,position,geometric);
            // Contact darkening where the camera sees the hit too.
            s.ao=min(s.ao,rtScreenVisibility(f,l,position,geometric));
            radiance=rtShadeHit(f,l,s,position,geometric,-d);travelled=hit.distance;
        }else radiance=rtMiss(f,l,d,roughness);
        result=float4(min(radiance,float3(60000)),travelled);
    }
    images[NonUniformResourceIndex(root.materials)][p]=result;
}
// Rough lobes (from where traced mirrors fade out up to the rough cut,
// rtParams[1].w), half resolution: ray-traced specular occlusion of the probe and
// sky reflections. The lobe is a cone of half-angle 1.3 alpha around the dominant
// direction; each half-resolution pixel traces four rays on a ring of it, the ring's
// rotation and radius set by the pixel's 2x2 phase, so any 2x2 block of results
// (rtRoughAt) is one fixed sixteen-direction quadrature: deterministic, never noise.
// A hit blocks its sub-cone by the share of it below the hit surface's plane (half
// when grazing it, all when facing it), so reflected silhouettes fade over the
// lobe instead of stepping; blocked shares take the hit's radiance. Root word 1:
// output (storage): rgb blocked radiance (premultiplied), a the blocked share (-1
// where nothing was traced). RT.Rough.X/Y then widen it (rtRoughFilterMain).
static const uint RT_ROUGH_RAYS=4;
// Rough rays' hit-surface roughness floor and radiance ceiling (exposed luminance;
// 8 is about five and a half stops over mid-grey before the display curve).
static const float RT_ROUGH_HIT_ROUGHNESS=0.7,RT_ROUGH_CLAMP=8;
[shader("compute")][numthreads(8,8,1)]
void rtRoughMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 p=min(int2(tid.xy)*2,int2(f.extent.xy)-1);float4 result=float4(0,0,0,-1);
    bool covered;float3 world=gbufferWorld(f,p,covered);float roughness=gbufferMaterial(f,p).x;
    // Metals only: their specular lobe is all of their light, while a dielectric's is a
    // few percent whose probe/sky fallback reads fine (Showcase: tracing rough floors
    // cost 0.37 ms and made the shadow, bottle and pedestal regions worse).
    [branch]if(covered && rtEnabled(l) && roughness>=0.8*l.rtParams[0].x && roughness<l.rtParams[1].w && gbufferMaterial(f,p).y>=0.5){
        float3 n=normalize(gbufferNormal(f,p)),v=normalize(f.camera.xyz-world);
        float distance=length(world-f.camera.xyz),alpha=roughness*roughness,spread=2*l.rtParams[0].z;
        float3 d=rtDominant(n,v,alpha),origin=world+n*(l.rtParams[1].x+l.rtParams[1].y*distance);
        float3 t1=normalize(cross(abs(d.y)<0.99?float3(0,1,0):float3(1,0,0),d)),t2=cross(d,t1);
        float cone=min(1.3*alpha,1.2),sub=cone/3;uint phase=(tid.x&1)+2*(tid.y&1);
        float radius=tan(cone*sqrt((float(phase)+0.5)/4)*0.9);
        float3 blocked=0;float share=0,valid=0;
        // The linear exposure multiplier readers before this frame's metering decode with
        // (look.slangh exposureState).
        float exposure=exposureScale(f,exposureState(l).y);
        for(uint i=0;i<RT_ROUGH_RAYS;i++){
            float phi=(float(i)+float(phase)*0.25)*(2*PI/RT_ROUGH_RAYS);
            float3 dir=normalize(d+radius*(cos(phi)*t1+sin(phi)*t2));
            // Directions below the surface are its own masking (the split-sum
            // albedo's), not the scene's.
            if(dot(dir,n)<0.01)continue;
            valid+=1;RtHit hit;
            if(rtTrace(f,l,origin,dir,l.rtParams[0].y,hit,RT_OPAQUE|RT_MASKED,tan(sub))){
                float width=spread*(distance+hit.distance)+2*tan(sub)*hit.distance;
                float3 position,geometric;Surface s=rtSurface(l,hit,dir,width,position,geometric);
                s.ao=min(s.ao,rtScreenVisibility(f,l,position,geometric));
                // Rough lobes carry low-frequency light only: hits shade as rough
                // surfaces (no sun or light highlights, which a few rays would
                // under-sample), and a ray's radiance stops at RT_ROUGH_CLAMP times
                // the exposed mid-grey, so a small emitter one ray finds never pops.
                s.roughness=max(s.roughness,RT_ROUGH_HIT_ROUGHNESS);
                float3 radiance=rtShadeHit(f,l,s,position,geometric,-dir);
                float exposed=luminance(radiance)*exposure;
                if(exposed>RT_ROUGH_CLAMP)radiance*=RT_ROUGH_CLAMP/exposed;
                float c=0.5+0.5*saturate(asin(saturate(dot(-dir,geometric)))/sub);
                blocked+=radiance*c;share+=c;
            }
        }
        result=valid>0?float4(min(blocked/valid,float3(60000)),share/valid):float4(0);
    }
    images[NonUniformResourceIndex(root.materials)][tid.xy]=result;
}
// One axis of the rough result's spatial filter (half resolution). A wide lobe's
// occlusion is low frequency, while sixteen directions per 2x2 block alias on small
// bright hits, which flicker as the sub-pixel jitter moves each texel's surface
// point: nine taps (four texels each side, every ring phase twice) average the
// blocked radiance and share of the texels on the same surface (tangent-plane
// distance, normal similarity), about 300 rays per output pixel after both axes and
// rtRoughAt, still deterministic. Untraced texels (-1) neither contribute nor
// change. Root words: 1 input (sampled), 2 output (storage), 3 axis (0 x, 1 y).
[shader("compute")][numthreads(8,8,1)]
void rtRoughFilterMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;int2 q=int2(tid.xy);
    float4 c=pixel(root.materials,q);
    [branch]if(c.a>=0){
        int2 limit=int2(f.extent.xy)-1;bool covered;float3 world=gbufferWorld(f,min(q*2,limit),covered);
        float3 n=gbufferNormal(f,min(q*2,limit));float tolerance=0.02+0.02*length(world-f.camera.xyz);
        int2 axis=root.entity==0?int2(1,0):int2(0,1);float4 sum=c;float total=1;
        for(int k=-4;k<=4;k++){
            int2 t=q+axis*k;if(k==0 || any(t<0) || any(t>=int2(half)))continue;
            float4 r=pixel(root.materials,t);if(r.a<0)continue;
            bool tapCovered;float3 tapWorld=gbufferWorld(f,min(t*2,limit),tapCovered);if(!tapCovered)continue;
            float w=exp(-0.125*float(k*k)-abs(dot(n,tapWorld-world))/tolerance)*pow(saturate(dot(n,gbufferNormal(f,min(t*2,limit)))),8);
            sum+=r*w;total+=w;
        }
        c=sum/total;
    }
    images[NonUniformResourceIndex(root.material)][q]=c;
}
// The rough lobes' occlusion at full-resolution pixel `p` (`image`: the filtered
// RT.Rough result): the 2x2 half-resolution results around it, equally weighted so
// their four ring phases form the whole quadrature, rejecting other surfaces by
// depth and untraced texels (zero where none is traced).
float4 rtRoughAt(Frame f,uint image,int2 p){
    int2 half=int2((f.extent.xy+1)/2),b=int2(floor((float2(p)+0.5)*0.5-0.5));
    float depth=pixel(f.extra.x,p).x;float4 sum=0,nearest=0;float total=0,best=1e30;
    for(uint k=0;k<4;k++){
        int2 q=clamp(b+int2(k&1,k>>1),int2(0),half-1);float4 r=pixel(image,q);if(r.a<0)continue;
        float dz=abs(pixel(f.extra.x,q*2).x/max(depth,1e-7)-1);
        float w=exp(-dz*40);sum+=r*w;total+=w;if(dz<best){best=dz;nearest=r;}
    }
    return total>1e-3?sum/total:nearest;
}
// One axis of the reflection blur. Root words: 1 input (sampled), 2 output
// (storage), 3 axis (0 x; 1 y, which also composites the traced reflections and
// the rough lobes' occlusion, word 4, over screen-space ones: rgb and a combined
// confidence, as screenReflection returns).
// Nine taps at half-sigma steps (two sigmas each side), weighted by the Gaussian,
// the tap's distance from the pixel's tangent plane, normal similarity and the
// similarity of the reflected hit distances, so reflections never blur across the
// reflector's silhouettes and creases, nor across reflected ones (an object over
// the sky or a far wall behind it). Both axes carry each pixel's own hit distance.
[shader("compute")][numthreads(8,8,1)]
void rtFilterMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=f.extent.xy))return;int2 p=int2(tid.xy);
    float4 c=pixel(root.materials,p);float3 sum=c.rgb;float total=1;
    bool covered;float3 world=gbufferWorld(f,p,covered);float distance=length(world-f.camera.xyz);float roughness=gbufferMaterial(f,p).x;
    float lobe=rtSigma(l,roughness,distance,c.a),sigma=min(lobe,l.rtParams[0].w);
    [branch]if(sigma>0.3){
        float3 n=gbufferNormal(f,p);
        float tolerance=0.01+0.02*distance,reach=0.05+0.1*c.a;float step=max(sigma*0.5,1);
        int2 axis=root.entity==0?int2(1,0):int2(0,1);
        for(int k=-4;k<=4;k++){
            if(k==0)continue;
            float offset=round(float(k)*step);int2 q=p+axis*int(offset);
            if(any(q<0)||any(q>=int2(f.extent.xy)))continue;
            float4 t=pixel(root.materials,q);if(t.a<0)continue;
            bool tapCovered;float3 tapWorld=gbufferWorld(f,q,tapCovered);if(!tapCovered)continue;
            float plane=abs(dot(n,tapWorld-world));
            float w=exp(-0.5*offset*offset/(sigma*sigma))*exp(-plane/tolerance-abs(t.a-c.a)/reach)*pow(saturate(dot(n,gbufferNormal(f,q))),16);
            sum+=t.rgb*w;total+=w;
        }
    }
    float4 o=float4(sum/total,c.a);
    if(root.entity!=0){
        // Traced reflections fade out towards the roughness cut and where the lobe's
        // blur outgrows the filter (probe and environment reflections take over).
        float confidence=c.a<0?0:(1-saturate((roughness/l.rtParams[0].x-0.8)/0.2))*saturate(2-lobe/max(l.rtParams[0].w,1e-3))*l.rtParams[1].z;
        // Rough lobes (root word 4: RT.Rough's result): the share local geometry
        // blocks, rising as traced mirrors fade and fading out towards the rough
        // cut; the unblocked share keeps the probe and sky reflections ibl falls back to.
        float4 rough=rtRoughAt(f,root.cascade,p);
        float occluded=saturate(rough.a)*saturate((roughness/l.rtParams[0].x-0.8)/0.2)*(1-saturate((roughness/max(l.rtParams[1].w,1e-3)-0.8)/0.2))*l.rtParams[1].z;
        float traced=confidence+occluded*(1-confidence);
        float3 tracedColour=traced>1e-4?(o.rgb*confidence+(rough.a>1e-4?rough.rgb/rough.a:float3(0))*occluded*(1-confidence))/traced:float3(0);
        float4 screen=screenReflection(f,l,p);float a=traced+screen.a*(1-traced);
        o=float4(a>1e-4?(tracedColour*traced+screen.rgb*screen.a*(1-traced))/a:float3(0),a);
    }
    images[NonUniformResourceIndex(root.material)][p]=o;
}
