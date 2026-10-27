// Virtual shadow requests and projection (look/vsm.rs). vsmMarkMain: one pixel of
// each 4x4 block (each in turn over 16 frames; a page spans thousands of pixels at its
// level) marks the page its lookup samples at its level and at the next coarser (the
// fallback while a page renders, and the blocker search's level), plus the neighbours
// its filters reach near a page's edge; its own dispatch, so whole waves gather their
// distinct pages and request each once. vsmProjectMain: each lit-facing G-buffer pixel's contact-hardening sun
// visibility from the pages (vsmSoftVisibility; -1 where none is resident) and its
// tent's half-width into the render-size mask lighting reads (vsmMask). Root word 0 is
// the frame record.
#include "raster.slangh"
#include "look.slangh"
#include "look-vsm.slangh"
#include "look-translucent.slangh"

// Widest tent (half-width, texels at the pixel's level) the contact-hardening filter
// uses; wider penumbrae clamp to it (about four to eight render pixels).
static const float VSM_SOFT_TEXELS=4;
// A pixel's page marks (vsmRequest): per level its own page, then the neighbours across
// x, y and the diagonal; NO_MARK where none.
static const uint NO_MARK=0xffffffffu;
struct VsmMarks {uint4 levels[2];};
// Sets each distinct page of the wave's marks once: the smallest pending index first,
// one lane setting its request bit after a plain load finds it unset, every lane
// clearing it from its slots (the wave's pixels mostly share pages; contended atomics
// cost more than the rest of the pass, and one loop over the wave's distinct pages
// beats a waterfall per mark).
void vsmMarkAll(Look l,VsmMarks m){
    for(;;){
        uint4 low=min(m.levels[0],m.levels[1]);
        uint index=WaveActiveMin(min(min(low.x,low.y),min(low.z,low.w)));
        if(index==NO_MARK)break;
        if(WaveIsFirstLane()){
            uint at=(index>>5)*4,bit=1u<<(index&31);
            if((buffers[NonUniformResourceIndex(l.vsmRequest.x)].Load(at)&bit)==0)
                buffers[NonUniformResourceIndex(l.vsmRequest.x)].InterlockedOr(at,bit);
        }
        m.levels[0]=select(m.levels[0]==index,NO_MARK,m.levels[0]);
        m.levels[1]=select(m.levels[1]==index,NO_MARK,m.levels[1]);
    }
}
// Last frame's tent half-width at render pixel `p` (vsmProjectMain): the soft filter's
// reach there; one texel before the mask holds one.
float vsmLastTent(Look l,int2 p){
    if(l.vsmMask.x==0)return 1;
    float t=textures[NonUniformResourceIndex(l.vsmMask.x)].Load(int3(p,0)).g;return t>=1&&t<=VSM_SOFT_TEXELS?t:1;
}
// The pages pixel `world` (light-space `light`, its normal at cosine `nl` to the
// sun, last frame's tent `tent`) samples, and the neighbours within the projection's
// reach of a page's edge: the normal offset's lateral shift plus, at its level, the
// tent and its gather, and one level coarser the blocker window (2.5 texels).
VsmMarks vsmRequest(Frame f,Look l,float3 world,float3 light,float nl,float tent){
    uint level=vsmLevel(f,l,world,light);float shift=shadowOffset(nl)*sqrt(saturate(1-nl*nl));
    VsmMarks m;m.levels[0]=NO_MARK;m.levels[1]=NO_MARK;
    [unroll]for(uint i=0;i<2;i++){
        uint k=level+i;if(k>=VSM_LEVELS)break;
        float2 q=light.xy/vsmStep(l,k);int2 page=int2(floor(q));
        float reach=i==0?shift+tent+1:shift*0.5+2.5;
        float2 local=(q-float2(page))*VSM_PAGE;
        int2 d=int2(local.x<reach?-1:local.x>VSM_PAGE-reach?1:0,local.y<reach?-1:local.y>VSM_PAGE-reach?1:0);
        m.levels[i]=uint4(vsmIndex(k,page),d.x!=0?vsmIndex(k,page+int2(d.x,0)):NO_MARK,
            d.y!=0?vsmIndex(k,page+int2(0,d.y)):NO_MARK,all(d!=0)?vsmIndex(k,page+d):NO_MARK);
    }
    return m;
}

