// Finite-range many-light shading V2 (ara_render look::lights; council-v3 D6, gate G1).
// Root words: frame = the raster frame record (G-buffer, light records at
// f.lighting.w, look record), materials = the V2 record (lights-v2.slangh
// LightsRecord); the bench primary pass uses the others (lightsPrimaryMain).
//
// Passes, in order (look/lights/gpu.rs records them):
// - lightsBuildMain: each light's sphere (centre, authored range: never shortened),
//   source radius and luminous intensity (the light grid's input).
// - lightsPrepMain: each 16x16 tile's receivers: the box around their surface points
//   and normal-offset ray origins and BRDF bound (bytes 32-47 unused).
// - lightsBinMain: each 64x64 coarse tile's lights (sphere against the union of its
//   tiles' boxes), in light order.
// - lightsSelectMain: each tile's lights from its coarse list (range, one-sided
//   emitters), each classified against trace()'s instances: fully clear pairs shade
//   without rays; partial pairs name the instances their segments may meet. With
//   LIGHTS_OMIT, sorted by their energy bound with suffix sums (omission).
// - lightsShadeMain: per pixel, every selected light through look-lights.slangh
//   localLight (the shading look::local uses), times per-source-sample visibility
//   for partial pairs; with LIGHTS_OMIT it stops once the remaining lights' bound is
//   at most the omitted-energy fraction of the luminance it has accumulated.
// - lightsExactMain: the exact finite-range control: every light, every pixel, every
//   source sample through the whole TLAS, same shading and pattern.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "trace.slangh"
#include "lights-v2.slangh"

// Sentinel in a tile head's count: the tile's lists overflowed; shade walks its
// coarse list (or every light) with exact visibility instead.
static const uint LIGHTS_FALLBACK=0x80000000u;
// Tile head count bit: some entry is partial (lightsShadeMain's visibility part runs).
static const uint LIGHTS_HAS_PARTIAL=0x40000000u,LIGHTS_COUNT=0x3fffffffu;
// Which entries lightsShadeMain shades (a compile-time PSO variant, so the lean part
// carries no trace() code): 0 every entry (omission and per-pixel counters use it),
// 1 the clear ones, writing the output, 2 the partial ones, adding to it.
#ifndef LIGHTS_PART
#define LIGHTS_PART 0
#endif
// Staged entries per tile in groupshared memory (a power of two, for the sort).
static const uint LIGHTS_STAGE=512;

[shader("compute")]
[numthreads(64,1,1)]
void lightsBuildMain(uint3 id:SV_DispatchThreadID){
    Frame f=frame();LightsRecord r=lightsRecord();uint i=id.x;if(i>=r.counts.x)return;
    Light light=loadLight(f.lighting.w,i);LightShape shape=lightShaped(light)?loadLightShape(f.lighting.w,i):(LightShape)0;
    float intensity=luminance(light.colorLumens.rgb)*light.colorLumens.w/(4*PI);
    float range=all(light.colorLumens.rgb<=0)||light.colorLumens.w<=0?0:light.positionRange.w;
    buffers[r.buffers.x].Store4(i*32,asuint(float4(light.positionRange.xyz,range)));
    buffers[r.buffers.x].Store4(i*32+16,asuint(float4(intensity,lightsSourceRadius(light,shape),0,0)));
}

