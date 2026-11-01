// Shadow moment pyramids (look/moments.rs; layout in look-shadow.slangh): one level of
// one tile per dispatch, a thread per texel. Level 0 reads the tile's 2x2 depth texels;
// level k the 2x2 texels of level k-1 it covers. Each texel holds its depths' mean and
// their variance about it, combined exactly (the children's variances plus their
// means' spread), so fp32 keeps centimetres at any map depth range, and the farthest
// depth from the light (the smallest stored).
// Root words: frame = the depth map (sampled), materials = the pyramid (storage),
// material = the level, entity = the tile's texel origin in the map the pyramid mirrors
// (x | y << 16), cascade = the tile's side (texels), mode = its origin in the depth map
// (x | y << 16), sample = the pyramid's level-0 size (x | y << 16).
#include "raster.slangh"

uint2 unpack2(uint v){return uint2(v&0xffffu,v>>16);}
// Texel origin of level `level` in the packed pyramid (look-shadow.slangh shadowLevelOrigin).
uint2 levelOrigin(uint2 size,uint level){return level==0?0:uint2(size.x,size.y-(size.y>>(level-1)));}

[shader("compute")][numthreads(8,8,1)]
void shadowMomentsMain(uint3 tid:SV_DispatchThreadID){
    uint level=root.material,side=root.cascade>>(level+1);if(any(tid.xy>=side))return;
    uint2 tile=unpack2(root.entity),size=unpack2(root.sample);
    float4 mu,v=0,lo;
    if(level==0){
        int2 at=int2(unpack2(root.mode)+tid.xy*2);
        [unroll]for(uint k=0;k<4;k++)mu[k]=textures[NonUniformResourceIndex(root.frame)].Load(int3(at+int2(k&1,k>>1),0)).r;
        lo=mu;
    }else{
        int2 at=int2(levelOrigin(size,level-1)+(tile>>level)+tid.xy*2);
        [unroll]for(uint k=0;k<4;k++){float4 t=images[NonUniformResourceIndex(root.materials)][at+int2(k&1,k>>1)];mu[k]=t.r;v[k]=t.g;lo[k]=t.b;}
    }
    float mean=dot(mu,0.25);float4 spread=mu-mean;
    float variance=dot(v,0.25)+dot(spread*spread,0.25);
    images[NonUniformResourceIndex(root.materials)][int2(levelOrigin(size,level)+(tile>>(level+1))+tid.xy)]=float4(mean,variance,min(min(lo.x,lo.y),min(lo.z,lo.w)),0);
}