[shader("compute")][numthreads(8,8,1)]
void vsmMarkMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(l.vsmRequest.x==0)return;
    uint n=l.flags.z;int2 p=int2(tid.xy)*4+int2((n&1)*2+((n>>2)&1),((n>>1)&1)*2+((n>>3)&1));
    if(any(p>=int2(f.extent.xy)))return;
    float depth=gbufferDepth(f,p);if(depth<=0)return;
    // Surfaces facing away from the sun take no sun term (look-shading.slangh shade).
    float nl=dot(gbufferNormal(f,p),f.sunDirection.xyz);if(nl<=0)return;
    float3 world=worldAtDepth(f,float2(p)+0.5,depth);
    vsmMarkAll(l,vsmRequest(f,l,world,vsmLight(l,world),nl,vsmLastTent(l,p)));
}

// A page lookup for the soft filters: world page `page` (in `slot`; -1 where not
// resident) at `level`, the receiver's page-local texel position `c` (texel i's centre
// at i, rows from the light +y edge), its biased stored depth, and its plane's stored
// depth change per texel step along x and down the rows.
struct VsmSoftTap {uint level;int2 page;int slot;float2 c;float depth;float2 slope;float texel;};
// The lookup of light-space receiver `q` at `level`; `plane` is the receiver plane's
// light z gradient (per metre of light x and y).
VsmSoftTap vsmSoftTap(Look l,uint level,float3 q,float2 plane){
    VsmSoftTap t;t.level=level;float width=vsmStep(l,level);t.texel=width/VSM_PAGE;
    t.page=int2(floor(q.xy/width));t.slot=vsmSlot(l,level,t.page);
    float2 local=q.xy/width-float2(t.page);t.c=float2(local.x,1-local.y)*VSM_PAGE-0.5;
    // One texel of depth bias at this level (as vsmVisibility); rows run towards light -y.
    t.depth=l.vsmDepth.x*(q.z+t.texel)+l.vsmDepth.y;t.slope=l.vsmDepth.x*t.texel*float2(plane.x,-plane.y);
    return t;
}
// Raw depths of the 2x2 texels around page-local boundary `g` (texels g-1 and g on
// each axis) of `t`'s page, in the order (g-1,g-1), (g,g-1), (g-1,g), (g,g): one gather
// inside the page, texel loads across its edge (vsmTexel: the neighbouring page, or
// clamped into this one where that is not resident).
float4 vsmDepths(Look l,VsmSoftTap t,int2 g){
    if(all(g>=1)&&all(g<=int(VSM_PAGE)-1)){
        uint s=uint(t.slot),cols=l.vsm.w;
        float2 origin=float2(s%cols,s/cols)*VSM_PAGE,atlas=float2(cols,l.vsmRequest.z)*VSM_PAGE;
        return textures[NonUniformResourceIndex(l.vsm.y)].GatherRed(samplers[2],(origin+float2(g))/atlas).wzxy;
    }
    float4 d=0;
    [loop]for(uint k=0;k<4;k++){
        float v=vsmTexel(l,t.level,t.page,t.slot,g-1+int2(k&1,k>>1));
        d=float4(k==0?v:d.x,k==1?v:d.y,k==2?v:d.z,k==3?v:d.w);
    }
    return d;
}
// Mean height (m) above the receiver plane of the texels hiding part of the sun's disc
// from the receiver (`tangent`: tan of its angular radius): those within their height
// times `tangent`, plus a texel and a half of footprint, laterally, inside a window of
// a texel and a half around it (the 4x4 texels of four gathers). Weights fade over a
// texel at that cone, at the window's edge and at the plane, so the estimate (and the
// penumbra it sets) stays continuous as receivers move; the mean fades to zero with the
// weight found. `blocked`: how many of the 16 texels rise above the plane.
float vsmBlockers(Look l,VsmSoftTap t,float tangent,out float blocked){
    int2 r=int2(floor(t.c+0.5));float heights=0,weights=0,texels=1/(l.vsmDepth.x*t.texel);blocked=0;
    [loop]for(uint k=0;k<4;k++){
        int2 g=r+int2(int(k&1)*2-1,int(k>>1)*2-1);float4 d=vsmDepths(l,t,g);
        float2 ox=float2(g.x-1,g.x)-t.c.x,oy=float2(g.y-1,g.y)-t.c.y;
        float4 x=float4(ox.x,ox.y,ox.x,ox.y),y=float4(oy.x,oy.x,oy.y,oy.y);
        // Height above the receiver's biased plane at each texel, in texels.
        float4 h=(d-(t.depth+t.slope.x*x+t.slope.y*y))*texels;
        float4 w=saturate(h*tangent+1.5-sqrt(x*x+y*y))*saturate(1.5-abs(x))*saturate(1.5-abs(y))*saturate(h);
        heights+=dot(w,h);weights+=dot(w,1);blocked+=dot(float4(h>0),1);
    }
    return weights>0?heights/weights*t.texel*saturate(weights):0;
}
// Tent-filtered sun visibility at `t` (half-width `radius` texels; one is a single
// bilinear compare), each texel compared with the receiver's plane, so wide kernels do
// not shadow their own surface. Gathers cover the tent's texels: one at a radius of
// one, up to 5x5 at VSM_SOFT_TEXELS.
float vsmTent(Look l,VsmSoftTap t,float radius){
    int2 first=int2(floor(t.c-radius))+1,last=int2(ceil(t.c+radius))-1;
    float sum=0,total=0;
    for(int gy=first.y+1;gy<=last.y+1;gy+=2){
        float2 oy=float2(gy-1,gy)-t.c.y,wy=max(radius-abs(oy),0);
        for(int gx=first.x+1;gx<=last.x+1;gx+=2){
            float2 ox=float2(gx-1,gx)-t.c.x,wx=max(radius-abs(ox),0);
            float4 d=vsmDepths(l,t,int2(gx,gy));
            float4 x=float4(ox.x,ox.y,ox.x,ox.y),y=float4(oy.x,oy.x,oy.y,oy.y);
            float4 w=float4(wx.x*wy.x,wx.y*wy.x,wx.x*wy.y,wx.y*wy.y);
            sum+=dot(w,step(d,t.depth+t.slope.x*x+t.slope.y*y));total+=dot(w,1);
        }
    }
    return total>0?sum/total:1;
}
// Contact-hardening sun visibility from the virtual shadow map (PCSS, Fernando 2005,
// for the sun's disc: angular radius Look.sunLux.w, as the path tracer's sun). Blockers
// searched one level coarser (this level where that is not resident) set the penumbra
// half-width, their mean height times tan(radius), which a tent of compares at the
// pixel's level filters (`tent` half-width: one texel without blockers, a single
// bilinear compare; VSM_SOFT_TEXELS at most). Where every searched texel and the
// pixel's own compares block, it is in umbra. Penumbrae stay sharp at contact and widen
// with the occluder's height; every pattern is fixed, so the mask is smooth and stable
// without temporal accumulation. Levels and the slope-scaled normal offset follow
// vsmVisibility; -1 where no page is resident or the map is off.
float vsmSoftVisibility(Frame f,Look l,float3 world,float3 normal,out float tent){
    tent=1;if(l.vsm.y==0)return -1;
    float nl=saturate(dot(normal,f.sunDirection.xyz));
    // The receiver plane's light z gradient (light z points at the sun), clamped near grazing.
    float3 n=mul(l.vsmSpace,float4(normal,0)).xyz;float2 plane=clamp(-n.xy/max(n.z,0.05),-8,8);
    float tangent=tan(l.sunLux.w);
    uint first=vsmLevel(f,l,world,vsmLight(l,world));
    for(uint level=first;level<min(first+4,VSM_LEVELS);level++){
        float texel=vsmStep(l,level)/VSM_PAGE;
        float3 q=vsmLight(l,world+normal*texel*shadowOffset(nl));
        VsmSoftTap t=vsmSoftTap(l,level,q,plane);
        if(t.slot<0)continue;
        VsmSoftTap search=t;
        if(level+1<VSM_LEVELS){VsmSoftTap coarse=vsmSoftTap(l,level+1,q,plane);if(coarse.slot>=0)search=coarse;}
        float blocked,penumbra=vsmBlockers(l,search,tangent,blocked)*tangent/texel;
        tent=min(sqrt(penumbra*penumbra+1),VSM_SOFT_TEXELS);
        // In umbra a bilinear compare first: where it is dark too, so is the tent.
        float v=0;
        for(uint pass=blocked>=16?0:1;pass<2;pass++){v=vsmTent(l,t,pass==0?1:tent);if(v<=0)break;}
        return v;
    }
    return -1;
}

[shader("compute")][numthreads(8,8,1)]
void vsmProjectMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);int2 p=int2(tid.xy);
    if(any(p>=int2(f.extent.xy)))return;
    float visible=-1,tent=1,depth=gbufferDepth(f,p);
    float3 normal=gbufferNormal(f,p);
    if(depth>0&&dot(normal,f.sunDirection.xyz)>0){
        float3 world=worldAtDepth(f,float2(p)+0.5,depth);visible=vsmSoftVisibility(f,l,world,normal,tent);
        // Sun through blended casters (look-translucent.slangh); -1 stays unresolved.
        if(visible>0)visible*=translucentSun(l,world);
    }
    images[NonUniformResourceIndex(l.vsmMask.y)][p]=float4(visible,tent,0,0);
}
