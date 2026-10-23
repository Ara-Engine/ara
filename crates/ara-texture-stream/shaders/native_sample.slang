// NativeBinding table ABI. Consumers declare Root32 buffers/textures/samplers
// before including this file. Hardware sampling handles BC decoding and sRGB.
uint4 ntInfo(uint table, uint textureId) {
    return buffers[NonUniformResourceIndex(table)].Load4(textureId * 32);
}
float ntDesiredLod(uint table, uint textureId, float2 dx, float2 dy) {
    uint4 info = ntInfo(table, textureId);
    float rho = max(length(dx * float2(info.zw)), length(dy * float2(info.zw)));
    uint levels = buffers[NonUniformResourceIndex(table)].Load(textureId * 32 + 20);
    return clamp(log2(max(rho, 1.0)), 0.0, float(info.y + levels - 1));
}
float4 ntSampleLevel(uint table, uint textureId, float2 uv, float desiredLod) {
    uint4 info = ntInfo(table, textureId);
    // The image's first resident mip is physical LOD zero. Sampler clamping
    // preserves a valid tail during promotion and performs native trilinear filtering.
    return textures[NonUniformResourceIndex(info.x)].SampleLevel(
        samplers[0], uv, max(0.0, desiredLod - float(info.y)));
}
float3 ntNormal(uint table, uint textureId, float2 uv, float desiredLod, float scale) {
    float3 n = ntSampleLevel(table, textureId, uv, desiredLod).xyz * 2.0 - 1.0;
    uint encoding = buffers[NonUniformResourceIndex(table)].Load(textureId * 32 + 16);
    // BC5 stores XY; reconstruct the unscaled normal, then apply glTF normalScale.
    if (encoding == 2) n.z = sqrt(max(0.0, 1.0 - dot(n.xy, n.xy)));
    n.xy *= scale;
    return normalize(n);
}
