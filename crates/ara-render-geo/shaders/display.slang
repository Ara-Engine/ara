struct Root{uint source;uint output;uint width;uint height;uint mode;uint paper;uint peak;uint reserved;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root:register(b0,space0);
#endif
BIND(5) Texture2D<float4> textures[]:register(t0,space1);
BIND(3) RWTexture2D<float4> images[64]:register(u0,space3);
float3 outputSrgb(float3 v){v=saturate(v);return select(v<=.0031308,12.92*v,1.055*pow(v,float3(1./2.4))-.055);}
float3 outputPq(float3 nits){float3 y=pow(saturate(nits/10000.),float3(2610./16384.));return pow((3424./4096.+2413./128.*y)/(1.+2392./128.*y),float3(2523./32.));}
[shader("compute")][numthreads(8,8,1)]
void displayMain(uint3 id:SV_DispatchThreadID){if(id.x>=root.width||id.y>=root.height)return;
    float3 c=textures[NonUniformResourceIndex(root.source)].Load(int3(id.xy,0)).rgb;
    float paper=asfloat(root.paper),peak=asfloat(root.peak);float maximum=max(0.,max(c.r,max(c.g,c.b)));
    float ceiling=root.mode==0?1.:peak/paper,start=root.mode==0?.8:1.;
    if(maximum>start){float headroom=max(ceiling-start,1e-6);float mapped=start+headroom*(1.-exp(-(maximum-start)/headroom));c*=mapped/maximum;}
    if(root.mode==0)c=outputSrgb(c);
    else if(root.mode==1){c=float3(dot(c,float3(.627404,.329283,.043313)),dot(c,float3(.069097,.919540,.011362)),dot(c,float3(.016391,.088013,.895595)));c=outputPq(c*paper);}
    else c*=paper/80.;
    images[NonUniformResourceIndex(root.output)][id.xy]=float4(c,1);
}
