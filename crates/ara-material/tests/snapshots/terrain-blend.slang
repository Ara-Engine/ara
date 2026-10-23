// Engine-owned material ABI v1, matching S1's eight root constant words.
// Color textures bind raw UNORM views; sampleColor performs sRGB decoding once.
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

struct AraMaterialInput {
    float2 uv;
    float3 worldPosition;
    float3 worldNormal;
    float3 worldTangent;
    float3 worldBitangent;
    float3 viewDirection;
};
struct AraSurface {
    float3 baseColor;
    float metallic;
    float roughness;
    float3 normal;
    float3 emissive;
    float opacityMask;
    float clearCoat;
    float clearCoatRoughness;
    float transmission;
    float3 color;
    float opacity;
    float clip;
};

float4 ara_load(uint offset) {
    return buffers[NonUniformResourceIndex(draw.bufferIndex)][draw.materialIndex + offset];
}
float2 ara_normalize(float2 v) { return v * rsqrt(max(dot(v, v), 1e-20)); }
float3 ara_normalize(float3 v) { return v * rsqrt(max(dot(v, v), 1e-20)); }
float4 ara_normalize(float4 v) { return v * rsqrt(max(dot(v, v), 1e-20)); }
float ara_linear_lane(float v) {
    return v <= 0.04045 ? v / 12.92 : pow(max((v + 0.055) / 1.055, 0.0), 2.4);
}
float3 ara_linear(float3 v) {
    return float3(ara_linear_lane(v.x), ara_linear_lane(v.y), ara_linear_lane(v.z));
}
float4 ara_sample_data(uint index, float2 uv) {
    return textures[NonUniformResourceIndex(index)].Sample(linearSampler, uv);
}
float3 ara_sample_color(uint index, float2 uv) { return ara_linear(ara_sample_data(index, uv).rgb); }
float3 ara_sample_normal(uint index, float2 uv) { return ara_normalize(ara_sample_data(index, uv).rgb * 2.0 - 1.0); }
float3 ara_normal_blend(float3 a, float3 b) {
    float3 t = a + float3(0, 0, 1);
    float3 u = b * float3(-1, -1, 1);
    return ara_normalize(t * dot(t, u) / max(t.z, 1e-5) - u);
}
float3 ara_normal_strength(float3 n, float strength) { return ara_normalize(float3(n.xy * strength, n.z)); }
float ara_hash(float2 p) { return frac(sin(dot(p, float2(127.1, 311.7))) * 43758.5453); }
float ara_noise(float2 p) {
    float2 i = floor(p);
    float2 f = frac(p);
    float2 u = f * f * (3.0 - 2.0 * f);
    return lerp(lerp(ara_hash(i), ara_hash(i + float2(1, 0)), u.x),
                lerp(ara_hash(i + float2(0, 1)), ara_hash(i + float2(1, 1)), u.x), u.y);
}
float3 ara_triplanar(uint index, float3 world, float3 normal, float scale) {
    float3 weights = abs(normal);
    weights /= max(weights.x + weights.y + weights.z, 1e-5);
    return ara_sample_color(index, world.yz * scale) * weights.x
         + ara_sample_color(index, world.zx * scale) * weights.y
         + ara_sample_color(index, world.xy * scale) * weights.z;
}
float2 ara_parallax(uint index, float2 uv, float3 tangentView, float scale) {
    float height = ara_sample_data(index, uv).r - 0.5;
    return uv - height * scale * tangentView.xy / max(abs(tangentView.z), 0.1);
}
float ara_fresnel(float3 n, float3 v, float power) { return pow(saturate(1.0 - dot(ara_normalize(n), ara_normalize(v))), max(power, 0.0)); }
float2 ara_rotate_uv(float2 uv, float radians) {
    float c = cos(radians), s = sin(radians);
    float2 p = uv - 0.5;
    return float2(c * p.x - s * p.y, s * p.x + c * p.y) + 0.5;
}
float ara_rounded_panel(float2 uv, float2 size, float radius) {
    radius = clamp(radius, 0.0, max(min(size.x, size.y) * 0.5, 0.0));
    float2 q = abs((uv - 0.5) * size) - max(size * 0.5 - radius, 0.0);
    float distance = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
    return saturate(0.5 - distance / max(fwidth(distance), 1e-4));
}

