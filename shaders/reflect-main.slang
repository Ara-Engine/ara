// Native-resolution deterministic quadrature. Every pixel owns all its samples;
// no neighbouring pixel, screen colour, spatial filter or history contributes.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "native_sample.slangh"
#include "glass-path.slangh"
float reflectRadical(uint x){return float(reversebits(x))*2.3283064365386963e-10;}
// Heitz visible-normal mapping, shared mathematics with the reference's
// visibleGgx, but driven by fixed quadrature coordinates instead of an RNG.
float3 reflectVisibleGgx(float3 n,float3 v,float alpha,float2 u){
    float3 t=normalize(cross(abs(n.y)<0.99?float3(0,1,0):float3(1,0,0),n)),b=cross(n,t);
    float3 vh=normalize(float3(alpha*dot(v,t),alpha*dot(v,b),max(dot(v,n),1e-5)));
    float lensq=dot(vh.xy,vh.xy);float3 t1=lensq>0?float3(-vh.y,vh.x,0)/sqrt(lensq):float3(1,0,0),t2=cross(vh,t1);
    float r=sqrt(u.x),phi=2*PI*u.y,x=r*cos(phi),y=r*sin(phi),blend=0.5*(1+vh.z);
    y=lerp(sqrt(max(0,1-x*x)),y,blend);float3 nh=x*t1+y*t2+sqrt(max(0,1-x*x-y*y))*vh;
    float3 h=normalize(float3(alpha*nh.xy,max(nh.z,0)));return t*h.x+b*h.y+n*h.z;
}
// Fixed GGX quadrature with correlated Smith and Fresnel weights. Divide by the
// sample count, including masked samples; never renormalize the visible part.
float3 reflectQuadrature(Frame f,Look l,float3 world,Surface s,float3 v,uint count){
    float3 sum=0,n=s.normal;float alpha=s.roughness*s.roughness;
    Lobes o=specularLobes(s,v);
    float3 origin=world+n*l.rtParams[1].x;
    for(uint k=0;k<count;k++){
        float3 h=reflectVisibleGgx(n,v,alpha,float2((float(k)+0.5)/float(count),reflectRadical(k)));
        float vh=dot(v,h);if(vh<=0)continue;
        float3 direction=reflect(-v,h);float nl=dot(n,direction);if(nl<=0)continue;
        float weight=nl*(o.nv+o.lv)/max(nl*o.lv+o.nv*sqrt(o.a2+(1-o.a2)*nl*nl),1e-6);
        sum+=glassPathRadiance(f,l,origin,direction,l.rtParams[0].z*length(world-f.camera.xyz))*weight*fresnel(vh,o.f0)*o.scale;
    }
    // Deferred IBL applies its split-sum albedo; cancel it for the integrated band.
    return sum/(float(count)*max(o.albedo,float3(1e-6)));
}
[shader("compute")][numthreads(1,1,1)]
void reflectClearMain(uint3 id:SV_DispatchThreadID){Look l=look(frame());for(uint i=0;i<8;i++)buffers[l.rtInfo[1].w].Store(i*4,0);}
[shader("compute")][numthreads(8,8,1)]
void reflectMirrorMain(uint3 id:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(id.xy>=f.extent.xy))return;int2 p=int2(id.xy);
    bool covered;float3 world=gbufferWorld(f,p,covered);float roughness=gbufferMaterial(f,p).x;
    if(!covered){images[root.materials][p]=0;return;}
    if(roughness>l.rtParams[0].x)return;
    reflectCount(l,0);float3 n=gbufferNormal(f,p),v=normalize(f.camera.xyz-world);
    float3 c=glassPathRadiance(f,l,world+n*l.rtParams[1].x,reflect(-v,n),l.rtParams[0].z*length(world-f.camera.xyz));
    images[root.materials][p]=float4(c,1);
}
[shader("compute")][numthreads(8,8,1)]
void reflectRoughMain(uint3 id:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(id.xy>=f.extent.xy))return;int2 p=int2(id.xy);
    bool covered;float3 world=gbufferWorld(f,p,covered);float roughness=gbufferMaterial(f,p).x;
    if(!covered || roughness<=l.rtParams[0].x || roughness>=l.rtParams[1].w)return;
    float3 n=gbufferNormal(f,p),v=normalize(f.camera.xyz-world),c;
    reflectCount(l,1);Surface s=(Surface)0;s.normal=n;s.albedo=gbufferAlbedo(f,p).rgb;s.roughness=roughness;s.metal=gbufferMaterial(f,p).y;
    c=reflectQuadrature(f,l,world,s,v,uint(l.rtParams[0].w));
    images[root.materials][p]=float4(c,1);
}
[shader("compute")][numthreads(8,8,1)]
void reflectPrefilteredMain(uint3 id:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);if(any(id.xy>=f.extent.xy))return;int2 p=int2(id.xy);
    bool covered;float3 world=gbufferWorld(f,p,covered);float roughness=gbufferMaterial(f,p).x;
    if(!covered || roughness<=l.rtParams[0].x || roughness<l.rtParams[1].w)return;
    reflectCount(l,7);float3 n=gbufferNormal(f,p),v=normalize(f.camera.xyz-world);
    images[root.materials][p]=float4(reflectPrefiltered(f,l,world,n,v,roughness),1);
}
