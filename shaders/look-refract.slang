// The opaque-scene copy transmissive forward surfaces refract (look/refract.rs,
// look-refract.slangh): root word 1 is the lit HDR (sampled), word 2 the level:
// 0 copies radiance and view depth inside the frame's refraction rect, 1..4 each
// blur the level above with a [1 3 3 1] tent at half its size.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "look-refract.slangh"
[shader("compute")][numthreads(8,8,1)]
void refractMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint level=root.material;uint4 r=refractRect(l,level);
    uint2 p=r.xy+tid.xy;if(any(p>=r.zw))return;
    float4 c=0;
    if(level==0){
        float depth=gbufferDepth(f,int2(p));
        c=float4(min(pixel(root.materials,int2(p)).rgb,float3(60000)),depth>0?refractViewDepth(f,worldAtDepth(f,float2(p)+0.5,depth)):0);
    }else{
        uint4 a=refractRect(l,level-1);uint2 o=refractLevelOrigin(f,level-1);
        for(uint j=0;j<4;j++)for(uint i=0;i<4;i++){
            int2 q=clamp(int2(p*2)+int2(i,j)-1,int2(a.xy),int2(a.zw)-1);
            float w=(i==0||i==3?1.:3.)*(j==0||j==3?1.:3.)/64.;
            c+=images[NonUniformResourceIndex(l.refractInfo[0].y)][o+uint2(q)]*w;
        }
    }
    images[NonUniformResourceIndex(l.refractInfo[0].y)][refractLevelOrigin(f,level)+p]=c;
}