// Lighting is engine-owned. All renderer passes consume the same AraSurface;
// material functions cannot replace the BRDF or introduce independent bindings.
float3 ara_ggx(float3 f0, float roughness, float nDotL, float nDotV, float nDotH, float vDotH) {
    float alpha = max(roughness * roughness, 0.002025);
    float a2 = alpha * alpha;
    float d = nDotH * nDotH * (a2 - 1.0) + 1.0;
    float distribution = a2 / max(3.14159265 * d * d, 1e-6);
    float k = (roughness + 1.0) * (roughness + 1.0) * 0.125;
    float visibility = 1.0 / max((nDotV * (1.0 - k) + k) * (nDotL * (1.0 - k) + k) * 4.0, 1e-5);
    float3 fresnel = f0 + (1.0 - f0) * pow(1.0 - vDotH, 5.0);
    return distribution * visibility * fresnel;
}
float3 ara_engine_light(AraSurface s, AraMaterialInput input, float3 lightDirection, float3 radiance, float3 indirect) {
    float3 n = ara_normalize(input.worldTangent * s.normal.x + input.worldBitangent * s.normal.y + input.worldNormal * s.normal.z);
    float3 v = ara_normalize(input.viewDirection);
    float3 l = ara_normalize(lightDirection);
    float3 h = ara_normalize(v + l);
    float nl = saturate(dot(n, l)), nv = saturate(dot(n, v));
    float nh = saturate(dot(n, h)), vh = saturate(dot(v, h));
    float metallic = saturate(s.metallic), coat = saturate(s.clearCoat);
    float3 f0 = lerp(float3(0.04), s.baseColor, metallic);
    float3 fresnel = f0 + (1.0 - f0) * pow(1.0 - vh, 5.0);
    float3 diffuse = s.baseColor * (1.0 - metallic) * (1.0 - fresnel) / 3.14159265;
    float3 specular = ara_ggx(f0, saturate(s.roughness), nl, nv, nh, vh);
    float3 clearCoat = ara_ggx(float3(0.04), saturate(s.clearCoatRoughness), nl, nv, nh, vh) * coat;
    float3 transmitted = s.baseColor * saturate(s.transmission) * saturate(dot(-n, l));
    return ((diffuse + specular) * (1.0 - coat * 0.04) + clearCoat) * radiance * nl
         + transmitted * radiance + s.baseColor * indirect * (1.0 - metallic) + s.emissive;
}

AraSurface ara_evaluate(AraMaterialInput input) {
    AraSurface result;
    result.baseColor = float3(1);
    result.metallic = 0;
    result.roughness = 0.5;
    result.normal = float3(0, 0, 1);
    result.emissive = float3(0);
    result.opacityMask = 1;
    result.clearCoat = 0;
    result.clearCoatRoughness = 0.1;
    result.transmission = 0;
    result.color = float3(1);
    result.opacity = 1;
    result.clip = 1;
    uint n_grass = uint(ara_load(0u).x);
    uint n_rock = uint(ara_load(1u).x);
    float n_scale = ara_load(2u).x;
    float3 n_world = input.worldPosition;
    float3 n_normal = input.worldNormal;
    float3 n_a = ara_triplanar(n_grass, n_world, n_normal, n_scale);
    float3 n_b = ara_triplanar(n_rock, n_world, n_normal, n_scale);
    float2 n_uv = input.uv;
    float n_mix = ara_noise(n_uv);
    float3 n_color = lerp(n_a, n_b, n_mix);
    float n_rough = float(0.8500000238418579);
    result.baseColor = n_color;
    result.roughness = n_rough;
    return result;
}

// Offline verification entry. Renderer passes call ara_evaluate directly.
struct AraValidationInput {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float3 worldPosition : TEXCOORD1;
    float3 worldNormal : TEXCOORD2;
    float3 worldTangent : TEXCOORD3;
    float3 worldBitangent : TEXCOORD4;
    float3 viewDirection : TEXCOORD5;
};
[shader("fragment")]
float4 fragmentMain(AraValidationInput v) : SV_Target {
    AraMaterialInput input;
    input.uv = v.uv; input.worldPosition = v.worldPosition;
    input.worldNormal = v.worldNormal; input.worldTangent = v.worldTangent;
    input.worldBitangent = v.worldBitangent; input.viewDirection = v.viewDirection;
    AraSurface s = ara_evaluate(input);
    clip(s.opacityMask - 0.5);
    return float4(ara_engine_light(s, input, float3(0, 1, 0), float3(1), float3(0.03)), 1);
}
