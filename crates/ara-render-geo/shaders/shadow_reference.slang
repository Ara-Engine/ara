struct Root{uint acceleration;uint world;uint normal;uint output;uint width;uint height;uint parameters;uint reserved;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root:register(b0,space0);
#endif
BIND(0) Texture2D<float4> textures[128]:register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[64]:register(u0,space2);
BIND(3) RWTexture2D<float4> images[64]:register(u0,space3);
BIND(4) RaytracingAccelerationStructure structures[16]:register(t0,space4);
[shader("compute")][numthreads(8,8,1)]
void shadowReference(uint3 id:SV_DispatchThreadID){if(id.x>=root.width||id.y>=root.height)return;
    float value=1;float4 normal=textures[NonUniformResourceIndex(root.normal)].Load(int3(id.xy,0));
    if(normal.w!=0){float4 surface=textures[NonUniformResourceIndex(root.world)].Load(int3(id.xy,0));
        float3 direction=buffers[NonUniformResourceIndex(root.parameters)].Load<float4>(0).xyz;
        RayDesc ray;ray.Origin=surface.xyz;ray.Direction=direction;ray.TMin=surface.w+0.002;ray.TMax=10000;
        RayQuery<RAY_FLAG_NONE> query;query.TraceRayInline(structures[NonUniformResourceIndex(root.acceleration)],RAY_FLAG_FORCE_OPAQUE|RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH,255,ray);
        while(query.Proceed()){}if(query.CommittedStatus()==COMMITTED_TRIANGLE_HIT)value=0;
    }
    images[NonUniformResourceIndex(root.output)][id.xy]=float4(value,value,value,1);
}
