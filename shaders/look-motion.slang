// Motion blur tiles (look/passes.rs `motion_passes`, look-motion.slangh): pass 0
// (root word 2) takes each 16-pixel display tile's largest blur vector, pass 1 the
// largest of its 3x3 neighbourhood, which the post pass's motion variant gathers along.
#include "raster.slangh"
#include "look.slangh"
#include "look-motion.slangh"
void tileMax(Frame f,Look l,uint2 tile){
    float2 best=0;int2 origin=int2(tile*MOTION_TILE);
    for(uint y=0;y<MOTION_TILE;y+=2)for(uint x=0;x<MOTION_TILE;x+=2){
        int2 p=min(origin+int2(x,y),int2(f.extent.xy)-1);float2 v=motionAt(f,l,p);if(dot(v,v)>dot(best,best))best=v;
    }
    images[NonUniformResourceIndex(l.motionImages.z)][tile]=float4(best,0,1);
}
void neighbourMax(Look l,uint2 tile,uint2 tiles){
    float2 best=0;
    for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
        int2 q=clamp(int2(tile)+int2(x,y),int2(0),int2(tiles)-1);float2 v=pixel(l.motionImages.x,q).xy;if(dot(v,v)>dot(best,best))best=v;
    }
    images[NonUniformResourceIndex(l.motionImages.w)][tile]=float4(best,0,1);
}
[shader("compute")][numthreads(8,8,1)]
void motionMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 tiles=(f.extent.xy+MOTION_TILE-1)/MOTION_TILE;
    if(any(tid.xy>=tiles))return;
    if(root.material==0)tileMax(f,l,tid.xy);else neighbourMax(l,tid.xy,tiles);
}
