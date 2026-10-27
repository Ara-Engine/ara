// Ray-traced reflections of the hybrid High/Epic path (D29; renderer::rt), noise
// free: one deterministic ray per pixel along the GGX lobe's dominant direction,
// for every surface below the roughness cut, shaded from the lighting caches
// (look-rt.slangh). Roughness widens the result by a fixed depth/normal-aware
// Gaussian whose width follows the lobe's footprint at the hit, never by stochastic
// sampling or temporal accumulation.
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
// Root word 1: output (storage): rgb reflected radiance, a the blur sigma in render
// pixels (0 sharp or already filtered; -1 not traced). Render resolution.
[shader("compute")][numthreads(8,8,1)]
void rtReflectMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=f.extent.xy))return;int2 p=int2(tid.xy);
    float4 result=float4(0,0,0,-1);bool covered;float3 world=gbufferWorld(f,p,covered);float roughness=gbufferMaterial(f,p).x;
    [branch]if(covered && rtEnabled(l) && roughness<l.rtParams[0].x){
        float3 n=normalize(gbufferNormal(f,p)),v=normalize(f.camera.xyz-world);
        float distance=length(world-f.camera.xyz),alpha=roughness*roughness,spread=l.rtParams[0].z;
        // The GGX lobe's dominant direction (Lagarde & de Rousiers 2014), kept above
        // the surface where a normal map tilts it below.
        float3 d=normalize(lerp(n,reflect(-v,n),(1-alpha)*(sqrt(1-alpha)+alpha)));
        float below=dot(d,n);if(below<0.01)d=normalize(d+n*(0.01-below));
        // Biased along the normal by the depth reconstruction's and coarser raster
        // LODs' error, which grows with distance.
        float3 origin=world+n*(l.rtParams[1].x+l.rtParams[1].y*distance);
        RtHit hit;float3 radiance;float sigma=0;
        if(rtTrace(f,l,origin,d,l.rtParams[0].y,hit,RT_OPAQUE|RT_MASKED,spread)){
            // The ray cone: the pixel's footprint growing with the path plus the lobe.
            float width=spread*(distance+hit.distance)+2*alpha*hit.distance;
            float3 position,geometric;Surface s=rtSurface(l,hit,d,width,position,geometric);
            radiance=rtShadeHit(f,l,s,position,geometric,-d);
            // The reflected lobe's spread at the hit (a Gaussian sigma of about 0.7
            // alpha rad), as the camera sees it across the reflector: the blur sigma in
            // render pixels. Lobes within the G-buffer's roughness floor and geometric
            // specular-AA widening (alpha 0.0064) stay mirrors: that widening fights
            // highlight aliasing, not reflected detail, which the resolve antialiases.
            sigma=0.7*max(alpha-0.0064,0)*hit.distance/max((distance+hit.distance)*spread,1e-6);
        }else radiance=rtMiss(f,l,d,roughness);
        result=float4(min(radiance,float3(60000)),sigma);
    }
    images[NonUniformResourceIndex(root.materials)][p]=result;
}
// One axis of the reflection blur. Root words: 1 input (sampled), 2 output
// (storage), 3 axis (0 x; 1 y, which also composites the traced reflections over
// screen-space ones: rgb and a combined confidence, as screenReflection returns).
// Nine taps at half-sigma steps (two sigmas each side), weighted by the Gaussian,
// the tap's distance from the pixel's tangent plane and normal similarity, so
// reflections never blur across silhouettes or creases.
[shader("compute")][numthreads(8,8,1)]
void rtFilterMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(tid.xy>=f.extent.xy))return;int2 p=int2(tid.xy);
    float4 c=pixel(root.materials,p);float3 sum=c.rgb;float total=1;
    float sigma=min(c.a,l.rtParams[0].w);
    [branch]if(sigma>0.3){
        bool covered;float3 world=gbufferWorld(f,p,covered);float3 n=gbufferNormal(f,p);
        float tolerance=0.01+0.02*length(world-f.camera.xyz);float step=max(sigma*0.5,1);
        int2 axis=root.entity==0?int2(1,0):int2(0,1);
        for(int k=-4;k<=4;k++){
            if(k==0)continue;
            float offset=round(float(k)*step);int2 q=p+axis*int(offset);
            if(any(q<0)||any(q>=int2(f.extent.xy)))continue;
            float4 t=pixel(root.materials,q);if(t.a<0)continue;
            bool tapCovered;float3 tapWorld=gbufferWorld(f,q,tapCovered);if(!tapCovered)continue;
            float plane=abs(dot(n,tapWorld-world));
            float w=exp(-0.5*offset*offset/(sigma*sigma))*exp(-plane/tolerance)*pow(saturate(dot(n,gbufferNormal(f,q))),16);
            sum+=t.rgb*w;total+=w;
        }
    }
    float4 o=float4(sum/total,c.a);
    if(root.entity!=0){
        // Traced reflections fade out towards the roughness cut and where the lobe's
        // blur outgrows the filter (probe and environment reflections take over).
        float roughness=gbufferMaterial(f,p).x;
        float confidence=c.a<0?0:(1-saturate((roughness/l.rtParams[0].x-0.8)/0.2))*saturate(2-c.a/max(l.rtParams[0].w,1e-3))*l.rtParams[1].z;
        float4 screen=screenReflection(f,l,p);float a=confidence+screen.a*(1-confidence);
        o=float4(a>1e-4?(o.rgb*confidence+screen.rgb*screen.a*(1-confidence))/a:float3(0),a);
    }
    images[NonUniformResourceIndex(root.material)][p]=o;
}