groupshared float3 prepLo[16],prepHi[16],prepOLo[16],prepOHi[16];
groupshared float4 prepM[32];
[shader("compute")]
[numthreads(16,16,1)]
void lightsPrepMain(uint3 g:SV_GroupID,uint3 t:SV_GroupThreadID,uint gi:SV_GroupIndex){
    Frame f=frame();LightsRecord r=lightsRecord();
    int2 p=int2(g.xy*LIGHTS_TILE+t.xy);bool covered=false;float3 world=0,n=0;float rough=1,albedo=0;
    if(all(p<int2(f.extent.xy))){world=gbufferWorld(f,p,covered);}
    // Receivers and their ray origins (range and bounds use both; the
    // visibility hull only the origins, so a surface never occludes itself).
    float3 lo=1e30,hi=-1e30,olo=1e30,ohi=-1e30;
    if(covered){
        n=gbufferNormal(f,p);float3 q=world+n*lightsBias(f,r,world);
        lo=min(world,q);hi=max(world,q);olo=q;ohi=q;
        // The BRDF bound only serves omission: skip the material reads without it.
        if((r.trace.w&LIGHTS_OMIT)!=0){float2 m=gbufferMaterial(f,p);rough=m.x;float3 a=gbufferAlbedo(f,p).rgb;albedo=max(max(a.r,a.g),a.b);}
    }
    // Wave reductions, then one lane per wave into groupshared memory (any wave size
    // of at least 16 lanes).
    uint wave=gi/WaveGetLaneCount(),waves=(256+WaveGetLaneCount()-1)/WaveGetLaneCount();
    float3 wlo=WaveActiveMin(lo),whi=WaveActiveMax(hi),wolo=WaveActiveMin(olo),wohi=WaveActiveMax(ohi);
    float wr=WaveActiveMin(covered?rough:1.),wa=WaveActiveMax(albedo),wc=WaveActiveSum(covered?1.:0.);
    if(WaveIsFirstLane()){prepLo[wave]=wlo;prepHi[wave]=whi;prepOLo[wave]=wolo;prepOHi[wave]=wohi;prepM[wave]=float4(wr,wa,wc,1);}
    GroupMemoryBarrierWithGroupSync();
    if(gi==0){
        for(uint k=1;k<waves;k++){
            prepLo[0]=min(prepLo[0],prepLo[k]);prepHi[0]=max(prepHi[0],prepHi[k]);prepOLo[0]=min(prepOLo[0],prepOLo[k]);
            prepOHi[0]=max(prepOHi[0],prepOHi[k]);
            float4 a=prepM[0],b=prepM[k];prepM[0]=float4(min(a.x,b.x),max(a.y,b.y),a.z+b.z,1);
        }
    }
    if(gi==0){
        uint tile=g.x+g.y*r.counts.y;float4 m=prepM[0];
        // look-shading.slangh brdf's bound per unit illuminance (lightsBound).
        float rr=max(m.x,0.045),r6=rr*rr*rr*rr*rr*rr;
        float brdfMax=m.y/PI+1.5/(PI*r6);
        buffers[r.buffers.z].Store4(tile*80,asuint(float4(prepLo[0],m.z)));
        buffers[r.buffers.z].Store4(tile*80+16,asuint(float4(prepHi[0],m.x)));
        buffers[r.buffers.z].Store4(tile*80+48,asuint(float4(prepOLo[0],brdfMax)));
        buffers[r.buffers.z].Store4(tile*80+64,asuint(float4(prepOHi[0],0)));
    }
}

// Squared distance from point `c` to box lo..hi.
float lightsBoxDistance2(float3 c,float3 lo,float3 hi){float3 d=max(max(lo-c,c-hi),0);return dot(d,d);}

