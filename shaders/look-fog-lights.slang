// Per-pixel local-light scattering in fog (look/volumetric.rs `Fog.Lights`): each pixel's
// view ray, up to its opaque surface (or past the background), integrates the light every
// local light in its 16x16 tile scatters towards the camera, at the pixel's own resolution
// rather than the froxels', so beams, cone edges and shadow shafts stay sharp. applyFog
// composites the result (look.slangh fogLightsAt); the froxels keep the sun, ambient
// light, emission and fog volumes, and their transmittance attenuates every sample here.
//
// Each light's segment is clipped analytically to its range sphere and, for spots, the
// outer cone, or for rects and discs the half-space in front of them (and a rect's barn
// doors), then integrated by equiangular sampling (Kulla and Fajardo 2012): samples
// spaced evenly in the angle the ray subtends at the light, which cancels the
// inverse-square peak, so fixed midpoints (no jitter, no noise, no history) converge
// fast. The angles fall in fixed bins, so the cost follows how much of the ray's view a
// light fills, and every term changes continuously across pixels: each sample's shadow
// filters over the stretch of ray it stands for (look-shadow.slangh shadowFog `smooth`).
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "look-fog.slangh"
// Clips [t0, t1] of the ray `o` + t `d` to the forward nappe of a cone (apex `apex`, unit
// axis `a`, half-angle cosine `c` > 0). The nappe is convex, so the result is one
// interval (empty: t1 <= t0). The quadratic's roots split the segment into at most
// three pieces; each piece lies wholly inside or outside, which its midpoint tells.
void clipCone(float3 o,float3 d,float3 apex,float3 a,float c,inout float t0,inout float t1){
    float3 w=o-apex;float da=dot(d,a),wa=dot(w,a),c2=c*c;
    float A=da*da-c2,B=wa*da-c2*dot(w,d),C=wa*wa-c2*dot(w,w);
    float r0=t0,r1=t0;
    if(abs(A)>1e-7){
        float disc=B*B-A*C;
        if(disc>=0){float s=sqrt(disc),x=(-B-s)/A,y=(-B+s)/A;r0=clamp(min(x,y),t0,t1);r1=clamp(max(x,y),t0,t1);}
    }else if(abs(B)>1e-9)r0=r1=clamp(-C/(2*B),t0,t1);
    float lo=t1,hi=t0;
    [unroll]for(uint k=0;k<3;k++){
        float s=k==0?t0:k==1?r0:r1,e=k==0?r0:k==1?r1:t1;
        if(e<=s)continue;
        float3 v=w+d*(0.5*(s+e));
        if(dot(v,a)>=c*length(v)){lo=min(lo,s);hi=max(hi,e);}
    }
    t0=lo;t1=hi;
}
// Clips [t0, t1] to where A + B t <= 0.
void clipBelow(float A,float B,inout float t0,inout float t1){
    if(abs(B)<1e-9){if(A>0)t1=t0;return;}
    float t=-A/B;if(B>0)t1=min(t1,t);else t0=max(t0,t);
}
// Clips [t0, t1] of the ray `o` + t `d` to the half-space in front of a plane through `p`
// facing `n`.
void clipFront(float3 o,float3 d,float3 p,float3 n,inout float t0,inout float t1){clipBelow(-dot(o-p,n),-dot(d,n),t0,t1);}
// The span along one rect axis (offset p0 + p1 t of the ray from the centre, height z0 +
// z1 t above the plane) that sees part of the rect past its barn doors
// (look-lights.slangh barnDoorSpan; half extent `a`, tips `door.x` out and `door.y` up):
// up to the tips' height, between the planes from the rect's edges through the tips
// (|p| <= a + door.x z / door.y; the whole rect shows there, none past them), and beyond
// it between those from the opposite edges through the tips (|p| <= (2a + door.x) z /
// door.y - a; light fades out towards them). The two wedges' intervals merge into their
// hull (one interval along any ray that matters).
void clipBarnDoors(float p0,float p1,float z0,float z1,float a,float2 door,inout float t0,inout float t1){
    float n0=t0,n1=t1,f0=t0,f1=t1,k=door.x/door.y,q=(2*a+door.x)/door.y;
    clipBelow(p0-k*z0-a,p1-k*z1,n0,n1);clipBelow(-p0-k*z0-a,-p1-k*z1,n0,n1);
    clipBelow(p0-q*z0+a,p1-q*z1,f0,f1);clipBelow(-p0-q*z0+a,-p1-q*z1,f0,f1);
    bool near=n1>n0,far=f1>f0;
    t0=near&&far?min(n0,f0):near?n0:f0;t1=near&&far?max(n1,f1):near?n1:far?f1:t0;
}
// Light local light `index` (`light`) scatters towards the camera along the ray `o` + t
// `d`, t in [0, `end`] through render uv `uv` (radiance, transmittance applied), with
// `samples` angular bins per half circle; `cone` is a pixel's angular radius, the narrowest shadow
// filter. Accumulates the samples' luminance-weighted view depth into `centroid`.
float3 fogLightRay(Frame f,Look l,Light light,uint index,float3 o,float3 d,float end,float2 uv,uint samples,float cone,inout float2 centroid){
    float3 centre=light.positionRange.xyz;float range=light.positionRange.w;
    float3 w=o-centre;float b=-dot(w,d),h2=max(dot(w,w)-b*b,0.);
    if(h2>=range*range)return 0;
    float chord=sqrt(range*range-h2),t0=max(b-chord,0.),t1=min(b+chord,end);
    uint kind=lightKind(light);LightShape shape=fogLightShape(f,light,index);
    if(kind==LIGHT_SPOT&&light.directionOuter.w>0.01)clipCone(o,d,centre,light.directionOuter.xyz,light.directionOuter.w,t0,t1);
    else if(kind>=LIGHT_RECT){
        float3 n=light.directionOuter.xyz;clipFront(o,d,centre,n,t0,t1);
        if(kind==LIGHT_RECT&&shape.rect.w>0){
            float3 x=shape.tangentLength.xyz,y=cross(x,n);float z0=dot(w,n),z1=dot(d,n);float2 extent=max(shape.rect.xy,0.001);
            clipBarnDoors(dot(w,x),dot(d,x),z0,z1,extent.x,shape.rect.zw,t0,t1);
            clipBarnDoors(dot(w,y),dot(d,y),z0,z1,extent.y,shape.rect.zw,t0,t1);
        }
    }
    if(t1<=t0)return 0;
    // Equiangular about the closest approach b, at distance he (at least the source's
    // near-field radius, inside which fogLocalLight's light stops rising): the angles
    // the ray subtends there, [a0, a1] within (-pi/2, pi/2), fall in fixed bins of pi /
    // `samples`, each sampled once at the middle of its covered part and weighted by
    // that part's width. A bin entering or leaving at either end does so with zero
    // weight, so the integral stays continuous across pixels while the cost follows the
    // angle the light fills: rays passing close by get `samples`, distant ones a few.
    float nearField=fogNearField(light,shape),he=max(sqrt(h2),sqrt(nearField));
    float a0=atan2(t0-b,he),a1=atan2(t1-b,he),bin=PI/float(samples);
    float g=l.fog.w,forward=dot(d,f.forward.xyz);
    float3 sum=0;float lo=t0;
    uint first=uint(floor((a0+0.5*PI)/bin)),last=min(uint(ceil((a1+0.5*PI)/bin)),samples);
    [loop]for(uint k=first;k<last;k++){
        float u0=max(a0,float(k)*bin-0.5*PI),u1=min(a1,float(k+1)*bin-0.5*PI);
        if(u1<=u0)continue;
        // The stretch [lo, hi] of ray this sample stands for; its shadow filters over half.
        float hi=k+1<last?b+he*tan(u1):t1,t=b+he*tan(0.5*(u0+u1));
        float footprint=max(0.5*(hi-lo),cone*t);lo=hi;
        float3 p=o+d*t;
        FogMedium m=fogMedium(f,l,p);
        if(all(m.scattering<=0))continue;
        float3 value=fogLocalLight(l,light,index,shape,nearField,p,d,g,footprint,true);
        if(all(value<=0))continue;
        // The estimator's weight (he^2 + (t-b)^2) (u1 - u0) / he: the inverse square
        // fogLocalLight divides by cancels where he is the ray's own distance.
        value*=m.scattering*fogTransmittance(f,l,uv,t,t*forward)*((he*he+(t-b)*(t-b))*(u1-u0)/he);
        sum+=value;
        float y=luminance(value);centroid+=float2(y*t*forward,y);
    }
    return sum;
}
// A sphere bounding where local light `light` can light fog (centre, radius): a spot's
// cone (through its apex and its base rim, for half-angles to 60 degrees), else its range.
float4 fogLightBounds(Light light){
    float4 s=light.positionRange;float c=light.directionOuter.w;
    if(lightKind(light)!=LIGHT_SPOT||c<0.5)return s;
    float r=s.w/(2*c);return float4(s.xyz+light.directionOuter.xyz*r,r);
}
// Entries of the group's tile list (bit i: entry i) whose light scatters in fog
// (parameters.w) and whose bounds (fogLightBounds) meet the group's frustum in front of
// its farthest opaque surface (fogGroupFar, f32 bits). Walking set bits keeps the
// summation order fixed (deterministic).
groupshared uint fogTileLights[TILE_LIGHTS/32];
groupshared uint fogGroupFar;
// The tile-list offset of the 16x16 light tile holding render pixel `p`.
uint fogTile(Frame f,float2 p){uint tilesX=(f.extent.x+15)/16;return (uint(p.y)/16*tilesX+uint(p.x)/16)*TILE_STRIDE;}
// Marks fogTileLights for the group's render rectangle [lo, hi) (inside light tile
// `tile`) in front of `far`: every thread of the group calls it between barriers.
void fogCullTileLights(Frame f,uint tile,float2 lo,float2 hi,float far,uint thread){
    // The frustum: planes through the camera and the rectangle's corner rays, normals inwards.
    float3 r0=viewRay(f,lo),r1=viewRay(f,float2(hi.x,lo.y)),r2=viewRay(f,hi),r3=viewRay(f,float2(lo.x,hi.y)),mid=r0+r2;
    float3 n0=normalize(cross(r0,r1)),n1=normalize(cross(r1,r2)),n2=normalize(cross(r2,r3)),n3=normalize(cross(r3,r0));
    n0*=sign(dot(n0,mid));n1*=sign(dot(n1,mid));n2*=sign(dot(n2,mid));n3*=sign(dot(n3,mid));
    uint count=buffers[NonUniformResourceIndex(f.lighting.z)].Load(tile*4);
    [loop]for(uint i=thread;i<count;i+=64){
        Light light=loadLight(f.lighting.w,buffers[NonUniformResourceIndex(f.lighting.z)].Load((tile+1+i)*4));
        float4 s=fogLightBounds(light);float3 c=s.xyz-f.camera.xyz;
        if(light.parameters.w>0&&dot(n0,c)>=-s.w&&dot(n1,c)>=-s.w&&dot(n2,c)>=-s.w&&dot(n3,c)>=-s.w&&length(c)-s.w<=far)
            InterlockedOr(fogTileLights[i/32],1u<<(i%32));
    }
}
// The view ray through render pixel `p`'s centre and the distance to its opaque surface
// (1e30 over the background).
float3 fogPixelRay(Frame f,int2 p,out float end){
    bool covered;float3 world=gbufferWorld(f,p,covered);end=covered?length(world-f.camera.xyz):1e30;
    return viewRay(f,float2(p)+0.5);
}
// The scatter along render pixel `p`'s ray `d` (to `end`) from the lights of light tile
// `tile` (those fogTileLights marks with `culled`, else every one with volumetric
// scattering, each clipped to its reach along the ray), and its mean view depth (look.slangh
// fogLightsAt); `scale`: render pixels per texel side, which widens the narrowest shadow
// filter. Lights sum in tile-list order (deterministic).
float4 fogLightsPixel(Frame f,Look l,int2 p,float3 d,float end,uint tile,uint scale,bool culled=true){
    float2 uv=(float2(p)+0.5)/float2(f.extent.xy);
    // A render pixel's angular radius (half its neighbour's ray offset), widened to the texel.
    float cone=0.5*float(scale)*length(viewRay(f,float2(p)+float2(1.5,0.5))-d);
    uint samples=uint(l.fogLightsParams.x);
    float3 sum=0;float2 centroid=0;
    if(culled){
        [loop]for(uint word=0;word<TILE_LIGHTS/32;word++){
            uint bits=fogTileLights[word];
            [loop]while(bits!=0){
                uint i=word*32+firstbitlow(bits);bits&=bits-1;
                uint index=buffers[NonUniformResourceIndex(f.lighting.z)].Load((tile+1+i)*4);
                sum+=fogLightRay(f,l,loadLight(f.lighting.w,index),index,f.camera.xyz,d,end,uv,samples,cone,centroid);
            }
        }
    }else{
        uint count=buffers[NonUniformResourceIndex(f.lighting.z)].Load(tile*4);
        [loop]for(uint i=0;i<count;i++){
            uint index=buffers[NonUniformResourceIndex(f.lighting.z)].Load((tile+1+i)*4);
            Light light=loadLight(f.lighting.w,index);
            if(light.parameters.w>0)sum+=fogLightRay(f,l,light,index,f.camera.xyz,d,end,uv,samples,cone,centroid);
        }
    }
    float4 result=float4(min(sum,60000),centroid.y>0?centroid.x/centroid.y:0);
    return any(isnan(result))||any(isinf(result))?0:result;
}
// One thread per fog-lights texel (look.fogLightsInfo.z render pixels a side, holding
// render pixel z*q's ray; one group stays inside one 16x16 light tile): every light of
// its tile that can reach the group's rays (look-lights.slangh; parameters.w), up to the
// opaque surface or, over the background, through the lights' whole reach. At half
// resolution fogLightsResolveMain then makes the render-size image the composite reads
// (this pass resets its edge list: no groups, no entries).
[shader("compute")][numthreads(8,8,1)]
void fogLightsMain(uint3 tid:SV_DispatchThreadID,uint3 group:SV_GroupID,uint thread:SV_GroupIndex){
    Frame f=frame();Look l=look(f);uint scale=l.fogLightsInfo.z;
    if(thread<TILE_LIGHTS/32)fogTileLights[thread]=0;
    if(thread==0)fogGroupFar=0;
    if(scale>1&&all(tid==0))buffers[NonUniformResourceIndex(l.fogLightsResolve.z)].Store4(0,uint4(0,1,1,0));
    int2 p=int2(tid.xy*scale);bool inside=scale!=0&&all(p<int2(f.extent.xy));p=min(p,int2(f.extent.xy)-1);
    float end;float3 d=fogPixelRay(f,p,end);
    GroupMemoryBarrierWithGroupSync();
    if(inside)InterlockedMax(fogGroupFar,asuint(end));
    float2 lo=float2(group.xy*8*scale),hi=min(lo+float(8*scale),float2(f.extent.xy));uint tile=fogTile(f,lo);
    GroupMemoryBarrierWithGroupSync();
    if(scale!=0)fogCullTileLights(f,tile,lo,hi,asfloat(fogGroupFar),thread);
    GroupMemoryBarrierWithGroupSync();
    if(!inside)return;
    images[NonUniformResourceIndex(l.fogLightsInfo.y)][tid.xy]=fogLightsPixel(f,l,p,d,end,tile,scale);
}
// Relative luminance spread between the half-resolution texels a render pixel blends
// above which it integrates its own ray (fogLightsResolveMain), and the exposed spread
// below which it never does (steps darker than about two 8-bit codes near black).
static const float FOG_EDGE_CONTRAST=0.04,FOG_EDGE_FLOOR=0.0005;
// Relative depth difference within which a half-resolution texel lies on a render
// pixel's own surface.
static const float FOG_EDGE_DEPTH=0.05;
// The half-resolution scatter (look.fogLightsResolve.x; texel q holds render pixel 2q's
// ray) at render pixel `p`: even pixels read their own texel exactly; the others blend
// the two or four around them bilinearly, each weighted by how close its opaque depth is
// to the pixel's (FOG_EDGE_DEPTH: the same surface). `edge`: the blend cannot stand for
// the pixel, because the texels on its surface disagree (a beam's edge, a spot's apex, a
// shaft's border: their luminance spread passes FOG_EDGE_CONTRAST and FOG_EDGE_FLOOR),
// or none lies on its surface (a thin occluder) and they carry visible light.
float4 fogLightsUpsample(Frame f,Look l,int2 p,out bool edge){
    uint image=l.fogLightsResolve.x;int2 q=p/2,t=p&1;edge=false;
    if(all(t==0))return pixel(image,q);
    int2 last=int2((f.extent.xy+1)/2)-1;float depth=gbufferDepth(f,p);
    float4 sum=0;float total=0,lo=1e30,hi=0,peak=0;
    [unroll]for(uint k=0;k<4;k++){
        int2 o=int2(k&1,k>>1);
        if(any(o>t))continue;
        int2 at=min(q+o,last);float z=gbufferDepth(f,at*2),rel=abs(z-depth)/max(max(z,depth),1e-30);
        float4 s=pixel(image,at);float y=luminance(s.rgb),w=1/(rel+0.01);
        sum+=s*w;total+=w;peak=max(peak,y);
        if(rel<=FOG_EDGE_DEPTH){lo=min(lo,y);hi=max(hi,y);}
    }
    float exposure=hdrExposure(f,l);
    edge=hi<lo?peak*exposure>4*FOG_EDGE_FLOOR:hi-lo>FOG_EDGE_CONTRAST*hi&&(hi-lo)*exposure>FOG_EDGE_FLOOR;
    return total>0?sum/total:0;
}
// The edge list (look.fogLightsResolve.z, `capacity` entries in w; look/volumetric.rs
// FogEdges): bytes 0..12 the edge pass's dispatch groups, 12 the count appended, then one
// u32 per pixel (x | y << 16). fogLightsMain resets the header each frame.
static const uint FOG_EDGE_HEADER=16;
// One thread per render pixel on half-resolution frames: the render-size scatter the
// composite reads (look.fogLightsResolve.y). Where fogLightsUpsample's blend stands for
// the pixel it writes the blend; edge pixels go to the edge list for fogLightsEdgesMain,
// which integrates their own rays at full resolution (past the list's capacity they keep
// the blend), so hard edges stay crisp at 1:1 while smooth beams keep half resolution's
// cost.
[shader("compute")][numthreads(8,8,1)]
void fogLightsResolveMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);
    int2 p=int2(tid.xy);if(any(p>=int2(f.extent.xy)))return;
    bool edge;float4 blend=fogLightsUpsample(f,l,p,edge);
    if(edge){
        uint list=l.fogLightsResolve.z,slot;
        buffers[NonUniformResourceIndex(list)].InterlockedAdd(12,1,slot);
        if(slot<l.fogLightsResolve.w){
            buffers[NonUniformResourceIndex(list)].Store(FOG_EDGE_HEADER+slot*4,uint(p.x)|(uint(p.y)<<16));
            // One more group of 64 each time a group's first entry lands.
            if(slot%64==0){uint unused;buffers[NonUniformResourceIndex(list)].InterlockedAdd(0,1,unused);}
            return;
        }
    }
    images[NonUniformResourceIndex(l.fogLightsResolve.y)][p]=blend;
}
// One thread per edge-list entry (an indirect dispatch of the groups fogLightsResolveMain
// counted): the pixel's own ray against every light of its tile, written over the render-
// size scatter. Full groups whatever the pixels' tiles, so the cost follows the number of
// edges; each light is clipped to its reach along the ray (fogLightRay).
[shader("compute")][numthreads(64,1,1)]
void fogLightsEdgesMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint list=l.fogLightsResolve.z;
    uint count=min(buffers[NonUniformResourceIndex(list)].Load(12),l.fogLightsResolve.w);
    if(tid.x>=count)return;
    uint entry=buffers[NonUniformResourceIndex(list)].Load(FOG_EDGE_HEADER+tid.x*4);
    int2 p=int2(entry&0xffff,entry>>16);
    float end;float3 d=fogPixelRay(f,p,end);
    images[NonUniformResourceIndex(l.fogLightsResolve.y)][p]=fogLightsPixel(f,l,p,d,end,fogTile(f,float2(p)),1,false);
}
