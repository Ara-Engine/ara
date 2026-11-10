// Generated hit.slang concatenates this prefix, the authoritative helper, and the probe.
struct Root {uint scene; uint table; uint output; uint d; uint e; uint f; uint g; uint h;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(1) RWByteAddressBuffer buffers[] : register(u0,space2);
BIND(4) RaytracingAccelerationStructure scenes[16] : register(t0,space4);
