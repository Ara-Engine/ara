// trace() test and benchmark kernels (ara_render look::trace tests and its ignored
// timing test). Root words: frame = scene record, materials = rays (two float4 per
// ray: origin and tMin, direction and tMax), material = results, entity = ray count,
// cascade = threads per dispatch row (dispatches past 65,535 groups use y), mode =
// ray flags, sample = ray mask, flags = cone spread (f32 bits).
#include "raster.slangh"
#include "trace.slangh"

TraceRay testRay(uint i){
    float4 a=asfloat(buffers[root.materials].Load4(i*32)),b=asfloat(buffers[root.materials].Load4(i*32+16));
    return traceRay(a.xyz,b.xyz,a.w,b.w,root.mode,root.sample,asfloat(root.flags));
}

// 32 bytes per ray: t (-1 on a miss), instance, primitive, submesh | front << 31,
// barycentrics, caller word, octahedral world normal (16:16).
[shader("compute")]
[numthreads(32,1,1)]
void traceTestMain(uint3 id:SV_DispatchThreadID){
    uint i=id.x+id.y*root.cascade;
    if(i>=root.entity)return;
    TraceScene s=traceScene(root.frame);
    TraceRay r=testRay(i);
    TraceHit h;bool found=traceClosest(s,r,h);
    float2 n=found?octEncode(traceHitNormal(s,h)):float2(0,0);
    uint2 q=uint2(saturate(n)*65535+0.5);
    buffers[root.material].Store4(i*32,uint4(asuint(found?h.t:-1.0),h.instance,h.primitive,h.submesh|(h.front?0x80000000u:0)));
    buffers[root.material].Store4(i*32+16,uint4(asuint(h.barycentrics.x),asuint(h.barycentrics.y),h.user,q.x|(q.y<<16)));
}

// The timed kernel: 16 bytes per ray (t, primitive, instance, submesh | front << 31),
// the R4-1 kernel's output width.
[shader("compute")]
[numthreads(32,1,1)]
void traceBenchMain(uint3 id:SV_DispatchThreadID){
    uint i=id.x+id.y*root.cascade;
    if(i>=root.entity)return;
    TraceScene s=traceScene(root.frame);
    TraceHit h;bool found=traceClosest(s,testRay(i),h);
    buffers[root.material].Store4(i*16,uint4(asuint(found?h.t:-1.0),h.primitive,h.instance,h.submesh|(h.front?0x80000000u:0)));
}
