[shader("compute")][numthreads(1,1,1)]
void hitMain() {
    RayDesc ray; ray.Origin=float3(1.5,0,-2); ray.Direction=float3(0,0,1); ray.TMin=0.001; ray.TMax=100;
    RayQuery<RAY_FLAG_FORCE_OPAQUE> query;
    query.TraceRayInline(scenes[root.scene], RAY_FLAG_NONE,255,ray);
    while(query.Proceed()) {}
    if(query.CommittedStatus()!=COMMITTED_TRIANGLE_HIT) {buffers[root.output].Store(0,0xffffffff);return;}
    uint instance=query.CommittedInstanceIndex(), primitive=query.CommittedPrimitiveIndex();
    AraHitTriangle tri=araHitTriangle(root.table,0,2,instance,primitive);
    // Keep this flat word contract explicit: Slang's Vulkan Store3 lowering uses
    // a std430 uint3 array (16-byte stride), rather than the packed 12-byte rows.
    buffers[root.output].Store(0,instance);
    buffers[root.output].Store(4,primitive);
    buffers[root.output].Store(8,tri.valid);
    for(uint i=0;i<3;i++) {
        buffers[root.output].Store(12+i*4,asuint(tri.a[i]));
        buffers[root.output].Store(24+i*4,asuint(tri.b[i]));
        buffers[root.output].Store(36+i*4,asuint(tri.c[i]));
    }
    buffers[root.output].Store(48,araHitTriangle(root.table,0,2,2,0).valid);
    buffers[root.output].Store(52,araHitTriangle(root.table,0,2,1,2).valid);
}