groupshared uint binBits[2];
groupshared uint binCount;
groupshared float3 binLo,binHi;
[shader("compute")]
[numthreads(64,1,1)]
void lightsBinMain(uint3 g:SV_GroupID,uint gi:SV_GroupIndex){
    Frame f=frame();LightsRecord r=lightsRecord();
    uint coarse=g.x+g.y*r.counts.w,cap=r.output.z,base=coarse*(cap+1);
    if(gi==0){
        float3 lo=1e30,hi=-1e30;
        for(uint k=0;k<LIGHTS_COARSE*LIGHTS_COARSE;k++){
            uint2 ft=g.xy*LIGHTS_COARSE+uint2(k%LIGHTS_COARSE,k/LIGHTS_COARSE);
            if(ft.x>=r.counts.y||ft.y>=r.counts.z)continue;
            uint tile=ft.x+ft.y*r.counts.y;
            float4 a=asfloat(buffers[r.buffers.z].Load4(tile*80)),b=asfloat(buffers[r.buffers.z].Load4(tile*80+16));
            if(a.w>0){lo=min(lo,a.xyz);hi=max(hi,b.xyz);}
        }
        binLo=lo;binHi=hi;binCount=0;
    }
    GroupMemoryBarrierWithGroupSync();
    float3 lo=binLo,hi=binHi;bool empty=any(lo>hi);
    for(uint chunk=0;chunk<r.counts.x&&!empty;chunk+=64){
        if(gi<2)binBits[gi]=0;
        GroupMemoryBarrierWithGroupSync();
        uint i=chunk+gi;bool keep=false;
        if(i<r.counts.x){float4 sphere=asfloat(buffers[r.buffers.x].Load4(i*32));keep=sphere.w>0&&lightsBoxDistance2(sphere.xyz,lo,hi)<sphere.w*sphere.w;}
        if(keep)InterlockedOr(binBits[gi/32],1u<<(gi%32));
        GroupMemoryBarrierWithGroupSync();
        uint before=countbits(binBits[0]&(gi<32?(1u<<gi)-1:0xffffffffu))+(gi>=32?countbits(binBits[1]&((1u<<(gi-32))-1)):0);
        uint total=countbits(binBits[0])+countbits(binBits[1]);
        uint at=binCount+before;
        if(keep&&at<cap)buffers[r.buffers.y].Store((base+1+at)*4,i);
        GroupMemoryBarrierWithGroupSync();
        if(gi==0)binCount+=total;
        GroupMemoryBarrierWithGroupSync();
    }
    if(gi==0){
        uint count=empty?0:binCount;
        if(count>cap){buffers[r.output.y].InterlockedAdd(LIGHTS_STAT_COARSE*4,1);count=LIGHTS_FALLBACK;}
        buffers[r.buffers.y].Store(base*4,count);
    }
}

