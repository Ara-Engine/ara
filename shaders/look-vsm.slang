// Virtual shadow requests and projection (look/vsm.rs). vsmMarkMain: one pixel of
// each 4x4 block (each in turn over 16 frames; a page spans thousands of pixels at its
// level) marks the page its lookup samples at its level and at the next coarser (the
// fallback while a page renders), plus the neighbours its filter reaches near a page's
// edge; its own dispatch, so whole waves mark and coalesce. vsmProjectMain: each
// lit-facing G-buffer pixel's sun visibility from the pages (-1 where none is
// resident) into the render-size mask lighting reads (vsmMask). Root word 0 is the
// frame record.
#include "raster.slangh"
#include "look.slangh"
#include "look-vsm.slangh"

// Sets the request bit of world page `page` at `level`: one lane per distinct page
// of the wave (its pixels mostly share pages; contended atomics cost more than the
// rest of the pass), after a plain load finds the bit unset.
void vsmMark(Look l,uint level,int2 page){
    uint index=vsmIndex(level,page);
    for(;;){
        if(WaveReadLaneFirst(index)!=index)continue;
        if(WaveIsFirstLane()){
            uint at=(index>>5)*4,bit=1u<<(index&31);
            if((buffers[NonUniformResourceIndex(l.vsmRequest.x)].Load(at)&bit)==0)
                buffers[NonUniformResourceIndex(l.vsmRequest.x)].InterlockedOr(at,bit);
        }
        break;
    }
}

// Marks the pages pixel `world` (light-space `light`) samples.
void vsmRequest(Frame f,Look l,float3 world,float3 light){
    uint level=vsmLevel(f,l,world,light);
    for(uint k=level;k<min(level+2,VSM_LEVELS);k++){
        float2 q=light.xy/vsmStep(l,k);int2 page=int2(floor(q));
        vsmMark(l,k,page);
        // The normal offset and filter reach under three texels past the edge.
        float2 local=(q-float2(page))*VSM_PAGE;
        int2 d=int2(local.x<3?-1:local.x>VSM_PAGE-3?1:0,local.y<3?-1:local.y>VSM_PAGE-3?1:0);
        if(d.x!=0)vsmMark(l,k,page+int2(d.x,0));
        if(d.y!=0)vsmMark(l,k,page+int2(0,d.y));
        if(all(d!=0))vsmMark(l,k,page+d);
    }
}

[shader("compute")][numthreads(8,8,1)]
void vsmMarkMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(l.vsmRequest.x==0)return;
    uint n=l.flags.z;int2 p=int2(tid.xy)*4+int2((n&1)*2+((n>>2)&1),((n>>1)&1)*2+((n>>3)&1));
    if(any(p>=int2(f.extent.xy)))return;
    float depth=gbufferDepth(f,p);if(depth<=0)return;
    // Surfaces facing away from the sun take no sun term (look-shading.slangh shade).
    if(dot(gbufferNormal(f,p),f.sunDirection.xyz)<=0)return;
    float3 world=worldAtDepth(f,float2(p)+0.5,depth);
    vsmRequest(f,l,world,vsmLight(l,world));
}

[shader("compute")][numthreads(8,8,1)]
void vsmProjectMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);int2 p=int2(tid.xy);
    if(any(p>=int2(f.extent.xy)))return;
    float visible=-1,depth=gbufferDepth(f,p);
    float3 normal=gbufferNormal(f,p);
    if(depth>0&&dot(normal,f.sunDirection.xyz)>0)
        visible=vsmVisibility(f,l,worldAtDepth(f,float2(p)+0.5,depth),normal,false);
    images[NonUniformResourceIndex(l.vsmMask.y)][p]=float4(visible,0,0,0);
}
