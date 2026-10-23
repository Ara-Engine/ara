
float4 ara_custom_layerWeights(float3 position) {
Frame f=frame(); uint w=sampleData(f,int2(position.xz/f.params.x)).y; return float4(w&255,(w>>8)&255,(w>>16)&255,(w>>24)&255)/255.;
}

float3 ara_custom_splat(float3 a, float3 b, float3 c, float3 d, float4 weights) {
return a*weights.x+b*weights.y+c*weights.z+d*weights.w;
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
    uint n_219204f69469fc880345 = uint(ara_load(1u).x);
    float n_3dccae42872d81efdc77 = ara_load(4u).x;
    uint n_3fc82ed7ce52a16f27af = uint(ara_load(0u).x);
    uint n_5159fd3f120c7c9da2d1 = uint(ara_load(2u).x);
    uint n_80ded538dbddad1843d9 = uint(ara_load(3u).x);
    float3 n_74a5eb8e37ab9c10aaeb = input.worldPosition;
    float3 n_e9c411afe75a07dce1e4 = input.worldNormal;
    float3 n_6c2d436f362a6ee15871 = ara_triplanar(n_3fc82ed7ce52a16f27af, n_74a5eb8e37ab9c10aaeb, n_e9c411afe75a07dce1e4, n_3dccae42872d81efdc77);
    float3 n_9d762131428ba1c97a67 = ara_triplanar(n_219204f69469fc880345, n_74a5eb8e37ab9c10aaeb, n_e9c411afe75a07dce1e4, n_3dccae42872d81efdc77);
    float3 n_3c6d6cf827b46da195d5 = ara_triplanar(n_5159fd3f120c7c9da2d1, n_74a5eb8e37ab9c10aaeb, n_e9c411afe75a07dce1e4, n_3dccae42872d81efdc77);
    float3 n_33a73bc434920398bce4 = ara_triplanar(n_80ded538dbddad1843d9, n_74a5eb8e37ab9c10aaeb, n_e9c411afe75a07dce1e4, n_3dccae42872d81efdc77);
    float4 n_3b2aa9d201b9ffbb4f58 = ara_custom_layerWeights(n_74a5eb8e37ab9c10aaeb);
    float3 n_9d835fd400df2aa6c405 = ara_custom_splat(n_6c2d436f362a6ee15871, n_9d762131428ba1c97a67, n_3c6d6cf827b46da195d5, n_33a73bc434920398bce4, n_3b2aa9d201b9ffbb4f58);
    float n_871112a0b06acb52a5f1 = float(0.8799999952316284);
    result.baseColor = n_9d835fd400df2aa6c405;
    result.roughness = n_871112a0b06acb52a5f1;
    return result;
}