groupshared uint4 stageEntry[LIGHTS_STAGE];
groupshared float stageBound[LIGHTS_STAGE];
groupshared uint selBits[2];
groupshared uint selCount,selFirst,selSpent,selPartial;
groupshared uint selOverflow;
[shader("compute")]
[numthreads(64,1,1)]
void lightsSelectMain(uint3 g:SV_GroupID,uint gi:SV_GroupIndex){
    Frame f=frame();LightsRecord r=lightsRecord();
    uint tile=g.x+g.y*r.counts.y;
    float4 a=asfloat(buffers[r.buffers.z].Load4(tile*80)),b=asfloat(buffers[r.buffers.z].Load4(tile*80+16));
    float4 olo=asfloat(buffers[r.buffers.z].Load4(tile*80+48));
    float3 ohi=asfloat(buffers[r.buffers.z].Load4(tile*80+64)).xyz;float brdfMax=olo.w;
    if(a.w<=0){if(gi==0)buffers[r.buffers.w].Store2(tile*8,uint2(0,0));return;}
    float3 lo=a.xyz,hi=b.xyz,centre=(lo+hi)*0.5,ext=(hi-lo)*0.5;
    uint cap=r.output.z,coarse=g.x/LIGHTS_COARSE+g.y/LIGHTS_COARSE*r.counts.w,base=coarse*(cap+1);
    uint listed=buffers[r.buffers.y].Load(base*4);
    bool scanAll=listed==LIGHTS_FALLBACK;uint total=scanAll?r.counts.x:listed;
    bool shadowed=r.trace.x!=TRACE_NONE&&(r.trace.w&LIGHTS_UNSHADOWED)==0;
    TraceScene s=traceScene(shadowed?r.trace.x:0);
    if(gi==0){selCount=0;selOverflow=0;selSpent=0;selPartial=0;}
    GroupMemoryBarrierWithGroupSync();
    for(uint chunk=0;chunk<total;chunk+=64){
        if(gi<2)selBits[gi]=0;
        GroupMemoryBarrierWithGroupSync();
        uint j=chunk+gi;bool keep=false;uint4 entry=0;float bound=0;
        if(j<total){
            uint i=scanAll?j:buffers[r.buffers.y].Load((base+1+j)*4);
            float4 sphere=asfloat(buffers[r.buffers.x].Load4(i*32)),info=asfloat(buffers[r.buffers.x].Load4(i*32+16));
            float near2=lightsBoxDistance2(sphere.xyz,lo,hi);
            keep=sphere.w>0&&near2<sphere.w*sphere.w;
            float rs=info.y;
            Light light=(Light)0;LightShape shape=(LightShape)0;
            if(keep){
                light=loadLight(f.lighting.w,i);
                if(lightShaped(light))shape=loadLightShape(f.lighting.w,i);
                // No facing cull: look::local's sphere and area shading (LTC, clipped in
                // the transformed space) lights surfaces slightly past a source's geometric
                // horizon, so a horizon test would drop light the exact control shades.
            }
            if(keep){
                // One-sided emitters light only what lies in front of their plane.
                if(lightKind(light)>=LIGHT_RECT){
                    float3 ln=light.directionOuter.xyz;
                    keep=dot(ln,centre-sphere.xyz)+dot(abs(ln),ext)>0;
                }
            }
            if(keep){
                entry=uint4(i,TRACE_NONE,TRACE_NONE,0);
                float near=sqrt(near2);
                if(shadowed&&(r.trace.w&LIGHTS_NOCLASSIFY)!=0)entry.x|=LIGHTS_PARTIAL|LIGHTS_ALL;
                else if(shadowed){
                    uint spent=0;
                    uint3 c=lightsClassify(s,olo.xyz,ohi,sphere.xyz,lightsSampleRadius(light,shape,near),r.trace.z,spent);
                    entry.x|=c.x;entry.y=c.y;entry.z=c.z;
                    InterlockedAdd(selSpent,spent);
                }
                bound=lightsBound(info.x,sphere.w,near,rs,brdfMax);
            }
        }
        if(keep)InterlockedOr(selBits[gi/32],1u<<(gi%32));
        GroupMemoryBarrierWithGroupSync();
        uint before=countbits(selBits[0]&(gi<32?(1u<<gi)-1:0xffffffffu))+(gi>=32?countbits(selBits[1]&((1u<<(gi-32))-1)):0);
        uint added=countbits(selBits[0])+countbits(selBits[1]);
        uint at=selCount+before;
        if(keep&&at<LIGHTS_STAGE){stageEntry[at]=entry;stageBound[at]=bound;}
        if(keep&&(entry.x&LIGHTS_PARTIAL)!=0)InterlockedOr(selPartial,1u);
        GroupMemoryBarrierWithGroupSync();
        if(gi==0){selCount+=added;if(selCount>LIGHTS_STAGE)selOverflow=1;}
        GroupMemoryBarrierWithGroupSync();
    }
    uint count=min(selCount,LIGHTS_STAGE);
    bool omit=(r.trace.w&LIGHTS_OMIT)!=0;
    if(omit&&selOverflow==0&&count>1){
        // Bitonic sort, descending bound, ties by light index (deterministic).
        uint size=1;while(size<count)size<<=1;
        for(uint k=gi;k<size;k+=64)if(k>=count){stageBound[k]=-1;stageEntry[k]=uint4(0xffffffffu,0,0,0);}
        GroupMemoryBarrierWithGroupSync();
        for(uint width=2;width<=size;width<<=1){
            for(uint stride=width>>1;stride>0;stride>>=1){
                for(uint k=gi;k<size;k+=64){
                    uint other=k^stride;
                    if(other>k){
                        bool descending=(k&width)==0;
                        float bk=stageBound[k],bo=stageBound[other];uint ik=stageEntry[k].x&LIGHTS_INDEX,io=stageEntry[other].x&LIGHTS_INDEX;
                        bool before=bk>bo||(bk==bo&&ik<io);
                        if(before!=descending){
                            uint4 e=stageEntry[k];stageEntry[k]=stageEntry[other];stageEntry[other]=e;
                            stageBound[k]=bo;stageBound[other]=bk;
                        }
                    }
                }
                GroupMemoryBarrierWithGroupSync();
            }
        }
        if(gi==0){float suffix=0;for(int k=int(count)-1;k>=0;k--){suffix+=stageBound[k];stageEntry[k].w=asuint(suffix);}}
        GroupMemoryBarrierWithGroupSync();
    }
    if(gi==0){
        uint first=0;bool overflow=selOverflow!=0;
        if(!overflow){buffers[r.output.y].InterlockedAdd(LIGHTS_STAT_ENTRIES*4,count,first);overflow=first+count>r.output.w;}
        if(selOverflow!=0)buffers[r.output.y].InterlockedAdd(LIGHTS_STAT_STAGE*4,1);
        else if(overflow)buffers[r.output.y].InterlockedAdd(LIGHTS_STAT_POOL*4,1);
        buffers[r.output.y].InterlockedAdd(LIGHTS_STAT_BUDGET*4,selSpent);
        selFirst=overflow?0xffffffffu:first;
        buffers[r.buffers.w].Store2(tile*8,overflow?uint2(0,LIGHTS_FALLBACK):uint2(first,count|(selPartial!=0?LIGHTS_HAS_PARTIAL:0)));
    }
    GroupMemoryBarrierWithGroupSync();
    if(selFirst==0xffffffffu)return;
    uint partial=0,everything=0;
    for(uint k=gi;k<count;k+=64){
        uint4 e=stageEntry[k];buffers[r.extra.x].Store4((selFirst+k)*16,e);
        partial+=(e.x&LIGHTS_PARTIAL)!=0?1:0;everything+=(e.x&LIGHTS_ALL)!=0?1:0;
    }
    if(partial>0)buffers[r.output.y].InterlockedAdd(LIGHTS_STAT_PARTIAL*4,partial);
    if(everything>0)buffers[r.output.y].InterlockedAdd(LIGHTS_STAT_ALL*4,everything);
}

