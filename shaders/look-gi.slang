// Per-frame probe relighting (look/gi.rs): surface caches to 6x6 irradiance.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
groupshared float3 radiance[64];
groupshared float4 directions[64];
groupshared float3 tile[36];
groupshared float4 seen[64];
// Distance (m) recorded for sky texels: reflections of the sky need no parallax.
static const float GI_SKY_DISTANCE=10000;
// Writes this probe's bordered 10x10 radiance tile (giSampleRadiance) from `seen`.
void storeRadiance(Look l,uint id,uint lane){
    for(uint i=lane;i<100;i+=64){
        int2 p=int2(i%10,i/10)-1;images[NonUniformResourceIndex(l.giRadiance.y)][giTile(l,id,8)+p]=seen[octIndex(p,8)];
    }
}
// Writes this probe's bordered 8x8 atlas tile from the 6x6 interior in `tile`.
void storeTile(Look l,uint id,uint lane){
    int2 p=int2(lane%8,lane/8)-1;images[NonUniformResourceIndex(l.giImages.z)][giTile(l,id,6)+p]=float4(tile[octIndex(p,6)],1);
}
// Width in metres of the sun shadow texel that tests `world`: its virtual shadow
// level's, else its cascade's.
float shadowTexel(Frame f,Look l,float3 world){
    if(l.vsm.y!=0)return vsmStep(l,vsmLevel(f,l,world,vsmLight(l,world)))/VSM_PAGE;
    uint c=min(cascadeIndex(f,world),max(f.look.z,1u)-1);return 2/(length(frameShadow(c)[0].xyz)*float(f.look.y));
}
// Visibility over a disc of `radius` metres around `world` (unit `normal`, `toLight`
// towards the light) in tile `s`: plain PCF over the receiver's tangent plane at the
// moment pyramid level the disc spans (look-shadow.slangh shadowKernel, fixed), the
// mean light a probe texel's footprint receives in one lookup; -1 outside the tile.
float giShadowFootprint(ShadowMap s,float3 world,float3 normal,float3 toLight,float radius){
    ShadowReceiver r=shadowReceiver(s,world,normal,toLight);if(!r.inside)return -1;
    float h=max(radius/r.texel,0.75);int level=clamp(int(round(log2(h)-1)),-1,shadowTop(s));
    return shadowKernel(s,r,shadowSun(0),level,float2(h,0),float2(0,h),true,true);
}
// Sun visibility at a probe texel's hit: virtual shadow pages first where resident
// (`pages`, a literal per entry point), else over the texel's footprint (`radius`
// metres around the hit). Shading picks a cascade by view depth, and a point outside
// its map counts as lit: right for pixels, which lie in the view, but probe hits
// surround the camera, and behind or beside it every one would be sunlit. Hits take
// the first cascade whose map holds them, else the far map.
float giSunVisibility(Frame f,Look l,float3 world,float3 normal,float radius,bool pages){
    float v=pages?vsmVisibility(f,l,world,normal,true):-1;bool mapped=v>=0;if(!mapped)v=1;uint maps=min(f.look.z,4u);
    // Cascades in order, then the far map (k == 4).
    [loop]for(uint k=0;k<5&&!mapped;k++){
        if(k<4&&k>=maps)continue;
        if(k==4&&l.farInfo.z==0)break;
        float s=giShadowFootprint(k<4?shadowCascade(f,l,k):shadowFar(l),world,normal,f.sunDirection.xyz,radius);
        if(s>=0){v=s;mapped=true;}
    }
    return v>0?min(v,horizonSun(l,world,mapped)):0;
}
// Where the direction `k` of an n x n grid (n = 2 or 4) across probe texel `texel`
// (octahedral, of 8x8) meets the hit plane through `hit` (normal `normal`, `along`
// its signed distance from the probe at `origin`, `distance` the hit's), and whether
// it counts (w): only where the bake found the texel's sub-rays there on that plane
// (`mask`, bit k of its 4x4; two of a 2x2 cell's four) and the direction meets the plane
// near the hit's distance. The plane goes on past the surface (a floor's beyond the
// room's walls, a wall's across its window), and those points were sunlit where the
// texel sees a wall or the sky: rooms read half as bright again.
float4 giFootprintPoint(float3 origin,uint2 texel,uint k,uint n,uint mask,float3 hit,float3 normal,float along,float distance){
    uint2 sub=uint2(k%n,k/n);float3 d=octDecode((float2(texel)+(float2(sub)+0.5)/float(n))/8);
    uint bits=n==4?(mask>>k)&1:countbits((mask>>(sub.y*8+sub.x*2))&0x33);
    float cosine=dot(d,normal);float t=cosine<-0.05?along/cosine:-1;
    return t>0.5*distance&&t<2*distance&&bits>=(n==4?1u:2u)?float4(origin+d*t,1):float4(hit,0);
}
// The direct light (sun and local lights) a probe texel's hit receives, averaged over
// the texel's footprint: one point stood for the whole texel (some 22 degrees), so a
// probe point-sampled sharp lighting (a light profile's spokes, a sunlit patch smaller
// than the texel) and neighbouring probes disagreed: blotches, and bright patches
// over-weighted. The hit plane (point `hit`, normal `normal`, seen from `origin`;
// octahedral `texel` of 8x8, solid angle `omega`, bake plane `mask`) takes each local
// light's unshadowed light at a grid of points across the texel (giFootprintPoint: 4x4
// for light profiles and hard-edged spot cones, whose patterns are sharp; 2x2 for the
// rest, smooth across it); the sun's shadow at 4x4 such points, each over a quarter of
// the footprint's disc (virtual shadow pages first, else giShadowFootprint), averaged
// over the points on the texel's surface (the hit alone where none is); and with
// LOOK_LOCAL_INDIRECT each local light's shadow once over the whole disc where it owns
// a shadow map this frame (look::local; the rest stay unshadowed).
float3 giFootprintDirect(Frame f,Look l,float3 origin,uint2 texel,float omega,uint mask,float3 hit,float3 normal,bool pages){
    float3 toward=hit-origin;float along=dot(toward,normal),distance=length(toward);
    float facing=abs(along)/max(distance,1e-4);
    // The footprint's disc: the texel's cone at the hit, widened by the plane's slant.
    float radius=distance*sqrt(omega/PI)*rsqrt(max(facing,0.25));
    // Shadow tests sit eight shadow texels off the plane, so it does not shadow itself.
    float3 lifted=hit+normal*(8*shadowTexel(f,l,hit)),sun=f.sunDirection.xyz;
    // The sun at a 4x4 grid of points across the footprint, each over a quarter of its
    // disc: one test per texel (a virtual page is a point compare, and a disc's PCF on
    // a coarse cascade level turns a sunlit patch edge all-or-nothing) aliased the
    // light of a patch smaller than a few texels, so probe columns alternated between
    // 0.6 and 1.8 times the light past a sunlit wall.
    // Glass and films between each point and the sun tint it (look-translucent.slangh).
    float3 sunSeen=0;float sunPoints=0;float3 lift=lifted-hit;
    for(uint k=0;k<16;k++){
        float4 q=giFootprintPoint(origin,texel,k,4,mask,hit,normal,along,distance);
        if(q.w>0){float3 p=q.xyz+lift;sunSeen+=giSunVisibility(f,l,p,normal,radius*0.25,pages)*translucentSun(l,p);sunPoints+=1;}
    }
    if(sunPoints==0){sunSeen=giSunVisibility(f,l,lifted,normal,radius*0.25,pages)*translucentSun(l,lifted);sunPoints=1;}
    float3 light=sunAt(f,l,hit)*saturate(dot(normal,sun))*(sunSeen/sunPoints);
    for(uint i=0;i<f.extent.z;i++){
        // look-lights.slangh: each light's form factor over its area.
        Light source=loadLight(f.lighting.w,i);
        float3 delta=source.positionRange.xyz-hit;float reach=source.positionRange.w+radius;
        if(dot(delta,delta)>=reach*reach)continue;
        bool sharp=uint(source.parameters.x+0.5)>=LIGHT_SHAPED||(lightKind(source)==LIGHT_SPOT&&source.parameters.y-source.directionOuter.w<0.05);
        uint n=sharp?4:2;float3 e=0;float points=0;
        for(uint k=0;k<n*n;k++){
            float4 q=giFootprintPoint(origin,texel,k,n,mask,hit,normal,along,distance);
            if(q.w>0){e+=lightIrradiance(f,l,source,i,q.xyz,normal);points+=1;}
        }
        if(points==0){e=lightIrradiance(f,l,source,i,hit,normal);points=1;}
        ShadowMap s;
        if(any(e>0)&&(l.flags.y&LOOK_LOCAL_INDIRECT)!=0&&shadowLocal(l,i,hit,source.positionRange.xyz,source.positionRange.w,s)){
            float v=giShadowFootprint(s,lifted,normal,normalize(delta),radius);
            if(v>=0)e*=v;
        }
        light+=e/points;
    }
    return light;
}
// One workgroup per relit probe, one lane per cache texel: this frame's window
// (giUpdate.xy), then last frame's (giCounts.yz). The atlas written now held every
// probe two frames ago, so only last frame's window is stale in it: those groups copy
// it forward, skipping probes this frame relights. Inactive probes stay black. A
// texel's hit plane is relit across its footprint (giFootprintDirect); `pages` (a
// literal per entry point) tests the sun against virtual shadow pages first.
void giRelight(uint3 group,uint lane,bool pages){
    Frame f=frame();Look l=look(f);uint cursor=l.giUpdate.x,window=l.giUpdate.y,read=l.giImages.x;
    if(group.x>=window){
        uint id=l.giCounts.y+group.x-window;if(id>=l.giCounts.w || id-cursor<window)return;
        if(lane<36)tile[lane]=textures[NonUniformResourceIndex(read)].Load(int3(giTile(l,id,6)+uint2(lane%6,lane/6),0)).rgb;
        GroupMemoryBarrierWithGroupSync();storeTile(l,id,lane);return;
    }
    uint id=cursor+group.x;if(id>=l.giCounts.w)return;
    float4 meta=giMeta(l,id);
    if(meta.w<0.5){
        if(lane<36)tile[lane]=float3(0);
        seen[lane]=0;
        GroupMemoryBarrierWithGroupSync();storeTile(l,id,lane);storeRadiance(l,id,lane);return;
    }
    uint4 t=buffers[NonUniformResourceIndex(l.giBuffers.x)].Load<uint4>((id*64+lane)*16);
    float3 albedo=float3(t.x&255,(t.x>>8)&255,(t.x>>16)&255)/255.;float sky=float(t.x>>24)/255.;
    float3 normal=octDecode(float2(t.y&1023,(t.y>>10)&1023)/1023.);
    float3 emissive=float3(t.z&511,(t.z>>9)&511,(t.z>>18)&511)*exp2(float(t.z>>27)-24);float distance=halfToFloat(t.w>>16);
    float3 dir=octDecode((float2(lane%8,lane/8)+0.5)/8),origin=giProbePosition(l,id)+meta.xyz,light=0;
    if(sky<1){
        // The texel's relit hit is one of its sixteen bake sub-rays (ProbeSurfaces::texels).
        uint ray=(t.y>>20)&15;float3 toward=octDecode((float2(lane%8,lane/8)+(float2(ray%4,ray/4)+0.5)/4)/8);
        float3 hit=origin+toward*distance;
        float3 irradiance=giFootprintDirect(f,l,origin,uint2(lane%8,lane/8),octSolidAngle(uint2(lane%8,lane/8),8),t.w&65535,hit,normal,pages);
        // Last frame's probes close the loop: multiple bounces converge over frames.
        irradiance+=giIrradiance(l,read,hit,normal);
        light=(1-sky)*(emissive+albedo/PI*irradiance);
    }
    if(sky>0)light+=sky*skyLight(l)*(physicalSky(l)?skyRadiance(f,l,dir,false):envTex(f.extra.w,environmentUv(dir)).rgb);
    radiance[lane]=light;directions[lane]=float4(dir,octSolidAngle(uint2(lane%8,lane/8),8));
    // The radiance cache keeps what this texel sees, filtered over relights like the
    // irradiance (a reset, or a non-finite history, starts it over).
    {
        float hysteresis=(l.flags.y&LOOK_RESET)!=0?0:l.giParams.x;
        float4 value=float4(light,sky>0.5?GI_SKY_DISTANCE:giParallax(t.y));
        float4 old=images[NonUniformResourceIndex(l.giRadiance.y)][giTile(l,id,8)+uint2(lane%8,lane/8)];
        if(hysteresis>0 && all(isfinite(old)))value.rgb=lerp(value.rgb,old.rgb,hysteresis);
        seen[lane]=value;
    }
    GroupMemoryBarrierWithGroupSync();
    if(lane<36){
        float3 n=octDecode((float2(lane%6,lane/6)+0.5)/6),sum=0;float normalization=0;
        // Each texel weighs by its solid angle: octahedral texels near the axes cover
        // about half the mean, so equal weights would overcount light from axis-aligned
        // openings (a window in an axis-aligned room) up to twofold.
        for(uint j=0;j<64;j++){float w=max(dot(n,directions[j].xyz),0)*directions[j].w;sum+=radiance[j]*w;normalization+=w;}
        float3 value=sum*(PI/max(normalization,1e-5));
        float3 old=textures[NonUniformResourceIndex(read)].Load(int3(giTile(l,id,6)+uint2(lane%6,lane/6),0)).rgb;
        float hysteresis=(l.flags.y&LOOK_RESET)!=0?0:l.giParams.x;
        tile[lane]=lerp(value,old,hysteresis);
    }
    GroupMemoryBarrierWithGroupSync();storeTile(l,id,lane);storeRadiance(l,id,lane);
}
[shader("compute")][numthreads(64,1,1)]
void giRelightMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){giRelight(group,lane,false);}
// With virtual shadow maps (look::vsm): one page compare per hit.
[shader("compute")][numthreads(64,1,1)]
void giRelightVsmMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){giRelight(group,lane,true);}
// Two probe lobes per half-resolution pixel (the full-resolution pixel 2p): rgb
// irradiance facing the normal (w: camera distance) and the radiance the surface
// reflects along its specular lobe's dominant direction (specularDirection, as ibl
// looks it up; w: volume coverage). Lighting upsamples them depth-aware, so the
// per-pixel cost of the eight-probe visibility search drops fourfold.
[shader("compute")][numthreads(8,8,1)]
void giGatherMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 p=int2(tid.xy)*2;bool covered;float3 world=gbufferWorld(f,p,covered);float4 diffuse=float4(0,0,0,1e9),specular=0;
    if(covered){
        float3 n=normalize(gbufferNormal(f,p)),v=normalize(f.camera.xyz-world.xyz),a,b;
        float roughness=gbufferMaterial(f,p).x;
        float coverage=giPoolLobes(l,l.giImages.y,world.xyz,n,specularDirection(n,v,roughness),v,roughness,true,a,b);
        if(coverage>0){diffuse.rgb=a;specular=float4(b,coverage);}
        diffuse.w=length(world.xyz-f.camera.xyz);
    }
    images[NonUniformResourceIndex(l.giGather.z)][tid.xy]=diffuse;
    images[NonUniformResourceIndex(l.giGather.w)][tid.xy]=specular;
}
// Writes probe records (Renderer::update_probe_surfaces) into the surface cache,
// meta and moment atlas in place, one group per probe. Root words: first record,
// staging buffer (records of [probe id, 0, 0, 0], 64 cache texels as uint4, 66
// static vectors: 64 of packed 16x16 moments (look/gi.rs pack_moments), then
// float4s), cache, meta, moment atlas (storage, bordered 18x18 tiles of UNORM16
// moments), probes per atlas row with bit 31 set to clear instead (probe = record
// index: zero surfaces, an inactive probe and dark irradiance tiles), then both
// irradiance atlases.
static const uint SCATTER_RECORD=16+64*16+66*16;
[shader("compute")][numthreads(128,1,1)]
void giScatterMain(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    uint index=root.frame+group.x,staging=root.materials,base=index*SCATTER_RECORD,statics=base+16+64*16;
    bool clear=(root.mode>>31)!=0;uint atlas=root.mode&0x7fffffffu;
    uint id=clear?index:buffers[NonUniformResourceIndex(staging)].Load(base);
    if(lane<64)buffers[NonUniformResourceIndex(root.material)].Store<uint4>((id*64+lane)*16,clear?uint4(0,0,0,0):buffers[NonUniformResourceIndex(staging)].Load<uint4>(base+16+lane*16));
    if(lane==0)buffers[NonUniformResourceIndex(root.entity)].Store<float4>(id*16,clear?float4(0,0,0,0):buffers[NonUniformResourceIndex(staging)].Load<float4>(statics+64*16));
    for(uint i=lane;i<18*18;i+=128){
        // Border texels fold across the octahedral edges (DDGI borders, octIndex).
        uint t=octIndex(int2(i%18,i/18)-1,16),x=t%16,y=t/16;
        uint v=clear?0:buffers[NonUniformResourceIndex(staging)].Load(statics+((y/2)*8+x/2)*16+((y&1)*2+(x&1))*4);
        images[NonUniformResourceIndex(root.cascade)][uint2(id%atlas,id/atlas)*18+uint2(i%18,i/18)]=float4(float(v&65535)/65535,float(v>>16)/65535,0,0);
    }
    if(clear && lane<64){
        uint2 texel=uint2(id%atlas,id/atlas)*8+uint2(lane%8,lane/8);
        images[NonUniformResourceIndex(root.sample)][texel]=0;images[NonUniformResourceIndex(root.flags)][texel]=0;
    }
}
