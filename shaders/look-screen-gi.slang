// Screen probe GI passes (look/screen_gi.rs): the Hi-Z pyramid, the probe trace and the
// half-resolution gather lighting reads (look-screen-gi.slangh). Every ray direction
// and probe position is fixed by the frame (no jitter): stable without accumulation.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "look-screen.slangh"
#include "look-screen-gi.slangh"
// Root word 2: Hi-Z level k (1..), the closest of the four level k-1 cells it covers.
[shader("compute")][numthreads(8,8,1)]
void hizMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint k=root.material;uint2 size=hizSize(f,k);if(any(tid.xy>=size))return;
    uint2 below=hizSize(f,k-1);uint origin=hizOrigin(f,k-1);float d=0;
    for(uint i=0;i<4;i++){
        uint2 q=min(tid.xy*2+uint2(i&1,i>>1),below-1);
        // Level k-1 through the storage view (written by the previous dispatch).
        d=max(d,k==1?pixel(f.extra.x,int2(q)).x:images[NonUniformResourceIndex(l.screenInfo[0].y)][uint2(origin,0)+q].x);
    }
    images[NonUniformResourceIndex(l.screenInfo[0].y)][uint2(hizOrigin(f,k),0)+tid.xy]=float4(d,0,0,0);
}
// Root word 2: level (1..) of the promoted-emission pyramid: this frame's G-buffer
// emission brighter than the emitter threshold (screenParams[1].x), as the reflection
// pyramid's layout. Level 1 averages 2x2 render pixels; later levels their previous.
[shader("compute")][numthreads(8,8,1)]
void emissionPyramidMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint level=root.material;uint2 size=ssrLevelSize(f,level),p=tid.xy;if(any(p>=size))return;
    uint image=l.screenInfo[3].y;float3 c=0;
    if(level==1){
        for(uint k=0;k<4;k++){int2 q=min(int2(p*2+uint2(k&1,k>>1)),int2(f.extent.xy)-1);float3 e=gbufferEmissive(f,q);if(luminance(e)>l.screenParams[1].x)c+=e;}
    }else{
        uint2 o=ssrLevelOrigin(f,level-1),s=ssrLevelSize(f,level-1);
        for(uint k=0;k<4;k++)c+=images[NonUniformResourceIndex(image)][o+min(p*2+uint2(k&1,k>>1),s-1)].rgb;
    }
    images[NonUniformResourceIndex(image)][ssrLevelOrigin(f,level)+p]=float4(min(c*0.25,float3(60000)),1);
}
groupshared float4 probeRadiance[SCREEN_PROBE_RAYS];
groupshared float3 probeDirections[SCREEN_PROBE_RAYS];
// What a probe ray that leaves the screen (or finds nothing within its reach) sees:
// the world probes' parallax-corrected radiance (their cache of what lies off screen,
// occluded as they see it), the sky where they do not reach.
float3 probeMiss(Frame f,Look l,float3 origin,float3 n,float3 d){
    float3 sky=skyLight(l)*(physicalSky(l)?skyRadiance(f,l,d,false):envTex(f.extra.w,environmentUv(d)).rgb)*horizonSky(l,origin);
    if(!giEnabled(l))return sky;
    float3 irradiance,radiance;float coverage=giPoolLobes(l,l.giImages.y,origin,n,d,float3(0,0,0),0,true,irradiance,radiance,false)*giWeight(l);
    return lerp(sky,radiance,coverage);
}
// One group per probe (tile), one lane per direction: place the probe at the tile's
// centre pixel (else the first covered quarter point), trace its hemisphere, then
// filter the radiance into the probe's levels and irradiance (look-screen-gi.slangh).
[shader("compute")][numthreads(64,1,1)]
void screenProbeMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();Look l=look(f);uint tile=screenProbeTileSize(l);uint2 probe=group.xy;
    int2 base=int2(probe*tile),last=int2(f.extent.xy)-1;
    int2 p=min(base+int2(tile/2),last);float depth=gbufferDepth(f,p);
    for(uint c=0;c<4&&depth<=0;c++){
        int2 q=min(base+int2((c&1)!=0?tile*3/4:tile/4,(c&2)!=0?tile*3/4:tile/4),last);float d=gbufferDepth(f,q);
        if(d>0){p=q;depth=d;}
    }
    if(depth<=0){if(lane==0)images[NonUniformResourceIndex(l.screenInfo[1].y)][probe]=0;return;}
    float3 world=worldAtDepth(f,float2(p)+0.5,depth),n=normalize(gbufferNormal(f,p)),v=normalize(f.camera.xyz-world);
    // Back faces (thin double-sided surfaces) gather on the side the camera sees.
    if(dot(n,v)<0)n=-n;
    float3 t,b;probeFrame(n,t,b);
    float2 uv=(float2(lane%8,lane/8)+0.5)/8;float3 local=hemiOctDecode(uv),dir=t*local.x+b*local.y+n*local.z;
    // Render pixels per radian at this depth, from two neighbouring pixel centres.
    float view=dot(world-f.camera.xyz,f.forward.xyz);
    float pixelAngle=length(worldAtDepth(f,float2(p)+float2(1.5,0.5),depth)-world)/max(view,1e-4);
    // Lifted off the surface by a few of its pixels (and a centimetre), so the trace's
    // first steps do not find the surface itself.
    float3 origin=world+n*max(0.01,view*pixelAngle*l.screenParams[0].w);
    float3 a,e;screenSegment(f,origin,dir,l.screenParams[0].z,a,e);
    float s=hizTrace(f,l,a,e-a,uint(l.screenParams[0].x),uint(l.screenParams[0].y),0.03);
    float3 radiance=0;float distance=l.screenParams[0].z;bool found=false;
    if(s>=0){
        float3 h=lerp(a,e,s);float3 hit=worldAtDepth(f,floor(h.xy)+0.5,hizLoad(f,l,0,int2(h.xy)));
        float4 q=mul(f.previousVp,float4(hit,1));float2 at=float2(q.x/q.w*0.5+0.5,0.5-q.y/q.w*0.5);
        if(q.w>0&&all(at>0)&&all(at<1)){
            // The ray's cone (its texel's solid angle) at the hit, in pixels, picks the
            // pyramid level: a cone-filtered radiance, not one aliased pixel.
            distance=length(hit-origin);float hitView=max(dot(hit-f.camera.xyz,f.forward.xyz),1e-3);
            float cone=2*sqrt(hemiOctSolidAngle(uv,1./8)/PI)*distance/(hitView*pixelAngle);
            // Emission promoted to emitter lights (look::EMITTER_RADIANCE) already
            // lights the scene directly: the same cone's share of it (this frame's
            // emission pyramid around the hit) comes off, or it would count twice.
            float level=log2(max(cone,2));
            radiance=max(pyramidBlurred(f,l.ssrImages.x,at,level)-pyramidBlurred(f,l.screenInfo[3].x,h.xy/float2(f.extent.xy),level),0);found=true;
            // Last frame's colour is what the camera saw: on glossy surfaces it holds
            // reflections of other things towards the camera, not what they send this
            // probe. As the visibility bitmask's bounce (look-ao.slang), a hit sends at
            // most what a diffuse surface of its albedo can: eight times the exposure's
            // mid-grey key times that albedo.
            float cap=8*0.18/exposureScale(f,exposureState(l).y);
            radiance=min(radiance,gbufferAlbedo(f,int2(h.xy)).rgb*cap);
        }
    }
    if(!found)radiance=probeMiss(f,l,origin,n,dir);
    radiance=any(isnan(radiance))?float3(0):min(radiance,60000);
    uint2 tileOrigin=probe*SCREEN_PROBE_TILE;uint atlas=l.screenInfo[0].w;
    probeRadiance[lane]=float4(radiance,hemiOctSolidAngle(uv,1./8));probeDirections[lane]=local;
    images[NonUniformResourceIndex(atlas)][tileOrigin+uint2(lane%8,lane/8)]=float4(radiance,distance);
    GroupMemoryBarrierWithGroupSync();
    {
        // GGX-prefiltered levels (perceptual roughness 0.4, 0.6, 0.8, 1; one output
        // texel per lane): each traced direction weighs by the lobe's D(h) n.l around
        // the texel's direction (the split sum's n = v = r), over the traced hemisphere
        // (the share of the lobe below the horizon takes the rest's mean).
        uint level=lane/16;uint2 c=uint2(lane%4,(lane/4)%4);float r=0.4+0.2*float(level),a2=r*r*r*r;
        float3 centre=hemiOctDecode((float2(c)+0.5)/4),sum=0;float weight=0;
        for(uint i=0;i<SCREEN_PROBE_RAYS;i++){
            float3 d=probeDirections[i];float nl=dot(centre,d);if(nl<=0)continue;
            float nh=dot(centre,normalize(centre+d)),t=nh*nh*(a2-1)+1,w=a2/(PI*t*t)*nl*probeRadiance[i].w;
            sum+=probeRadiance[i].rgb*w;weight+=w;
        }
        images[NonUniformResourceIndex(atlas)][tileOrigin+screenProbeLevelAt(level+1)+c]=float4(sum/max(weight,1e-12),0);
    }
    if(lane<16){
        // The irradiance: the cosine lobe around each texel's direction over the traced
        // hemisphere, normalized by the share of the lobe it holds.
        uint2 c=uint2(lane%4,lane/4);float3 normal=hemiOctDecode((float2(c)+0.5)/4),e=0;float cosines=0;
        for(uint i=0;i<SCREEN_PROBE_RAYS;i++){float w=max(dot(normal,probeDirections[i]),0)*probeRadiance[i].w;e+=probeRadiance[i].rgb*w;cosines+=w;}
        images[NonUniformResourceIndex(atlas)][tileOrigin+SCREEN_PROBE_IRRADIANCE+c]=float4(e*(PI/max(cosines,1e-6)),0);
    }else if(lane==16){
        images[NonUniformResourceIndex(l.screenInfo[1].y)][probe]=float4(world,asfloat(packNormal(n)));
    }
}
// Per half-resolution pixel (the full-resolution pixel 2p): screen-probe irradiance
// (rgb; w the share the probes cover) and pre-filtered radiance along the specular
// lobe's dominant direction (rgb). Lighting upsamples them depth-aware (screenLobes).
[shader("compute")][numthreads(8,8,1)]
void screenGatherMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 p=int2(tid.xy)*2;bool covered;float3 world=gbufferWorld(f,p,covered);float4 diffuse=0,specular=0;
    if(covered){
        float3 n=normalize(gbufferNormal(f,p)),v=normalize(f.camera.xyz-world);if(dot(n,v)<0)n=-n;
        float roughness=gbufferMaterial(f,p).x;float3 irradiance,radiance;
        float share=screenProbeLobes(f,l,p,world,n,v,specularDirection(n,v,roughness),roughness,irradiance,radiance);
        diffuse=float4(irradiance,share);specular=float4(radiance,0);
    }
    images[NonUniformResourceIndex(l.screenInfo[2].z)][tid.xy]=diffuse;
    images[NonUniformResourceIndex(l.screenInfo[2].w)][tid.xy]=specular;
}