// The G-buffer surface at `p` and its view vector.
Surface lightsSurface(Frame f,int2 p){
    Surface s;float4 a=gbufferAlbedo(f,p);float2 m=gbufferMaterial(f,p);
    s.albedo=a.rgb;s.alpha=1;s.transmission=0;s.ao=1;s.normal=gbufferNormal(f,p);s.roughness=m.x;s.metal=m.y;s.emissive=0;
    return s;
}

// One light at one pixel: localLight times visibility (`entry` classes it; a zero
// class shades unshadowed, as fully clear pairs are). `rays` counts segments.
float3 lightsShadeOne(Frame f,Look l,LightsRecord r,TraceScene s,bool shadowed,Surface surface,Lobes o,float3 v,float3 world,float3 origin,uint4 entry,inout AreaPixel area,inout uint rays,inout uint lit){
    uint index=entry.x&LIGHTS_INDEX;Light light=loadLight(f.lighting.w,index);
    float size;float3 c=localLight(f,l,surface,o,v,world,index,light,area,size);
    if(!any(c>0))return 0;
    lit++;
    if(shadowed&&(entry.x&LIGHTS_PARTIAL)!=0){
        LightShape shape=lightShaped(light)?loadLightShape(f.lighting.w,index):(LightShape)0;
#if LIGHTS_PART!=1
        c*=lightsVisibility(s,origin,light,shape,entry,r.trace.y,r.trace.z,(r.trace.w&LIGHTS_NOMASK)==0,rays);
#endif
    }
    return c;
}

