// Reflection capture bake and relight (look/captures.rs). A capture's six faces
// are rendered by the raster renderer itself (deferred, unjittered, 90 degrees)
// and projected into the atlas's scratch column (`captureFaceMain`), box-reduced
// into a radiance mip chain (`captureMipMain`), then GGX-prefiltered with filtered
// importance sampling into the capture's own column (`capturePrefilterMain`).
// Every frame `captureRefitMain` measures the ambient light at each capture point
// (copying the records into the device-local frame buffer when they changed), and
// `captureTilesMain` lists the captures each 16x16 render tile can need.
// Columns hold five bordered octahedral tiles (look-captures.slangh captureLevelY).
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
static const uint CAPTURE_SAMPLES=128;
float captureRadical(uint bits){bits=(bits<<16)|(bits>>16);bits=((bits&0x55555555)<<1)|((bits&0xAAAAAAAA)>>1);bits=((bits&0x33333333)<<2)|((bits&0xCCCCCCCC)>>2);bits=((bits&0x0F0F0F0F)<<4)|((bits&0xF0F0F0F0)>>4);bits=((bits&0x00FF00FF)<<8)|((bits&0xFF00FF00)>>8);return float(bits)*2.3283064365386963e-10;}
float3 captureBasis(float3 n,float3 local){float3 t=normalize(cross(abs(n.y)<0.99?float3(0,1,0):float3(1,0,0),n));return t*local.x+cross(n,t)*local.y+n*local.z;}
// The interior texel a bordered tile's texel `p` (interior coordinates, -1..n)
// repeats: the border folds across the octahedral edges (look-gi.slangh octIndex).
int2 captureFold(int2 p,int n){
    if(p.x<0){p.x=-p.x-1;p.y=n-1-p.y;}else if(p.x>=n){p.x=2*n-1-p.x;p.y=n-1-p.y;}
    if(p.y<0){p.y=-p.y-1;p.x=n-1-p.x;}else if(p.y>=n){p.y=2*n-1-p.y;p.x=n-1-p.x;}
    return p;
}
// Root words (look/captures.rs bake passes): materials the face's HDR (sampled),
// material the atlas (storage), entity the face or level, cascade the scratch
// column's x, mode level 0's interior texels, sample the capture column's x.
RWTexture2D<float4> captureAtlas(){return images[NonUniformResourceIndex(root.material)];}
// Scratch radiance at level `k`, texel `p` of its bordered tile: level 0 holds
// weighted sums of face samples (rgb, weight), the reduced levels plain radiance.
float3 captureScratch(uint k,int2 p){
    float4 v=captureAtlas()[uint2(root.cascade,captureLevelY(root.mode,k))+uint2(p)];
    return k==0?v.rgb/max(v.a,1e-6):v.rgb;
}
// Bilinear scratch radiance along `d` at level `k` (tiles keep their borders).
float3 captureScratchAlong(uint k,float3 d){
    float n=float(root.mode>>k);float2 f=octEncode(d)*n+0.5;int2 b=int2(floor(f));float2 w=f-float2(b);
    float3 c00=captureScratch(k,b),c10=captureScratch(k,b+int2(1,0)),c01=captureScratch(k,b+int2(0,1)),c11=captureScratch(k,b+int2(1,1));
    return lerp(lerp(c00,c10,w.x),lerp(c01,c11,w.x),w.y);
}
// The exposure the HDR target stores (look-refract.slangh refractExposure): 1 while
// it holds linear radiance; with the pre-exposed HDR contract, hdrExposure(f, l).
// Faces are divided by it, so the atlas stays absolute radiance.
float captureHdrExposure(Frame f,Look l){return 1;}
// Projects the rendered face `root.entity` (0..5: +x, -x, +y, -y, +z, -z) into
// the scratch column's level 0 with four samples per texel; faces after the first
// add to what earlier faces wrote. Open sky is re-evaluated without the sun disk:
// shading adds the sun's specular analytically.
[shader("compute")][numthreads(8,8,1)]
void captureFaceMain(uint3 tid:SV_DispatchThreadID){
    int n=int(root.mode);if(any(tid.xy>=uint2(n+2,n+2)))return;
    Frame f=frame();Look l=look(f);int2 q=captureFold(int2(tid.xy)-1,n);float4 sum=0;
    for(uint s=0;s<4;s++){
        float3 d=octDecode((float2(q)+float2(s&1,s>>1)*0.5+0.25)/float(n)),a=abs(d);
        uint major=a.x>=a.y&&a.x>=a.z?(d.x>0?0:1):a.y>=a.z?(d.y>0?2:3):(d.z>0?4:5);
        if(major!=root.entity)continue;
        float4 h=mul(f.vp,float4(f.camera.xyz+d,1));if(h.w<=0)continue;
        float2 uv=saturate(float2(h.x/h.w*0.5+0.5,0.5-h.y/h.w*0.5));
        float3 c=textures[NonUniformResourceIndex(root.materials)].SampleLevel(samplers[2],uv,0).rgb/captureHdrExposure(f,l);
        if(physicalSky(l)&&all(textures[NonUniformResourceIndex(f.extra.x)].GatherRed(samplers[2],uv)==0))c=skyRadiance(f,l,d,false);
        // Half floats: keep finite, bounded radiance (four samples sum below 65504 after the quarter).
        c=all(c==c)?clamp(c,0,60000):0;
        sum+=float4(c,1);
    }
    uint2 p=uint2(root.cascade,0)+tid.xy;
    float4 previous=root.entity==0?0:captureAtlas()[p];
    captureAtlas()[p]=previous+sum*0.25;
}
// Scratch level `root.entity` (1..4): the 2x2 box of the level above, borders folded.
[shader("compute")][numthreads(8,8,1)]
void captureMipMain(uint3 tid:SV_DispatchThreadID){
    uint k=root.entity;int n=int(root.mode>>k);if(any(tid.xy>=uint2(n+2,n+2)))return;
    int2 q=captureFold(int2(tid.xy)-1,n)*2+1;
    float3 c=(captureScratch(k-1,q)+captureScratch(k-1,q+int2(1,0))+captureScratch(k-1,q+int2(0,1))+captureScratch(k-1,q+int2(1,1)))*0.25;
    captureAtlas()[uint2(root.cascade,captureLevelY(root.mode,k))+tid.xy]=float4(c,1);
}
// The capture's column (x `root.sample`): level 0 is the radiance itself; level k
// GGX-prefilters roughness k/4 by filtered importance sampling (Krivanek & Colbert
// 2008), each sample reading the scratch mip whose texels match its solid angle,
// so bright details blur without noise.
[shader("compute")][numthreads(8,8,1)]
void capturePrefilterMain(uint3 tid:SV_DispatchThreadID){
    uint n0=root.mode,k=0;if(tid.y>=captureLevelY(n0,CAPTURE_LEVELS))return;
    while(k+1<CAPTURE_LEVELS&&tid.y>=captureLevelY(n0,k+1))k++;
    int n=int(n0>>k);if(tid.x>=uint(n+2))return;
    int2 q=captureFold(int2(tid.x,tid.y-captureLevelY(n0,k))-1,n);float3 result;
    if(k==0)result=captureScratch(0,q+1);
    else{
        float3 dir=octDecode((float2(q)+0.5)/float(n));float a=float(k)/(CAPTURE_LEVELS-1);a*=a;float a2=a*a;
        float texel=4*PI/float(n0*n0);float3 sum=0;float weight=0;
        for(uint i=0;i<CAPTURE_SAMPLES;i++){
            float2 u=float2((i+0.5)/CAPTURE_SAMPLES,captureRadical(i));float phi=2*PI*u.x;
            float cosine=sqrt((1-u.y)/(1+(a2-1)*u.y)),sine=sqrt(max(0.,1-cosine*cosine));
            float3 h=captureBasis(dir,float3(cos(phi)*sine,sin(phi)*sine,cosine)),d=reflect(-dir,h);float nl=dot(dir,d);
            if(nl<=0)continue;
            // The view is the normal: pdf(l) = D(h) / 4.
            float D=a2/(PI*pow(cosine*cosine*(a2-1)+1,2));
            float lod=clamp(0.5*log2(4/(CAPTURE_SAMPLES*D*texel))+1,0,CAPTURE_LEVELS-1);
            uint lo=min(uint(lod),CAPTURE_LEVELS-1),hi=min(lo+1,CAPTURE_LEVELS-1);
            sum+=lerp(captureScratchAlong(lo,d),captureScratchAlong(hi,d),lod-float(lo))*nl;weight+=nl;
        }
        result=sum/max(weight,1e-4);
    }
    captureAtlas()[uint2(root.sample,captureLevelY(n0,k))+tid.xy]=float4(result,1);
}
// Relights each capture against its bake: the ambient irradiance at its capture
// point (the mean over the six axes of what `ibl` would use: probe GI where the
// volume covers, else the sky's), one thread per capture and axis so the six
// lookups run side by side. Root word 2: bit i (low half) marks a fresh bake, whose
// baseline becomes this measurement; bit 16+i clears a baseline (a new atlas or a
// moved capture). Root word 3 nonzero: the records changed (look/captures.rs
// `copy_records`), so each capture's first thread copies its record from host
// memory into the frame buffer first; the copies persist. Shading multiplies the
// capture by the current-to-baseline ratio times its multiplier (zero until a
// baseline exists).
groupshared float3 captureAxes[CAPTURE_MAX*6];
[shader("compute")][numthreads(96,1,1)] // CAPTURE_MAX captures x 6 axes
void captureRefitMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint i=tid.x/6,k=tid.x%6;bool live=i<l.captureInfo[0].y;
    uint records=l.captureInfo[0].x,target=l.captureInfo[0].w,slot=i*CAPTURE_SLOT;
    if(live){
        if(k==0&&root.entity!=0)
            for(uint b=0;b<CAPTURE_RECORD;b+=16)
                buffers[NonUniformResourceIndex(target)].Store<uint4>(slot+b,buffers[NonUniformResourceIndex(records)].Load<uint4>(i*CAPTURE_RECORD+b));
        float3 p=buffers[NonUniformResourceIndex(records)].Load<CaptureShade>(i*CAPTURE_RECORD+64).origin.xyz;
        uint irradiance=physicalSky(l)?l.sampled1.x:f.lighting.x;
        float3 n=float3(k==0?1.:k==1?-1.:0.,k==2?1.:k==3?-1.:0.,k==4?1.:k==5?-1.:0.);
        float3 ambient=envTex(irradiance,environmentUv(n)).rgb*horizonSky(l,p),gi=0,unused;float coverage=0;
        if(giEnabled(l))coverage=giPoolLobes(l,l.giImages.y,p,n,n,float3(0,0,0),false,gi,unused)*giWeight(l);
        captureAxes[tid.x]=lerp(ambient,gi,coverage);
    }
    GroupMemoryBarrierWithGroupSync();
    if(!live||k!=0)return;
    float3 current=0;
    for(uint a=0;a<6;a++)current+=captureAxes[tid.x+a];
    current/=6;
    CaptureShade c=buffers[NonUniformResourceIndex(records)].Load<CaptureShade>(i*CAPTURE_RECORD+64);
    float4 baked=((root.material>>(16+i))&1)!=0?float4(0,0,0,0):buffers[NonUniformResourceIndex(target)].Load<float4>(slot+CAPTURE_RECORD+16);
    if(((root.material>>i)&1)!=0)baked=float4(current,1);
    buffers[NonUniformResourceIndex(target)].Store<float4>(slot+CAPTURE_RECORD+16,baked);
    // A channel near zero at both times keeps its ratio near one.
    float eps=max(luminance(baked.rgb),1e-6)*1e-3;
    float4 ratio=baked.w>0?float4(clamp((current+eps)/(baked.rgb+eps),0.25,4)*c.extent.w,1):0;
    buffers[NonUniformResourceIndex(target)].Store<float4>(slot+CAPTURE_RECORD,ratio);
}
groupshared uint captureAny,captureFull;
// One 16x16 render tile per group, after the G-buffer: the visible captures whose
// influence reaches any of its opaque pixels, cut after the first (in blending
// order) that covers all of them fully (later ones receive no weight there).
// Deferred lighting tests only these (look-captures.slangh captureTileMask).
[shader("compute")][numthreads(16,16,1)]
void captureTilesMain(uint3 tid:SV_DispatchThreadID,uint3 group:SV_GroupID,uint index:SV_GroupIndex){
    Frame f=frame();Look l=look(f);
    if(index==0){captureAny=0;captureFull=~0u;}
    GroupMemoryBarrierWithGroupSync();
    uint reach=0,full=~0u;bool covered=false;float3 world=0;
    if(all(tid.xy<f.extent.xy))world=gbufferWorld(f,int2(tid.xy),covered);
    if(covered){
        full=0;
        for(uint m=l.captureInfo[1].z;m!=0;m&=m-1){
            uint i=firstbitlow(m);float3 p;float w=captureInfluence(captureCull(l,i),world,p);
            if(w>0)reach|=1u<<i;
            if(w>=1)full|=1u<<i;
        }
    }
    InterlockedOr(captureAny,reach);InterlockedAnd(captureFull,full);
    GroupMemoryBarrierWithGroupSync();
    if(index!=0)return;
    uint tile=group.y*((f.extent.x+15)/16)+group.x,any=captureAny,first=captureFull&any;
    uint mask=first!=0?any&((2u<<firstbitlow(first))-1):any;
    if(tile<CAPTURE_TILE_CAPACITY)buffers[NonUniformResourceIndex(l.captureInfo[0].w)].Store(CAPTURE_TILES+tile*4,mask);
}
