// Native raster fill. Geometry remains local and textures remain compressed.
#include "raster.slangh"
#include "look.slangh"
#include "look-shading.slangh"
#include "native_sample.slangh"
struct NativeVertex {float3 position;float3 normal;float2 uv;float4 tangent;float2 uv1;uint weights;uint reserved;};
struct NativeUv {float2 offset;float2 scale;float rotation;uint set;uint2 reserved;};
struct NativeMaterial {Material pbr;NativeUv uv[6];uint4 slots;float4 factors;uint4 layers;};
struct Instance {uint4 meta;float4x4 world;float4x4 previous;float4 nx;float4 ny;float4 nz;};
struct Varying {
    float4 clip:SV_Position;
    float3 world:TEXCOORD0;
    float3 normal:TEXCOORD1;
    float4 tangent:TEXCOORD2;
    float2 uv:TEXCOORD3;
    float2 uv1:TEXCOORD4;
    float4 weights:TEXCOORD5;
    float4 currentClip:TEXCOORD6;
    float4 previousClip:TEXCOORD7;
    nointerpolation uint material:TEXCOORD8;
    nointerpolation uint id:TEXCOORD9;
};
Varying nativeVertex(uint vertex,uint instance,bool shadow) {
    uint2 draw=buffers[NonUniformResourceIndex(root.flags)].Load2((root.entity+instance)*8);
    Instance i=buffers[NonUniformResourceIndex(root.material)].Load<Instance>(128+draw.x*192);
    uint4 g=buffers[NonUniformResourceIndex(root.mode)].Load4(draw.y*32);
    uint4 previous=buffers[NonUniformResourceIndex(root.mode)].Load4(draw.y*32+16);
    uint index=buffers[NonUniformResourceIndex(g.z)].Load(g.w+vertex*4);
    NativeVertex v=buffers[NonUniformResourceIndex(g.x)].Load<NativeVertex>(g.y+index*64);
    NativeVertex p=i.meta.y!=0?buffers[NonUniformResourceIndex(previous.x)].Load<NativeVertex>(previous.y+index*64):v;
    Frame f=frame();Varying o;
    float4 world=mul(i.world,float4(v.position,1));
    o.world=world.xyz;o.clip=mul(shadow?f.shadows[root.cascade]:f.vp,world);
    o.normal=normalize(float3(dot(i.nx.xyz,v.normal),dot(i.ny.xyz,v.normal),dot(i.nz.xyz,v.normal)));
    float3 tangent=mul(i.world,float4(v.tangent.xyz,0)).xyz;
    tangent-=o.normal*dot(tangent,o.normal);
    o.tangent=float4(normalize(tangent),v.tangent.w*asfloat(i.meta.w));
    o.uv=v.uv;o.uv1=v.uv1;
    o.weights=float4(v.weights&255,(v.weights>>8)&255,(v.weights>>16)&255,v.weights>>24)/255.;
    float4x4 vp=buffers[NonUniformResourceIndex(root.material)].Load<float4x4>(0);
    float4x4 previousVp=buffers[NonUniformResourceIndex(root.material)].Load<float4x4>(64);
    o.currentClip=mul(vp,world);o.previousClip=mul(previousVp,mul(i.previous,float4(p.position,1)));
    o.material=i.meta.x;o.id=draw.x+1;return o;
}
[shader("vertex")]
Varying nativeGeometryVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){return nativeVertex(vertex,instance,false);}
struct ShadowVarying {float4 clip:SV_Position;float2 uv:TEXCOORD0;float2 uv1:TEXCOORD1;nointerpolation uint material:TEXCOORD2;};
[shader("vertex")]
ShadowVarying nativeShadowVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){Varying v=nativeVertex(vertex,instance,true);ShadowVarying s;s.clip=v.clip;s.uv=v.uv;s.uv1=v.uv1;s.material=v.material;return s;}
NativeMaterial material(uint id){return buffers[NonUniformResourceIndex(root.materials)].Load<NativeMaterial>(id*320);}
float2 coordinates(NativeUv transform,Varying v){float2 p=(transform.set==0?v.uv:v.uv1)*transform.scale;float s,c;sincos(transform.rotation,s,c);return transform.offset+float2(c*p.x-s*p.y,s*p.x+c*p.y);}
float4 sampleMap(NativeMaterial m,uint slot,Varying v){
    uint id=slot<4?m.pbr.textures[min(slot,3)]:(slot==4?m.slots.x:m.slots.y);
    if(id==0xffffffff)return float4(slot==1?float3(0,0,1):float3(1),1);
    float2 uv=coordinates(m.uv[slot],v);
    float lod=ntDesiredLod(root.sample,id,ddx(uv),ddy(uv));
    if(slot==1)return float4(ntNormal(root.sample,id,uv,lod,m.pbr.surface.y),1);
    bool decode=slot==0?(m.pbr.flags.y&(1u<<25))!=0:slot==3 && (m.pbr.flags.y&(1u<<26))!=0;
    return ntSampleColorLevel(root.sample,id,uv,lod,decode);
}
Surface surfaceMaterial(NativeMaterial m,Varying v){
    Surface s;float4 base=sampleMap(m,0,v)*m.pbr.color;
    s.albedo=max(0,base.rgb);s.alpha=base.a;
    float3 normal=normalize(v.normal),t=v.tangent.xyz-normal*dot(v.tangent.xyz,normal);float handedness=v.tangent.w;
    if(m.slots.w!=0){
        float2 uv=coordinates(m.uv[1],v),a=ddx(uv),b=ddy(uv);float determinant=a.x*b.y-a.y*b.x;
        if(abs(determinant)>1e-10){float3 e1=ddx(v.world),e2=ddy(v.world);t=(e1*b.y-e2*a.y)/determinant;t-=normal*dot(t,normal);float3 bitangent=(e2*a.x-e1*b.x)/determinant;handedness=dot(cross(normal,t),bitangent)<0?-1:1;}
    }
    t=normalize(t);float3 n=sampleMap(m,1,v).xyz;
    s.normal=normalize(t*n.x+cross(normal,t)*handedness*n.y+normal*n.z);
    float4 mr=sampleMap(m,2,v);s.roughness=clamp(m.pbr.emissiveRoughness.w*mr.g,0.045,1);s.metal=saturate(m.pbr.surface.x*mr.b);
    s.emissive=max(0,m.pbr.emissiveRoughness.rgb*sampleMap(m,3,v).rgb);
    s.ao=lerp(1,sampleMap(m,4,v).r,m.factors.x);return s;
}
Surface surface(Varying v){
    NativeMaterial m=material(v.material);Surface s=surfaceMaterial(m,v);
    if(m.slots.z>0){
        float4 weights=v.weights;for(uint i=m.slots.z;i<4;i++)weights[i]=0;
        float sum=dot(weights,float4(1));weights=sum>1e-8?weights/sum:float4(1,0,0,0);s=(Surface)0;
        for(uint i=0;i<m.slots.z;i++){
            Surface a=surfaceMaterial(material(m.layers[i]),v);float w=weights[i];
            s.albedo+=a.albedo*w;s.normal+=a.normal*w;s.roughness+=a.roughness*w;s.metal+=a.metal*w;s.emissive+=a.emissive*w;s.alpha+=a.alpha*w;s.ao+=a.ao*w;
        }
        s.normal=normalize(s.normal);
    }
    if(m.pbr.flags.x==1 && s.alpha<m.pbr.surface.w)discard;
    float3 du=ddx(s.normal),dv=ddy(s.normal);float kernel=min(2*0.15915494*(dot(du,du)+dot(dv,dv)),0.18);
    s.roughness=sqrt(sqrt(saturate(pow(s.roughness,4)+kernel)));return s;
}
float2 motion(Varying v){float2 now=v.currentClip.xy/v.currentClip.w,previous=v.previousClip.xy/v.previousClip.w;return (now-previous)*float2(0.5,-0.5);}
struct GBuffer {float4 albedo:SV_Target0;float4 normal:SV_Target1;float4 material:SV_Target2;float4 world:SV_Target3;float id:SV_Target4;float2 velocity:SV_Target5;};
[shader("fragment")]
GBuffer nativeGeometryFragment(Varying v){Surface s=surface(v);GBuffer g;g.albedo=float4(s.albedo,s.ao);g.normal=float4(s.normal,s.roughness);g.material=float4(s.emissive,s.metal);g.world=float4(v.world,1);g.id=float(v.id);g.velocity=motion(v);return g;}
[shader("fragment")]
void nativeShadowFragment(ShadowVarying shadow){Varying v=(Varying)0;v.uv=shadow.uv;v.uv1=shadow.uv1;NativeMaterial m=material(shadow.material);if(m.pbr.flags.x==1 && sampleMap(m,0,v).a*m.pbr.color.a<m.pbr.surface.w)discard;}
struct Forward {float4 color:SV_Target0;float id:SV_Target1;float4 albedo:SV_Target2;float4 normal:SV_Target3;float4 material:SV_Target4;float4 world:SV_Target5;float4 velocity:SV_Target6;};
[shader("fragment")]
Forward nativeForwardFragment(Varying v){Surface s=surface(v);Frame f=frame();Look l=look(f);float2 uv=v.clip.xy/float2(f.extent.xy);Forward o;o.color=float4(applyFog(f,l,shade(f,l,s,v.world,int2(v.clip.xy)),v.world,uv)*s.alpha,s.alpha);o.id=float(v.id);o.albedo=float4(s.albedo,s.alpha);o.normal=float4(s.normal,s.roughness);o.material=float4(s.emissive,s.metal);o.world=float4(v.world,1);o.velocity=float4(motion(v),0,1);return o;}