[shader("compute")]
[numthreads(16,16,1)]
void lightsShadeMain(uint3 g:SV_GroupID,uint3 t:SV_GroupThreadID){
    Frame f=frame();LightsRecord r=lightsRecord();
    int2 p=int2(g.xy*LIGHTS_TILE+t.xy);if(any(p>=int2(f.extent.xy)))return;
    bool covered;float3 world=gbufferWorld(f,p,covered);
    bool stats=(r.trace.w&LIGHTS_STATS)!=0;
    uint tile=g.x+g.y*r.counts.y;uint2 head=buffers[r.buffers.w].Load2(tile*8);
    bool fallback=head.y==LIGHTS_FALLBACK;uint count=fallback?0:head.y&LIGHTS_COUNT;
#if LIGHTS_PART==2
    // Only tiles with partial pairs (or overflowed lists) have work here.
    if(!covered||(!fallback&&(head.y&LIGHTS_HAS_PARTIAL)==0))return;
#else
    if(!covered){images[NonUniformResourceIndex(r.output.x)][p]=0;if(stats)images[NonUniformResourceIndex(r.extra.y)][p]=0;return;}
#endif
    Look l=look(f);
    Surface surface=lightsSurface(f,p);float3 v=normalize(f.camera.xyz-world);Lobes o=specularLobes(surface,v);AreaPixel area=(AreaPixel)0;
    float3 origin=world+surface.normal*lightsBias(f,r,world);
    bool shadowed=r.trace.x!=TRACE_NONE&&(r.trace.w&LIGHTS_UNSHADOWED)==0;TraceScene s=traceScene(shadowed?r.trace.x:0);
    bool omit=(r.trace.w&LIGHTS_OMIT)!=0;float fraction=r.params.x;
    lightsDebugOn=(r.trace.w&LIGHTS_DEBUG)!=0;
    float3 sum=0;float accumulated=0;uint rays=0,lit=0,omitted=0,considered=0;
    if(fallback&&LIGHTS_PART!=1){
        // Overflowed lists: every light of the coarse list (or scene), exact visibility
        // (the visibility part shades them whole; the lean part leaves them zero).
        uint cap=r.output.z,coarse=g.x/LIGHTS_COARSE+g.y/LIGHTS_COARSE*r.counts.w,base=coarse*(cap+1);
        uint listed=buffers[r.buffers.y].Load(base*4);bool scanAll=listed==LIGHTS_FALLBACK;uint total=scanAll?r.counts.x:listed;
        for(uint j=0;j<total;j++){
            uint i=scanAll?j:buffers[r.buffers.y].Load((base+1+j)*4);
            sum+=lightsShadeOne(f,l,r,s,shadowed,surface,o,v,world,origin,uint4(i|LIGHTS_PARTIAL|LIGHTS_ALL,TRACE_NONE,TRACE_NONE,0),area,rays,lit);
        }
        considered=total;
    }else if(!fallback){
        considered=count;
        for(uint j=0;j<count;j++){
            uint4 entry=buffers[r.extra.x].Load4((head.x+j)*16);
#if LIGHTS_PART==1
            if((entry.x&LIGHTS_PARTIAL)!=0)continue;
#elif LIGHTS_PART==2
            if((entry.x&LIGHTS_PARTIAL)==0)continue;
#endif
            if(omit&&asfloat(entry.w)<=fraction*accumulated){omitted=count-j;break;}
            float3 c=lightsShadeOne(f,l,r,s,shadowed,surface,o,v,world,origin,entry,area,rays,lit);
            sum+=c;accumulated+=luminance(c);
        }
    }
#if LIGHTS_PART==2
    if(!fallback)sum+=images[NonUniformResourceIndex(r.output.x)][p].rgb;
#endif
    // LIGHTS_DEBUG: the ray origin of the recorded disagreement replaces the radiance.
    images[NonUniformResourceIndex(r.output.x)][p]=lightsDebugOn?float4(lightsDebugOrigin,float(lightsDebugTri)):float4(sum,1);
    if(stats)images[NonUniformResourceIndex(r.extra.y)][p]=lightsDebugOn?float4(lightsDebug):float4(lit,rays,considered,omitted);
}

