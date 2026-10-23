struct Root{uint desc;uint table;uint pool;uint feedback;uint upload;uint count;uint pages;uint textures;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root:register(b0,space0);
#endif
BIND(1) RWByteAddressBuffer buffers[64]:register(u0,space2);
[shader("compute")][numthreads(64,1,1)]
void uploadMain(uint3 gid:SV_GroupID,uint lane:SV_GroupIndex) {
    if(gid.x>=root.count)return;
    uint packet=gid.x*(4+130*130)*4;uint4 header=buffers[NonUniformResourceIndex(root.upload)].Load4(packet);
    for(uint word=lane;word<130*130;word+=64)
        buffers[NonUniformResourceIndex(root.pool)].Store((header.y*130*130+word)*4,buffers[NonUniformResourceIndex(root.upload)].Load(packet+16+word*4));
    // Publication occurs in a separate dispatch after a whole-pool UAV barrier.
}
[shader("compute")][numthreads(64,1,1)]
void publishMain(uint3 id:SV_DispatchThreadID) {
    if(id.x>=root.count)return;uint4 h=buffers[NonUniformResourceIndex(root.upload)].Load4(id.x*(4+130*130)*4);
    if(h.z!=0xffffffff)buffers[NonUniformResourceIndex(root.table)].Store(h.z*4,0);
    buffers[NonUniformResourceIndex(root.table)].Store(h.x*4,h.y+1);
}
[shader("compute")][numthreads(64,1,1)]
void resetMain(uint3 id:SV_DispatchThreadID) {
    if(id.x<128)buffers[NonUniformResourceIndex(root.feedback)].Store(id.x*4,id.x>=16&&id.x<16+root.textures?0xffffffff:0);
}
