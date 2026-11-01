// The near froxel volume of volumetric fog (look/volumetric.rs): injection with
// temporal reprojection, then front-to-back integration per column. Atlases hold one
// tile of volumetricGrid.xy texels per slice (look.slangh nearTile).
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "look-fog.slangh"
// Radical inverse of `i` in base `b`: the Halton sequence's deterministic jitter.
float halton(uint i,uint b){float f=1,r=0;[loop]while(i>0){f/=float(b);r+=f*float(i%b);i/=b;}return r;}
// World position of froxel coordinate `c` (x, y in froxels, z in slices) and the
// normalised view ray through it.
float3 froxelWorld(Frame f,Look l,float3 c,out float3 ray){
    ray=viewRay(f,c.xy*float(l.volumetricGrid.w));
    float depth=nearDepth(l,c.z/float(l.volumetricGrid.z));
    return f.camera.xyz+ray*(depth/max(dot(ray,f.forward.xyz),1e-3));
}
// Trilinear tap of a slice atlas (`grid` cells, `columns` slices per atlas row) at
// cell coordinates `c` (centres at +0.5): two bilinear taps clamped inside their
// slices, blended across them.
float4 atlasTap(uint image,uint3 grid,uint columns,float3 c){
    float2 size=float2(grid.xy)*float2(columns,(grid.z+columns-1)/columns),xy=clamp(c.xy,0.5,float2(grid.xy)-0.5);
    float z=clamp(c.z-0.5,0.,float(grid.z-1));uint lo=uint(z),hi=min(lo+1,grid.z-1);
    float4 a=lut(image,(float2(uint2(lo%columns,lo/columns)*grid.xy)+xy)/size),b=lut(image,(float2(uint2(hi%columns,hi/columns)*grid.xy)+xy)/size);
    return lerp(a,b,frac(z));
}
// Last frame's volume at world point `p` through the unjittered previous
// view-projection. False outside it (or behind the previous camera), where the froxel
// starts over.
bool froxelHistory(Frame f,Look l,float3 p,out float4 history){
    history=0;float4 clip=mul(f.previousVp,float4(p,1));if(clip.w<=0)return false;
    uint3 grid=l.volumetricGrid.xyz;float2 uv=float2(clip.x/clip.w*0.5+0.5,0.5-clip.y/clip.w*0.5);
    float3 c=float3(uv*float2(f.extent.xy)/float(l.volumetricGrid.w),nearSlice(l,clip.w)*grid.z);
    if(any(c<0)||any(c>float3(grid)))return false;
    history=atlasTap(l.volumetricImages.x,grid,l.volumetricAtlas.x,c);
    return true;
}
// One thread per ambient cell (4x4x2 froxels; look/volumetric.rs AMBIENT_CELL): the
// mean radiance fog receives besides the sun and local lights (fogAmbient: probe GI or
// the sky) at the cell's centre. Smooth light, so the coarse grid loses nothing.
[shader("compute")][numthreads(8,8,1)]
void fogAmbientMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint3 grid=l.volumetricAmbient.xyz;if(any(tid>=grid))return;
    float3 unused;float3 p=froxelWorld(f,l,(float3(tid)+0.5)*float3(4,4,2),unused);
    uint columns=l.volumetricAmbient.w;
    images[NonUniformResourceIndex(l.volumetricStorage.y)][uint2(tid.z%columns,tid.z/columns)*grid.xy+tid.xy]=float4(fogAmbient(f,l,p),1);
}
// Local lights touching one injection group's froxels (8x8 across, one slice), a bit
// per light: walking set bits keeps the summation order fixed (deterministic), and
// each froxel visits only the lights that can reach it.
groupshared uint fogGroupLights[8];
groupshared float3 fogGroupMin,fogGroupMax;

