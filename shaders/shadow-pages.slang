// Sun page service passes (look/vsm/sun; opt-in, today's look-vsm.slang is the control).
// sunPagesMarkMain: one thread per 4x4 pixel block. The block's rotating sample pixel (one
// of 16 per frame, as look-vsm.slang) marks the pages its lookup reads: its level, and the
// rings two and four levels coarser (two is also the fallback while its level's page
// renders; four only where the page's blockers reach that far), each with the neighbours
// its window reaches near a page edge, and lowers their lowest-receiver words to its
// height (the invalidation bound's receiver term; look/vsm/sun unions them since a page
// was first seen, so every pixel counts within 16 frames). Waves gather their distinct
// pages and write each once. sunPagesProjectMain: each lit-facing pixel's source-shaped
// sun visibility from the
// pages (shadow-pages.slangh spRing: per-texel back-projection onto 256 fixed disc
// samples, rings at the pixel's level and two and four coarser as the blockers' height
// needs), -1 where no page is resident, into the mask lighting reads (vsmMask).
// Root word 0 is the frame record.
#include "raster.slangh"
#include "look.slangh"
#include "shadow-pages.slangh"

static const uint NO_MARK=0xffffffffu;
// Texel radius of a ring's window (8x8 texels around the receiver).
static const float SP_RING=3.5;
// A pixel's marks: four levels, each its page and the neighbours across x, y and the diagonal.
struct SpMarks {uint4 levels[4];};

// Level `k`'s page of world `p` (current shear) and the neighbours within `reach` texels
// of its edges; NO_MARK past the clipmap.
uint4 spLevelMarks(Look l,float3 p,uint k,float reach){
    if(k>=SP_LEVELS)return NO_MARK;
    float width=spStep(l,k);float2 q=spGround(l,p,spShear(l))/width;int2 page=int2(floor(q));
    float2 local=(q-float2(page))*SP_PAGE;
    int2 d=int2(local.x<reach?-1:local.x>SP_PAGE-reach?1:0,local.y<reach?-1:local.y>SP_PAGE-reach?1:0);
    return uint4(spIndex(k,page),d.x!=0?spIndex(k,page+int2(d.x,0)):NO_MARK,
        d.y!=0?spIndex(k,page+int2(0,d.y)):NO_MARK,all(d!=0)?spIndex(k,page+d):NO_MARK);
}
// Request bits and lowest receivers (`word`: the block's encoded lowest receiver) of each
// distinct page of the wave's marks, the smallest pending index first; one lane writes.
void spMarkAll(Look l,SpMarks m,uint word){
    uint base=l.vsmRequest.x;
    for(;;){
        uint4 a=min(m.levels[0],m.levels[1]),b=min(m.levels[2],m.levels[3]),c=min(a,b);
        uint index=WaveActiveMin(min(min(c.x,c.y),min(c.z,c.w)));
        if(index==NO_MARK)break;
        bool mine=any(m.levels[0]==index)||any(m.levels[1]==index)||any(m.levels[2]==index)||any(m.levels[3]==index);
        uint best=WaveActiveMax(mine?word:0);
        if(WaveIsFirstLane()){
            uint at=(index>>5)*4,bit=1u<<(index&31);
            if((buffers[NonUniformResourceIndex(base)].Load(at)&bit)==0)
                buffers[NonUniformResourceIndex(base)].InterlockedOr(at,bit);
            uint r=(SP_BIT_WORDS+index)*4;
            if(buffers[NonUniformResourceIndex(base)].Load(r)<best)
                buffers[NonUniformResourceIndex(base)].InterlockedMax(r,best);
        }
        [unroll]for(uint i=0;i<4;i++)m.levels[i]=select(m.levels[i]==index,NO_MARK,m.levels[i]);
    }
}
[shader("compute")][numthreads(8,8,1)]
void sunPagesMarkMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(l.vsmRequest.x==0)return;
    uint n=l.flags.z;int2 p=int2(tid.xy)*4+int2((n&1)*2+((n>>2)&1),((n>>1)&1)*2+((n>>3)&1));
    SpMarks m;[unroll]for(uint i=0;i<4;i++)m.levels[i]=NO_MARK;
    uint word=0;
    if(all(p<int2(f.extent.xy))){
        float depth=gbufferDepth(f,p);
        float3 normal=gbufferNormal(f,p);float nl=dot(normal,f.sunDirection.xyz);
        if(depth>0&&nl>0){
            float3 world=worldAtDepth(f,float2(p)+0.5,depth);
            uint level=spLevel(f,l,world,spGround(l,world,spShear(l)));
            float shift=shadowOffset(nl)*sqrt(saturate(1-nl*nl));
            m.levels[0]=spLevelMarks(l,world,level,shift+SP_RING+1);
            m.levels[2]=spLevelMarks(l,world,level+2,SP_RING+1);
            // The outer ring only where the level's page says its blockers reach past ring two.
            int slot=spSlot(l,level,int2(floor(spGround(l,world,spShear(l))/spStep(l,level))));
            float top=slot>=0?spRecord(l,slot).z:1e30;
            float2 k=spShear(l);
            if((top-world.y)*tan(max(l.sunLux.w,1e-5))*(1+dot(k,k))>SP_RING*spStep(l,level+2)/SP_PAGE)
                m.levels[3]=spLevelMarks(l,world,level+4,SP_RING+1);
            // (tag << 16) | (65535 - floor(65535 depth)): the newest frame's lowest wins a max.
            uint q=uint(floor(saturate(spDepth(l,world.y))*65535));
            word=((l.vsmRequest.w>>16)<<16)|(65535-q);
        }
    }
    spMarkAll(l,m,word);
}

