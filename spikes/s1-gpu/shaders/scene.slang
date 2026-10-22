// The register ABI is also the converter root-signature ABI; see RESULT.md.
struct DrawConstants {
    uint materialIndex;
    uint textureIndex;
    uint bufferIndex;
    uint flags;
    float phase;
    float aspect;
    uint padding0;
    uint padding1;
};
#ifdef VULKAN
#define GPU_BINDING(index) [[vk::binding(index, 0)]]
[[vk::push_constant]] ConstantBuffer<DrawConstants> draw;
#else
#define GPU_BINDING(index)
ConstantBuffer<DrawConstants> draw : register(b0, space0);
#endif
GPU_BINDING(0) Texture2D<float4> textures[16] : register(t0, space1);
GPU_BINDING(1) RWStructuredBuffer<float4> buffers[4] : register(u0, space2);
GPU_BINDING(2) SamplerState linearSampler : register(s0, space0);
GPU_BINDING(3) RaytracingAccelerationStructure scene : register(t0, space3);

struct VertexInput {
    float3 position : POSITION;
    float3 normal : NORMAL;
    float2 uv : TEXCOORD0;
};
struct FragmentInput {
    float4 clip : SV_Position;
    float3 world : TEXCOORD0;
    float3 normal : TEXCOORD1;
    float2 uv : TEXCOORD2;
};

[shader("vertex")]
FragmentInput vertexMain(VertexInput input) {
    FragmentInput output;
    float3 eye = float3(6, 5, 8);
    float3 forward = normalize(float3(0, 0.6, 0) - eye);
    float3 right = normalize(cross(forward, float3(0, 1, 0)));
    float3 up = cross(right, forward);
    float3 relative = input.position - eye;
    float z = dot(relative, forward);
    output.clip = float4(dot(relative, right) * 1.9 / draw.aspect,
                         dot(relative, up) * 1.9, (z - 0.1) * 100.0 / 99.9, z);
    output.world = input.position;
    output.normal = input.normal;
    output.uv = input.uv;
    return output;
}

[shader("fragment")]
float4 fragmentMain(FragmentInput input) : SV_Target {
    float3 normal = normalize(input.normal);
    float3 light = normalize(float3(-0.7, 1.0, 0.35));
    float shadow = 1.0;
    if ((draw.flags & 1) != 0) {
        RayDesc ray;
        ray.Origin = input.world + normal * 0.006;
        ray.Direction = light;
        ray.TMin = 0.001;
        ray.TMax = 100.0;
        RayQuery<RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH | RAY_FLAG_FORCE_OPAQUE> query;
        query.TraceRayInline(scene, RAY_FLAG_NONE, 0xff, ray);
        while (query.Proceed()) {}
        shadow = query.CommittedStatus() == COMMITTED_TRIANGLE_HIT ? 0.12 : 1.0;
    }
    float3 albedo = textures[NonUniformResourceIndex(draw.textureIndex)]
        .Sample(linearSampler, input.uv).rgb;
    uint cell = (uint(abs(input.world.x) * 16) + uint(abs(input.world.z) * 16)) % 256;
    float3 tint = buffers[NonUniformResourceIndex(draw.bufferIndex)][cell].rgb;
    float materialGain = 0.85 + 0.05 * draw.materialIndex;
    float diffuse = max(dot(normal, light), 0.0);
    float3 color = albedo * tint * materialGain * (0.22 + diffuse * shadow * 1.15);
    // Small fixed exposure + display gamma; no post stack in the binding spike.
    return float4(pow(color / (color + 0.7), 1.0 / 2.2), 1);
}

[shader("compute")]
[numthreads(64, 1, 1)]
void computeMain(uint3 thread : SV_DispatchThreadID) {
    uint i = thread.x;
    if (i < 256) {
        float wave = 0.88 + 0.12 * sin(float(i) * 0.08 + draw.phase);
        buffers[NonUniformResourceIndex(draw.bufferIndex)][i] = float4(wave, 0.94, 1.0, 1.0);
    }
}