[shader("compute")]
[numthreads(8,8,1)]
void lightsExactMain(uint3 id:SV_DispatchThreadID){
    Frame f=frame();LightsRecord r=lightsRecord();int2 p=int2(id.xy);if(any(p>=int2(f.extent.xy)))return;
    bool covered;float3 world=gbufferWorld(f,p,covered);bool stats=(r.trace.w&LIGHTS_STATS)!=0;
    if(!covered){images[NonUniformResourceIndex(r.output.x)][p]=0;if(stats)images[NonUniformResourceIndex(r.extra.y)][p]=0;return;}
    Look l=look(f);Surface surface=lightsSurface(f,p);float3 v=normalize(f.camera.xyz-world);Lobes o=specularLobes(surface,v);AreaPixel area=(AreaPixel)0;
    float3 origin=world+surface.normal*lightsBias(f,r,world);
    bool shadowed=r.trace.x!=TRACE_NONE&&(r.trace.w&LIGHTS_UNSHADOWED)==0;TraceScene s=traceScene(shadowed?r.trace.x:0);
    float3 sum=0;uint rays=0,lit=0;lightsDebugOn=(r.trace.w&LIGHTS_DEBUG)!=0;
    for(uint i=0;i<r.counts.x;i++)sum+=lightsShadeOne(f,l,r,s,shadowed,surface,o,v,world,origin,uint4(i|LIGHTS_PARTIAL|LIGHTS_ALL,TRACE_NONE,TRACE_NONE,0),area,rays,lit);
    images[NonUniformResourceIndex(r.output.x)][p]=lightsDebugOn?float4(lightsDebugOrigin,float(lightsDebugTri)):float4(sum,1);
    if(stats)images[NonUniformResourceIndex(r.extra.y)][p]=lightsDebugOn?float4(lightsDebug):float4(lit,rays,r.counts.x,0);
}

// Bench primary visibility (look/lights/bench.rs; not a production pass): the camera
// ray through each pixel centre against trace()'s scene, written as a G-buffer in the
// renderer's encodings. Root words: material = material table (32 bytes per caller
// word: albedo rgb + roughness, metalness), entity / cascade / mode / sample = depth,
// normal, albedo and material storage images, flags = the trace scene record.
[shader("compute")]
[numthreads(8,8,1)]
void lightsPrimaryMain(uint3 id:SV_DispatchThreadID){
    Frame f=frame();int2 p=int2(id.xy);if(any(p>=int2(f.extent.xy)))return;
    float3 d=viewRay(f,float2(p)+0.5);TraceScene s=traceScene(root.flags);
    TraceHit hit;bool found=traceClosest(s,traceRay(f.camera.xyz,d,0.,1e30),hit);
    if(!found){
        images[NonUniformResourceIndex(root.entity)][p]=0;images[NonUniformResourceIndex(root.cascade)][p]=float4(0.5,0.5,0,1);
        images[NonUniformResourceIndex(root.mode)][p]=0;images[NonUniformResourceIndex(root.sample)][p]=0;return;
    }
    float3 world=f.camera.xyz+d*hit.t;float3 n=traceHitNormal(s,hit);if(dot(n,d)>0)n=-n;
    float4 a=asfloat(buffers[root.material].Load4(hit.user*32)),m=asfloat(buffers[root.material].Load4(hit.user*32+16));
    float4 clip=mul(f.vp,float4(world,1));
    images[NonUniformResourceIndex(root.entity)][p]=float4(clip.z/clip.w,0,0,0);
    images[NonUniformResourceIndex(root.cascade)][p]=gbufferEncodeNormal(n,1);
    images[NonUniformResourceIndex(root.mode)][p]=float4(a.rgb,1);
    // w: the material slot (the bench reads it back to classify surfaces).
    images[NonUniformResourceIndex(root.sample)][p]=float4(a.w,m.x,1,float(hit.user)/255.);
}