// Source-shaped sun visibility of world `world` (normal `normal`) from the pages: the
// receiver point (normal-offset by the pixel level's texel) against rings of blocker
// texels, each back-projected onto the 256 fixed disc samples (spRing), so blockers at
// different heights each hide their own samples and the per-sample results combine before
// the samples average. Ring one is the pixel's level (screen-matched texels at contact);
// rings two and four levels coarser add the blockers whose height reaches further, as far
// as the page's top requires. -1 where the pixel's level and the next three hold no page.
// `reach`: how far (texels of the level read) the tallest blocker's sample rays spread
// (diagnostic, the mask's second channel).
float spVisibility(Frame f,Look l,SpDisc disc,float3 world,float3 normal,out float reach){
    reach=SP_RING+1;if(l.vsm.y==0)return -1;
    float nl=saturate(dot(normal,f.sunDirection.xyz));
    uint first=spLevel(f,l,world,spGround(l,world,spShear(l)));
    for(uint level=first;level<min(first+4,SP_LEVELS);level++){
        float texel=spStep(l,level)/SP_PAGE;
        float3 p=world+normal*texel*shadowOffset(nl);
        SpPage s=spPage(l,level,p);
        if(s.slot<0)continue;
        // No caster above the receiver anywhere in the page: lit.
        if(s.top<=p.y+texel)return 1;
        // The receiver plane's height gradient in the page's ground frame (clamped at grazing).
        float dn=max(dot(normal,float3(s.k.x,1,s.k.y)),0.05);float2 g=clamp(-normal.xz/dn,-8,8);
        // Ground distance the tallest blocker's sample rays reach from the receiver.
        float spread=(s.top-p.y)*(length(abs(disc.centre-s.k)+disc.half));
        uint4 m0=0,m1=0;
        spRing(l,s,disc,p.y,g,texel,0,m0,m1);
        float covered=SP_RING*texel;
        uint4 v0=disc.valid0,v1=disc.valid1;
        // Umbra: every sample hidden already.
        [loop]for(uint ring=1;ring<3&&spread>covered&&spCount(m0&v0,m1&v1)<uint(disc.count);ring++){
            uint coarse=level+2*ring;if(coarse>=SP_LEVELS)break;
            SpPage c=spPage(l,coarse,p);if(c.slot<0)break;
            float2 cg=clamp(-normal.xz/max(dot(normal,float3(c.k.x,1,c.k.y)),0.05),-8,8);
            spRing(l,c,disc,p.y,cg,texel,covered,m0,m1);
            covered=SP_RING*c.texel;
        }
        reach=clamp(spread/texel,1,64);
        return 1-float(spCount(m0&v0,m1&v1))/disc.count;
    }
    return -1;
}

// The frame's disc sample mask, built once per workgroup (four cells per thread).
groupshared uint gsValid[8];

[shader("compute")][numthreads(8,8,1)]
void sunPagesProjectMain(uint3 tid:SV_DispatchThreadID,uint gi:SV_GroupIndex){
    Frame f=frame();Look l=look(f);int2 p=int2(tid.xy);
    SpDisc disc=spDiscFrame(f,l);
    if(gi<8)gsValid[gi]=0;
    GroupMemoryBarrierWithGroupSync();
    uint bits=0;
    [unroll]for(uint c=0;c<4;c++)if(spInside(disc,gi*4+c))bits|=1u<<((gi*4+c)&31);
    InterlockedOr(gsValid[gi/8],bits);
    GroupMemoryBarrierWithGroupSync();
    disc.valid0=uint4(gsValid[0],gsValid[1],gsValid[2],gsValid[3]);
    disc.valid1=uint4(gsValid[4],gsValid[5],gsValid[6],gsValid[7]);
    disc.count=max(float(spCount(disc.valid0,disc.valid1)),1);
    if(any(p>=int2(f.extent.xy)))return;
    float visible=-1,reach=SP_RING+1,depth=gbufferDepth(f,p);
    float3 normal=gbufferNormal(f,p);
    if(depth>0&&dot(normal,f.sunDirection.xyz)>0)
        visible=spVisibility(f,l,disc,worldAtDepth(f,float2(p)+0.5,depth),normal,reach);
    images[NonUniformResourceIndex(l.vsmMask.y)][p]=float4(visible,reach,0,0);
}