// One thread per froxel: in-scattered light per metre (rgb, emission included) and
// grey extinction (a) at a point jittered by the frame's Halton offset (the same for
// every froxel, so the volume stays coherent while history integrates the offsets),
// blended into the reprojected history by volumetric.z (zero after a reset; an even
// average over the first frames, then 0.9). The sun is the lit share of two points
// half a froxel apart; local lights are culled per group against its froxels' bounds.
[shader("compute")][numthreads(8,8,1)]
void fogInjectMain(uint3 tid:SV_DispatchThreadID,uint3 group:SV_GroupID,uint index:SV_GroupIndex){
    Frame f=frame();Look l=look(f);uint3 grid=l.volumetricGrid.xyz;bool shadowed=(l.flags.y&LOOK_FOG_SHADOWED)!=0;
    if(index<8)fogGroupLights[index]=0;
    if(index==0){
        float3 lo=1e30,hi=-1e30;
        for(uint k=0;k<8;k++){float3 unused;float3 w=froxelWorld(f,l,float3(group.xy*8+uint2(k&1,(k>>1)&1)*8,group.z+(k>>2)),unused);lo=min(lo,w);hi=max(hi,w);}
        fogGroupMin=lo;fogGroupMax=hi;
    }
    GroupMemoryBarrierWithGroupSync();
    for(uint i=index;i<(shadowed?f.extent.z:0);i+=64){
        // Bounding spheres about the centre hold every kind (look-lights.slangh).
        Light light=loadLight(f.lighting.w,i);float4 sphere=light.positionRange;
        if(light.parameters.w>0 && length(clamp(sphere.xyz,fogGroupMin,fogGroupMax)-sphere.xyz)<=sphere.w)InterlockedOr(fogGroupLights[i/32],1u<<(i%32));
    }
    GroupMemoryBarrierWithGroupSync();
    if(any(tid>=grid))return;
    uint n=l.volumetricAtlas.y%16+1;float3 jitter=float3(halton(n,2),halton(n,3),halton(n,5));
    float3 d;float3 p=froxelWorld(f,l,float3(tid)+jitter,d);
    FogMedium m=fogMedium(f,l,p);
    bool physical=physicalSky(l);
    float height=max(p.y-l.atmosphere.x,0);float3 sr=0;float sm=0;float e=m.extinction;
    // Clear air's extinction is near grey over the near field (under 1% at 100 m).
    if(physical)e+=dot(extinction(height,l.look.w,sr,sm),float3(0.2126,0.7152,0.0722));
    float3 sun=f.sunDirection.xyz;float cosine=dot(d,sun),g=l.fog.w;
    float lit=1;
    if(shadowed){float3 unused;lit=0.5*(fogVisibility(f,l,p)+fogVisibility(f,l,froxelWorld(f,l,float3(tid)+frac(jitter+0.5),unused)));}
    // Glass and films tint the shafts behind them (look-translucent.slangh).
    float3 source=sunAt(f,l,p)*lit*translucentSun(l,p)*(sr*phaseR(cosine)+sm*phaseM(cosine,0.8)+m.scattering*(phaseM(cosine,g)*l.volumetricEmissive.w));
    if(physical)source+=(sr+sm)*multiScattering(l,height,sun.y)*l.sunLux.rgb;
    if(any(m.scattering>0)){
        // Ambient light from its coarse grid (fogAmbientMain), at the jittered point.
        float3 cell=(float3(tid)+jitter)/float3(4,4,2);
        source+=m.scattering*atlasTap(l.volumetricStorage.z,l.volumetricAmbient.xyz,l.volumetricAmbient.w,cell).rgb;
        for(uint word=0;word<8;word++){
            uint bits=fogGroupLights[word];
            while(bits!=0){
                uint i=word*32+firstbitlow(bits);bits&=bits-1;
                source+=m.scattering*fogLocalLight(f,l,loadLight(f.lighting.w,i),i,p,d,g);
            }
        }
    }
    float4 current=float4(source+m.emission,e);
    float weight=l.volumetric.z;float4 history;float3 unused;
    if(weight>0 && froxelHistory(f,l,froxelWorld(f,l,float3(tid)+0.5,unused),history))current=lerp(current,history,weight);
    if(any(isnan(current))||any(isinf(current)))current=0;
    images[NonUniformResourceIndex(l.volumetricImages.w)][nearTile(l,tid.z)+tid.xy]=current;
}
// One thread per column: Hillaire's energy-conserving integral of each slice front to
// back, storing in-scatter (rgb) and transmittance (a) at every slice's far end.
[shader("compute")][numthreads(8,8,1)]
void fogIntegrateMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint3 grid=l.volumetricGrid.xyz;if(any(tid.xy>=grid.xy))return;
    float3 ray=viewRay(f,(float2(tid.xy)+0.5)*float(l.volumetricGrid.w));float stretch=1/max(dot(ray,f.forward.xyz),1e-3);
    float3 light=0;float trans=1,previous=0;
    for(uint z=0;z<grid.z;z++){
        float end=nearDepth(l,float(z+1)/grid.z),ds=(end-previous)*stretch;previous=end;
        uint2 at=nearTile(l,z)+tid.xy;float4 s=pixel(l.volumetricImages.y,int2(at));
        light+=trans*s.rgb*segmentWeight(s.a,ds);trans*=exp(-s.a*ds);
        images[NonUniformResourceIndex(l.volumetricStorage.x)][at]=float4(light,trans);
    }
}
