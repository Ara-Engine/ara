// Ground-truth-based ambient occlusion (look/passes.rs): an independent
// implementation of Jimenez et al. 2016 (GTAO) at half resolution, in world space
// from the G-buffer depth, then a depth-aware denoise. Per-pixel slice rotation
// uses interleaved gradient noise advancing each frame; temporal antialiasing
// integrates it, so the result is stable without stochastic rays.
#include "raster.slangh"
#include "look.slangh"
// Reversed-Z depth at full-resolution pixel `p` to a world position.
float ign(float2 p,uint frame){p+=5.588238*float(frame&63);return frac(52.9829189*frac(dot(p,float2(0.06711056,0.00583715))));}
// Root word 2: half-resolution output pixel count per row is f.extent.x/2 rounded up.
// Output: r ambient visibility, g distance to the camera (for the filters).
[shader("compute")][numthreads(8,8,1)]
void aoTraceMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 p=int2(tid.xy)*2;float depth=pixel(f.extra.x,p).x;
    if(depth<=0){images[NonUniformResourceIndex(l.aoImages.z)][tid.xy]=float4(1,1e9,0,1);return;}
    float2 centre=float2(p)+0.5;float3 P=worldAtDepth(f,centre,depth),N=normalize(gbufferNormal(f,p)),V=normalize(f.camera.xyz-P);
    float distance=length(f.camera.xyz-P);
    // Screen radius of the world radius at this distance.
    float3 side=normalize(cross(f.forward.xyz,abs(f.forward.y)<0.99?float3(0,1,0):float3(1,0,0)));
    float4 a=mul(f.vp,float4(P,1)),b=mul(f.vp,float4(P+side*l.ao.x,1));
    float radius=length((b.xy/b.w-a.xy/a.w)*0.5*float2(f.extent.xy));
    float noise=ign(float2(tid.xy),f.extent.w),jitter=ign(float2(tid.xy)+float2(37,17),f.extent.w);
    uint slices=uint(l.ao.y),steps=uint(l.ao.z);float visibility=0;
    if(radius<1){images[NonUniformResourceIndex(l.aoImages.z)][tid.xy]=float4(1,distance,0,1);return;}
    for(uint s=0;s<slices;s++){
        float phi=(float(s)+noise)/float(slices)*PI;float2 direction=float2(cos(phi),sin(phi));
        // The slice plane contains V and the world direction of `direction` on screen.
        float3 along=worldAtDepth(f,centre+direction*4,depth)-P;float3 sliceDirection=normalize(along-V*dot(along,V));
        float3 axis=normalize(cross(sliceDirection,V));float3 projected=N-axis*dot(N,axis);float length2=dot(projected,projected);
        if(length2<1e-6)continue;float projectedLength=sqrt(length2);projected/=projectedLength;
        float n=sign(dot(projected,sliceDirection))*acos(clamp(dot(projected,V),-1.,1.));
        float horizons[2]={-1,-1};
        [unroll]for(uint sideIndex=0;sideIndex<2;sideIndex++){
            float sign=sideIndex==0?1.:-1.;
            for(uint k=0;k<steps;k++){
                float t=(float(k)+jitter)/float(steps);float offset=max(t*t*radius,1+float(k));
                float2 q=centre+direction*sign*offset;if(any(q<0)||any(q>=float2(f.extent.xy)))break;
                float sampleDepth=pixel(f.extra.x,int2(q)).x;if(sampleDepth<=0)continue;
                float3 delta=worldAtDepth(f,floor(q)+0.5,sampleDepth)-P;float len=length(delta);
                float falloff=saturate(1-len*len/(l.ao.x*l.ao.x));float cosine=dot(delta/max(len,1e-5),V);
                // Distant samples and cutouts (gbufferOccluder, read only where the
                // sample would raise the horizon) raise no horizon.
                if(lerp(-1.,cosine,falloff)>horizons[sideIndex])
                    horizons[sideIndex]=max(horizons[sideIndex],lerp(-1.,cosine,falloff*gbufferOccluder(f,int2(q))));
            }
        }
        // The positive side runs along sliceDirection (angles grow towards it).
        float h0=n+max(-acos(horizons[1])-n,-PI/2),h1=n+min(acos(horizons[0])-n,PI/2);
        float arc=(-cos(2*h0-n)+cos(n)+2*h0*sin(n))+(-cos(2*h1-n)+cos(n)+2*h1*sin(n));
        visibility+=projectedLength*arc*0.25;
    }
    visibility=saturate(visibility/max(float(slices),1));
    images[NonUniformResourceIndex(l.aoImages.z)][tid.xy]=float4(visibility,distance,0,1);
}
// 4x4 depth-aware box around each half-resolution pixel (removes the slice noise
// pattern; TAA does the rest).
[shader("compute")][numthreads(8,8,1)]
void aoDenoiseMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    float2 centre=pixel(l.aoImages.x,int2(tid.xy)).rg;float sum=0,total=0;
    for(int y=-2;y<=1;y++)for(int x=-2;x<=1;x++){
        int2 q=clamp(int2(tid.xy)+int2(x,y),int2(0),int2(half)-1);float2 v=pixel(l.aoImages.x,q).rg;
        float w=exp(-abs(v.g-centre.g)/max(centre.g*0.02,0.05));sum+=v.r*w;total+=w;
    }
    images[NonUniformResourceIndex(l.aoImages.w)][tid.xy]=float4(sum/max(total,1e-5),centre.g,0,1);
}

