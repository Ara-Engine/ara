// Deferred decals (ara_render renderer/native/decals.rs): one 16x16-tile compute pass
// between the opaque G-buffer and everything that reads it. Each tile keeps the
// visible decals (CPU-culled, faded and sorted) whose screen rect and depth range
// meet its pixels as one bit each, in draw order; each pixel then reads its G-buffer
// once, blends every decal in order in registers and writes once. Pixels no decal
// touches cost one depth read; tiles without decals cost nothing more.
//
// Root words: 0 frame, 1 material rows, 2 instance rows (bit 31 of meta.y: receives
// no decals), 3 decal records, 4 visible list (header: count, then the G-buffer
// storage views albedo, normal, material, emissive), 6 texture table, 7 the coverage
// view (ViewMode::DecalCoverage) when one.
#include "raster.slangh"
#include "native_sample.slangh"
struct DecalUv {float2 offset;float2 scale;float rotation;uint set;uint2 reserved;};
// A decal material row (instanced::Material, 320 bytes): its PBR block, then six slot
// transforms from byte 80 and the occlusion/opacity map IDs at 272. Loaded in parts:
// slangc 2026.17 crashes reflecting a whole-row struct load in this pass.
struct DecalMaterial {Material pbr;uint row;uint2 maps;};
DecalMaterial decalMaterial(uint row){DecalMaterial m;m.pbr=buffers[NonUniformResourceIndex(root.materials)].Load<Material>(row*320);m.row=row;m.maps=buffers[NonUniformResourceIndex(root.materials)].Load2(row*320+272);return m;}
// renderer/native/decals.rs Record: world to the unit box, axes with the base colour,
// normal and roughness opacities, the other opacities, fade angles and material, maps.
struct DecalRecord {float4x4 toBox;float4 x;float4 y;float4 z;float4 opacity;float4 angle;float4 maps;};
static const uint RECORD=160,HEADER=32,ENTRY=32,NO_MAP=0xffffffff;
groupshared uint tileMask[32];
groupshared uint tileNear,tileFar;
// Linear to sRGB-encoded: the albedo target stores through its UNORM alias view.
float3 srgbEncode(float3 c){return select(c<=0.0031308,c*12.92,1.055*pow(c,1./2.4)-0.055);}
// Where the ray through render pixel `q` meets the plane through `p` with normal `n`
// (the footprint of a neighbouring pixel on this surface; perspective or ortho).
float3 planeHit(Frame f,float2 q,float3 p,float3 n){
    float3 a=worldAtDepth(f,q,1),b=worldAtDepth(f,q,0.5),d=b-a;float denom=dot(d,n);
    return abs(denom)>1e-8?a+d*(dot(p-a,n)/denom):p;
}
// Map `slot` of `m` at decal coordinates `uv` with its transform, over the footprint.
float4 decalMap(DecalMaterial m,uint slot,float2 uv,float2 dx,float2 dy){
    uint id=slot<4?m.pbr.textures[min(slot,3)]:(slot==4?m.maps.x:m.maps.y);
    if(id==NO_MAP)return float4(slot==1?float3(0,0,1):float3(1),1);
    DecalUv t=buffers[NonUniformResourceIndex(root.materials)].Load<DecalUv>(m.row*320+80+slot*32);float s,c;sincos(t.rotation,s,c);
    float2x2 r=float2x2(c*t.scale.x,-s*t.scale.y,s*t.scale.x,c*t.scale.y);
    float2 at=t.offset+mul(r,uv);
    uint image=buffers[NonUniformResourceIndex(root.sample)].Load(id*32);
    float4 texel=textures[NonUniformResourceIndex(image)].SampleGrad(samplers[0],at,mul(r,dx),mul(r,dy));
    if(slot==1){
        float3 n=texel.xyz*2-1;
        if(buffers[NonUniformResourceIndex(root.sample)].Load(id*32+16)==2)n.z=sqrt(max(0,1-dot(n.xy,n.xy)));
        n.xy*=m.pbr.surface.y;return float4(dot(n,n)>1e-12?normalize(n):float3(0,0,1),1);
    }
    bool decode=slot==0?(m.pbr.flags.y&(1u<<25))!=0:slot==3 && (m.pbr.flags.y&(1u<<26))!=0;
    if(decode)texel.rgb=ntDecodeColor(texel.rgb);
    return texel;
}
// The overdraw view's heat legend (baseline.slang overdrawHeat): 1 blue, 2 green,
// 4 yellow, 8 orange, 16 or more red.
float3 decalHeat(float count){
    float t=clamp(log2(count),0,4);float3 blue=float3(0,0,1),green=float3(0,1,0),yellow=float3(1,1,0),orange=float3(1,0.2122,0),red=float3(1,0,0);
    return t<1?lerp(blue,green,t):t<2?lerp(green,yellow,t-1):t<3?lerp(yellow,orange,t-2):lerp(orange,red,t-3);
}
[shader("compute")]
[numthreads(16,16,1)]
void decalsMain(uint3 tid:SV_DispatchThreadID,uint3 group:SV_GroupID,uint index:SV_GroupIndex){
    Frame f=frame();
    uint4 head=buffers[root.cascade].Load4(0);uint4 head2=buffers[root.cascade].Load4(16);uint count=min(head.x,1024u);
    if(index<32)tileMask[index]=0;
    if(index==0){tileNear=0;tileFar=0x7f800000;}
    GroupMemoryBarrierWithGroupSync();
    int2 p=int2(tid.xy);bool inside=all(tid.xy<f.extent.xy);
    float depth=inside?gbufferDepth(f,p):0;
    // Reversed-Z: positive depths order like their bits; zero is the sky.
    if(depth>0){InterlockedMax(tileNear,asuint(depth));InterlockedMin(tileFar,asuint(depth));}
    GroupMemoryBarrierWithGroupSync();
    float near=asfloat(tileNear),far=asfloat(tileFar);uint2 lo=group.xy*16,hi=lo+16;
    if(tileNear!=0){
        for(uint i=index;i<count;i+=256){
            uint4 e=buffers[root.cascade].Load4(HEADER+i*ENTRY);float2 z=asfloat(buffers[root.cascade].Load2(HEADER+i*ENTRY+16));
            uint2 r0=uint2(e.y&0xffff,e.y>>16),r1=uint2(e.z&0xffff,e.z>>16);
            if(r0.x<hi.x && r1.x>lo.x && r0.y<hi.y && r1.y>lo.y && z.x>=far && z.y<=near)InterlockedOr(tileMask[i>>5],1u<<(i&31));
        }
    }
    GroupMemoryBarrierWithGroupSync();
    bool coverage=root.flags!=0;
    if(depth<=0)return;
    uint words=(count+31)/32,anyBits=0;
    for(uint w=0;w<words;w++)anyBits|=tileMask[w];
    if(anyBits==0 && !coverage)return;
    uint id=uint(pixel(f.extra.y,p).x+0.5);
    bool receives=id!=0 && (buffers[NonUniformResourceIndex(root.material)].Load(128+(id-1)*192+4)>>31)==0;
    float4 albedo=images[NonUniformResourceIndex(head.y)][p];albedo.rgb=ntDecodeColor(albedo.rgb);
    float4 encoded=images[NonUniformResourceIndex(head.z)][p];float3 n=octDecode(encoded.xy),surface=n;
    float4 material=images[NonUniformResourceIndex(head.w)][p];
    float4 emissive=images[NonUniformResourceIndex(head2.x)][p];
    float3 world=worldAtDepth(f,float2(p)+0.5,depth);
    // Neighbouring pixels' footprint on this surface: texture LODs without the
    // derivative breaks a screen-space difference has at depth edges.
    float3 ddxWorld=(planeHit(f,float2(p)+float2(1.5,0.5),world,surface)-world)*f.upscale.z;
    float3 ddyWorld=(planeHit(f,float2(p)+float2(0.5,1.5),world,surface)-world)*f.upscale.z;
    uint touched=0;
    for(uint w=0;w<words && receives;w++){
        uint bits=tileMask[w];
        while(bits!=0){
            uint i=w*32+firstbitlow(bits);bits&=bits-1;
            uint4 e=buffers[root.cascade].Load4(HEADER+i*ENTRY);
            DecalRecord d=buffers[NonUniformResourceIndex(root.entity)].Load<DecalRecord>(e.x*RECORD);
            float3 local=mul(d.toBox,float4(world,1)).xyz;
            if(any(abs(local)>0.5))continue;
            // Surfaces turned away from the projection axis fade out (angle fade).
            float facing=dot(surface,d.z.xyz);
            float weight=asfloat(e.w)*saturate((facing-d.angle.z)/max(d.angle.y-d.angle.z,1e-4));
            if(weight<=0)continue;
            float2 uv=float2(local.x+0.5,0.5-local.y);
            float3 lx=mul(d.toBox,float4(ddxWorld,0)).xyz,ly=mul(d.toBox,float4(ddyWorld,0)).xyz;
            float2 dx=float2(lx.x,-lx.y),dy=float2(ly.x,-ly.y);
            DecalMaterial m=decalMaterial(asuint(d.angle.w));
            uint opacityChannel=asuint(d.maps.x);
            float opacity=m.pbr.color.a*weight*(opacityChannel<4?component(decalMap(m,5,uv,dx,dy),opacityChannel):1);
            float4 base=decalMap(m,0,uv,dx,dy);
            if(opacity<=0)continue;
            touched++;
            albedo.rgb=lerp(albedo.rgb,max(0,base.rgb*m.pbr.color.rgb),saturate(opacity*d.x.w));
            // The decal's normal in the surface's own tangent frame: the decal's X axis
            // (U) on the surface, V along its -Y, as reconstructed glTF bases are.
            if(d.y.w>0 && m.pbr.textures[1]!=NO_MAP){
                float3 tn=decalMap(m,1,uv,dx,dy).xyz;
                float3 t=d.x.xyz-n*dot(d.x.xyz,n);t=dot(t,t)>1e-8?normalize(t):normalize(cross(n,abs(n.y)<0.99?float3(0,1,0):float3(1,0,0)));
                float handedness=dot(cross(n,t),-d.y.xyz)<0?-1:1;
                float3 mapped=normalize(t*tn.x+cross(n,t)*handedness*tn.y+n*tn.z);
                float3 blended=lerp(n,mapped,saturate(opacity*d.y.w));
                n=dot(blended,blended)>1e-8?normalize(blended):n;
            }
            float4 mr=decalMap(m,2,uv,dx,dy);
            material.x=lerp(material.x,clamp(m.pbr.emissiveRoughness.w*mr.g,0.045,1),saturate(opacity*d.z.w));
            material.y=lerp(material.y,saturate(m.pbr.surface.x*mr.b),saturate(opacity*d.opacity.x));
            float3 glow=max(0,m.pbr.emissiveRoughness.rgb*decalMap(m,3,uv,dx,dy).rgb);
            emissive.rgb=lerp(emissive.rgb,glow,saturate(opacity*d.opacity.z));
            uint aoChannel=asuint(d.maps.z);
            float ao=saturate(d.maps.y*(aoChannel<4?component(decalMap(m,4,uv,dx,dy),aoChannel):1));
            albedo.a=lerp(albedo.a,ao,saturate(opacity*d.opacity.w));
            // d.opacity.y (specular) waits for a G-buffer specular channel.
        }
    }
    if(coverage){
        // Decals per pixel over the dimmed scene; surfaces that refuse decals grey.
        float3 scene=float3(dot(albedo.rgb,float3(0.2126,0.7152,0.0722))*0.2+0.02);
        albedo.rgb=touched>0?decalHeat(float(touched)):receives?scene:float3(0.12);
        images[NonUniformResourceIndex(head.y)][p]=float4(albedo.rgb,1);
        return;
    }
    if(touched==0)return;
    images[NonUniformResourceIndex(head.y)][p]=float4(srgbEncode(saturate(albedo.rgb)),albedo.a);
    images[NonUniformResourceIndex(head.z)][p]=float4(octEncode(n),encoded.zw);
    images[NonUniformResourceIndex(head.w)][p]=material;
    images[NonUniformResourceIndex(head2.x)][p]=gbufferEncodeEmissive(emissive.rgb,emissive.a);
}
