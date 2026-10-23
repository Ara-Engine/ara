struct Root{uint source;uint output;uint inputWidth;uint inputHeight;uint width;uint height;uint a;uint b;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root:register(b0,space0);
#endif
BIND(5) Texture2D<float4> textures[]:register(t0,space1);
BIND(3) RWTexture2D<float4> images[64]:register(u0,space3);
float3 sample(int2 p){p=clamp(p,0,int2(root.inputWidth,root.inputHeight)-1);return pow(textures[NonUniformResourceIndex(root.source)].Load(int3(p,0)).rgb,float3(2.2));}
[shader("compute")][numthreads(8,8,1)]
void upscaleMain(uint3 id:SV_DispatchThreadID){if(id.x>=root.width||id.y>=root.height)return;
    float2 p=(float2(id.xy)+.5)/float2(root.width,root.height)*float2(root.inputWidth,root.inputHeight)-.5;
    int2 lo=int2(floor(p));float2 t=frac(p);
    float3 color=lerp(lerp(sample(lo),sample(lo+int2(1,0)),t.x),lerp(sample(lo+int2(0,1)),sample(lo+1),t.x),t.y);
    images[NonUniformResourceIndex(root.output)][id.xy]=float4(pow(saturate(color),float3(1.0/2.2)),1);
}