// Visibility-bitmask ambient occlusion and near-field indirect light for the High/Epic
// tier (look/passes.rs `bitmask_ao`): an independent implementation of Therrien,
// Levesque and Gilet 2023 over GTAO's screen slices. Each sample hides only the slice
// sectors between its front and its back (the sample pushed `shadowParams[0].x` metres
// away from the camera), 32 sectors per slice in GTAO's cosine-weighted measure, so thin
// or distant foreground stops darkening what lies behind it (no horizon halos). The
// sectors each slice side hides receive the radiance, from last frame's colour (the
// reflection pyramid, reprojected), of the sample there that hides the most, where it
// faces the pixel: one bounce from the geometry within the radius (repeated frame to
// frame; one fetch per side, the denoise averages 64 of them), which lighting adds where the
// sectors hide probe and sky light (look-shading.slangh ibl). Slice rotations and step
// offsets follow a fixed 4x4 pattern that the denoise's 4x4 box averages exactly:
// nothing varies per frame. Output (half resolution): rgb bounce irradiance, a
// visibility (lighting reads them through screenAmbient).
static const uint AO_SECTORS=32;
// Ordered-dither index (0..15) of half-resolution pixel `p` in its 4x4 block.
uint aoBayer(uint2 p){uint2 a=p&1,b=(p>>1)&1;return (((a.x^a.y)<<1)|a.y)*4+(((b.x^b.y)<<1)|b.y);}
// acos to about 7e-5 rad (Abramowitz and Stegun 4.4.45): one square root, no
// transcendental, far finer than a sector. Arguments rounded past one clamp (no NaN).
float aoAcos(float x){float a=min(abs(x),1);float r=sqrt(1-a)*(1.5707288+a*(-0.2121144+a*(0.0742610-0.0187293*a)));return x<0?PI-r:r;}
// The share of the slice's hemisphere (n-pi/2..n+pi/2) below the direction at cosine
// `c` from the view vector on side `side` (+1 towards the slice direction, where angles
// grow), in GTAO's measure cos(x-n)|sin x|, whose integral from 0 to h is
// (cos n - cos(2h-n) + 2h sin n)/4; cos(2h-n) expands from `c`, so only the angle takes
// an inverse. `n`: the projected normal's angle (sine `sn`, cosine `cn`); `lower`: the
// measure from n-pi/2 to the view vector; `total`: the hemisphere's.
float aoShare(float c,float side,float n,float sn,float cn,float lower,float total){
    float h=side*aoAcos(c);if(h<=n-PI/2)return 0;if(h>=n+PI/2)return 1;
    float s=side*sqrt(saturate(1-c*c)),a=0.25*(cn-(2*c*c-1)*cn-2*s*c*sn+2*h*sn);
    return saturate((lower+(h<0?-a:a))/total);
}
// The sectors between hemisphere shares `a` and `b` (a <= b), rounded to the nearest edge.
uint aoSectors(float a,float b){
    uint lo=uint(a*AO_SECTORS+0.5),hi=uint(b*AO_SECTORS+0.5);if(hi<=lo)return 0;
    return (hi>=AO_SECTORS?0xffffffffu:(1u<<hi)-1)&~((1u<<lo)-1);
}
// Last frame's radiance at world `s` from reflection pyramid level `level` (1 or 2;
// look-ssr.slang packs the levels side by side), reprojected; `found` is false off screen.
float3 aoRadiance(Frame f,Look l,float3 s,uint level,out bool found){
    float4 h=mul(f.previousVp,float4(s,1));float2 uv=float2(h.x/h.w*0.5+0.5,0.5-h.y/h.w*0.5);
    found=h.w>0&&all(uv>0)&&all(uv<1);
    float2 size=float2(max(f.extent.xy>>level,uint2(1))),atlas=float2(f.extent.x,max(f.extent.y/2,1u));
    float origin=level==1?0:float(max(f.extent.x>>1,1u));
    return historyTex(l.ssrImages.x,(float2(origin,0)+clamp(uv*size,0.5,size-0.5))/atlas).rgb;
}
// Root word 2 as aoTraceMain. shadowParams[0]: x thickness (m), y bounce intensity (1
// physical); shadowInfo[0].x bit 0: last frame's pyramid holds colour (else no bounce).
[shader("compute")][numthreads(8,8,1)]
void aoBitmaskMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 p=int2(tid.xy)*2;float depth=pixel(f.extra.x,p).x;
    if(depth<=0){images[NonUniformResourceIndex(l.aoImages.z)][tid.xy]=float4(0,0,0,1);return;}
    float2 centre=float2(p)+0.5;float3 P=worldAtDepth(f,centre,depth),N=normalize(gbufferNormal(f,p)),V=normalize(f.camera.xyz-P);
    // Back faces (thin double-sided surfaces) occlude around the side the camera sees,
    // as Cycles shades them: their stored normal points away from it.
    if(dot(N,V)<0)N=-N;
    // Screen radius of the world radius at this distance.
    float3 side=normalize(cross(f.forward.xyz,abs(f.forward.y)<0.99?float3(0,1,0):float3(1,0,0)));
    float4 a=mul(f.vp,float4(P,1)),b=mul(f.vp,float4(P+side*l.ao.x,1));
    float radius=length((b.xy/b.w-a.xy/a.w)*0.5*float2(f.extent.xy));
    if(radius<1){images[NonUniformResourceIndex(l.aoImages.z)][tid.xy]=float4(0,0,0,1);return;}
    uint pattern=aoBayer(tid.xy),slices=uint(l.ao.y),steps=uint(l.ao.z);
    float rotation=(float(pattern)+0.5)/16,jitter=(float((pattern*7+5)&15)+0.5)/16;
    float thickness=l.shadowParams[0].x;bool bounce=(l.shadowInfo[0].x&1)!=0;
    // The brightest radiance a diffuse occluder of albedo one sends here: eight times
    // the exposure's mid-grey key. Last frame's colour also holds specular glints and,
    // on thin geometry, the background behind it; diffuse bounce never exceeds this.
    float cap=8*0.18/exposureScale(f,exposureState(l).y);
    float visible=0,weights=0,radius2=l.ao.x*l.ao.x;float3 light=0;
    for(uint s=0;s<slices;s++){
        float phi=(float(s)+rotation)/float(slices)*PI;float2 direction=float2(cos(phi),sin(phi));
        // The slice plane contains V and the world direction of `direction` on screen.
        float3 along=worldAtDepth(f,centre+direction*4,depth)-P;float3 sliceDirection=normalize(along-V*dot(along,V));
        float3 axis=normalize(cross(sliceDirection,V));float3 projected=N-axis*dot(N,axis);float length2=dot(projected,projected);
        if(length2<1e-6)continue;float projectedLength=sqrt(length2);projected/=projectedLength;
        float side=dot(projected,sliceDirection)<0?-1.:1.,cn=clamp(dot(projected,V),-1.,1.),n=side*aoAcos(cn),sn=side*sqrt(saturate(1-cn*cn));
        float lower=0.25*(2*cn+(2*n-PI)*sn),total=cn+n*sn;
        // Each side's sectors: a side whose sectors are all hidden stops marching.
        float middle=saturate(lower/total);uint positive=aoSectors(middle,1),negative=aoSectors(0,middle);
        uint hidden=0;float3 received=0;
        for(uint sideIndex=0;sideIndex<2;sideIndex++){
            float sign=sideIndex==0?1.:-1.;uint full=sideIndex==0?positive:negative;
            // The sample hiding the most sectors on this side lends its radiance to every
            // sector the side hides: one radiance and normal fetch per side, not per sample.
            uint sideBits=0,most=0;int2 source=0;float3 towards=0;float span=1;
            for(uint k=0;k<steps&&(hidden&full)!=full;k++){
                float t=(float(k)+jitter)/float(steps);float offset=max(t*t*radius,1+float(k));
                float2 q=centre+direction*sign*offset;if(any(q<0)||any(q>=float2(f.extent.xy)))break;
                int2 at=int2(q);float sampleDepth=pixel(f.extra.x,at).x;if(sampleDepth<=0)continue;
                float3 delta=worldAtDepth(f,floor(q)+0.5,sampleDepth)-P;float len2=dot(delta,delta);
                // Samples past the radius hide nothing; arcs shrink to their front over its
                // outer half (far occluders hid too much against Cycles).
                if(len2>=radius2)continue;
                float len=max(sqrt(len2),1e-5),reach=saturate((l.ao.x-len)/(0.5*l.ao.x));
                // Assumed thickness tapers from half the radius to none at the radius:
                // a far sample stands for a thin silhouette, not a solid slab.
                float3 back=delta-V*(thickness*saturate(2-2*len/l.ao.x));float front=dot(delta,V)/len;
                float rear=lerp(front,dot(back,V)*rsqrt(max(dot(back,back),1e-10)),reach);
                float a=aoShare(front,sign,n,sn,cn,lower,total),b=aoShare(rear,sign,n,sn,cn,lower,total);
                uint fresh=aoSectors(min(a,b),max(a,b))&~hidden;if(fresh==0)continue;
                // Cutouts (gbufferOccluder) hide nothing.
                if(gbufferOccluder(f,at)<0.5)continue;
                hidden|=fresh;uint count=countbits(fresh);sideBits+=count;
                if(count>most){most=count;source=at;towards=delta;span=len;}
            }
            if(bounce&&sideBits>0){
                // Half-resolution colour (the finest level: least background mixed in), at
                // most what a diffuse surface of the occluder's albedo sends. Metals keep
                // their (capped) colour: a sphere's reflected floor and sky do light its
                // contact shadow, as in the path-traced reference.
                bool found;float3 radiance=min(aoRadiance(f,l,P+towards,1,found),gbufferAlbedo(f,source).rgb*cap);
                // A diffuse sample sends its (view-independent) radiance where its front (the
                // side the camera sees) faces the pixel.
                float3 front=gbufferNormal(f,source);if(dot(front,f.camera.xyz-P-towards)<0)front=-front;
                float facing=saturate(-dot(front,towards)/span*4);
                if(found)received+=radiance*(facing*float(sideBits));
            }
        }
        float weight=projectedLength*total;
        visible+=weight*(1-float(countbits(hidden))/AO_SECTORS);light+=received*(weight/AO_SECTORS);weights+=weight;
    }
    // Open sectors carry no screen light; irradiance is pi times the cosine-weighted mean radiance.
    float v=weights>0?visible/weights:1;float3 e=weights>0?light/weights*(PI*l.shadowParams[0].y):0;
    // Finite and non-negative whatever the input: the denoise and lighting average these.
    v=isnan(v)?1:saturate(v);e=any(isnan(e))?float3(0):clamp(e,0,60000);
    images[NonUniformResourceIndex(l.aoImages.z)][tid.xy]=float4(e,v);
}
// 4x4 depth-aware box around each half-resolution pixel of the bitmask trace: it holds
// each phase of the trace's 4x4 pattern once, so the pattern averages out exactly on
// smooth surfaces (TAA resolves what edges leave). Relative depth alone weighs the
// taps, as GTAO's denoise: the bounce across a crease differs only within its 8 pixels.
[shader("compute")][numthreads(8,8,1)]
void aoBitmaskDenoiseMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 c=int2(tid.xy);float depth=pixel(f.extra.x,c*2).x;
    if(depth<=0){images[NonUniformResourceIndex(l.aoImages.w)][tid.xy]=float4(0,0,0,1);return;}
    float4 sum=0;float total=0,scale=72.13/depth;
    for(int y=-2;y<=1;y++)for(int x=-2;x<=1;x++){
        int2 q=clamp(c+int2(x,y),int2(0),int2(half)-1);float d=pixel(f.extra.x,q*2).x;
        // exp(-50 |d/depth - 1|): 2% relative depth is one e-fold; background weighs nothing.
        float4 a=pixel(l.aoImages.x,q);float w=d>0&&!any(isnan(a))?exp2(-abs(d-depth)*scale):0;sum+=w>0?a*w:0;total+=w;
    }
    images[NonUniformResourceIndex(l.aoImages.w)][tid.xy]=total>1e-5?sum/total:pixel(l.aoImages.x,c);
}
