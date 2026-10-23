// Landscape ABI: column-major, Y-up metres, reversed-Z, explicit tick wind.
struct Root {uint frame; uint group; uint shadowPass; uint depth; uint shadow; uint hzb; uint hzbOut; uint reserved;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(0) Texture2D<float4> textures[128] : register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[64] : register(u0,space2);
BIND(2) SamplerState samplers[3] : register(s0,space0);
BIND(3) RWTexture2D<float4> images[64] : register(u0,space3);
struct Frame {
    float4x4 vp; float4x4 lightVp; float4x4 inverseVp;
    float4 camera; float4 sun; float4 sunColor;
    uint4 grid; uint4 extent; uint4 indices; uint4 more; float4 params; uint4 geometry;
};
Frame frame(){return buffers[NonUniformResourceIndex(root.frame)].Load<Frame>(0);}
uint loadWord(uint slot,uint at){return buffers[NonUniformResourceIndex(slot)].Load(at*4);}
uint3 sampleData(Frame f,int2 p){
    p=clamp(p,int2(0),int2(f.grid.x-1));uint slot=f.indices.x;uint cells=f.grid.y;
    uint2 tile=min(uint2(p)/cells,f.grid.z-1);uint page=loadWord(slot,8+tile.y*f.grid.z+tile.x);
    if(page>0){uint pageBase=loadWord(slot,6)+(page-1)*(cells+1)*(cells+1)*3;uint2 local=uint2(p)-tile*cells;
        uint at=pageBase+(local.y*(cells+1)+local.x)*3;return uint3(loadWord(slot,at),loadWord(slot,at+1),loadWord(slot,at+2));}
    uint side=loadWord(slot,2),step=loadWord(slot,3),offset=loadWord(slot,5);float2 c=float2(p)/step;uint2 b=min(uint2(c),side-2);float2 t=c-b;
    uint at=offset+(b.y*side+b.x)*3;
    float a=asfloat(loadWord(slot,at)),b1=asfloat(loadWord(slot,at+3)),c1=asfloat(loadWord(slot,at+side*3)),d=asfloat(loadWord(slot,at+(side+1)*3));
    return uint3(asuint(lerp(lerp(a,b1,t.x),lerp(c1,d,t.x),t.y)),loadWord(slot,at+1),loadWord(slot,at+2));
}
float terrainHeight(Frame f,int2 p){return asfloat(sampleData(f,p).x);}
float worldHeight(Frame f,float2 p){
    float2 c=clamp(p/f.params.x,0,float(f.grid.x-1));int2 base=int2(min(floor(c),float(f.grid.x-2)));float2 t=c-base;
    return lerp(lerp(terrainHeight(f,base),terrainHeight(f,base+int2(1,0)),t.x),lerp(terrainHeight(f,base+int2(0,1)),terrainHeight(f,base+1),t.x),t.y);
}
float3 terrainNormal(Frame f,float2 p){float e=f.params.x;return normalize(float3(worldHeight(f,p-float2(e,0))-worldHeight(f,p+float2(e,0)),2*e,worldHeight(f,p-float2(0,e))-worldHeight(f,p+float2(0,e))));}
uint tileLevel(Frame f,int2 tile){tile=clamp(tile,0,int(f.grid.z-1));return loadWord(f.indices.z,tile.y*f.grid.z+tile.x);}
float patchDistance(Frame f,uint2 tile){float width=f.grid.y*f.params.x;float2 lo=tile*width;float2 d=f.camera.xz-clamp(f.camera.xz,lo,lo+width);return length(d);}
bool sphereIn(float4x4 vp,float3 p,float radius){
    float4 c=mul(vp,float4(p,1));
    float rx=radius*length(float3(vp[0][0],vp[0][1],vp[0][2]));
    float ry=radius*length(float3(vp[1][0],vp[1][1],vp[1][2]));
    float rw=radius*length(float3(vp[3][0],vp[3][1],vp[3][2]));
    return c.w+rw>0 && abs(c.x)<=c.w+rx+rw && abs(c.y)<=c.w+ry+rw && c.z+rw>=0;
}
uint foliageVertices(uint group){Frame f=frame();if(f.geometry.x!=0)return loadWord(f.geometry.y,group*8+1);uint species=group/3,lod=group%3;return species==0?(lod==0?12:6):species==1?(lod==0?108:lod==1?36:6):(lod==0?24:lod==1?12:6);}
[shader("compute")][numthreads(64,1,1)]
void tileUpload(uint id:SV_DispatchThreadID){
    Frame f=frame();uint count=(f.grid.y+1)*(f.grid.y+1);if(id>=f.extent.w*count)return;uint batch=id/count,sample=id%count;
    uint stride=4+count*3,base=batch*stride;uint tileId=loadWord(root.reserved,base),page=loadWord(root.reserved,base+1),old=loadWord(root.reserved,base+2);
    uint target=f.indices.x,pageBase=loadWord(target,6)+page*count*3;
    if(sample==0){if(old!=0xffffffff&&old!=tileId)buffers[NonUniformResourceIndex(target)].Store((8+old)*4,0);buffers[NonUniformResourceIndex(target)].Store((8+tileId)*4,page+1);}
    uint3 value=uint3(loadWord(root.reserved,base+4+sample*3),loadWord(root.reserved,base+5+sample*3),loadWord(root.reserved,base+6+sample*3));
    buffers[NonUniformResourceIndex(target)].Store3((pageBase+sample*3)*4,value);
    uint2 tile=uint2(tileId%f.grid.z,tileId/f.grid.z),local=uint2(sample%(f.grid.y+1),sample/(f.grid.y+1));uint2 global=tile*f.grid.y+local;
    uint step=loadWord(target,3);if(all(global%step==0)){
        uint side=loadWord(target,2),offset=loadWord(target,5);uint at=offset+((global.y/step)*side+global.x/step)*3;
        buffers[NonUniformResourceIndex(target)].Store3(at*4,value);
    }
}
[shader("compute")][numthreads(64,1,1)]
void resetMain(uint id:SV_DispatchThreadID){Frame f=frame();
    if(id<6){uint step=1u<<id;uint cells=f.grid.y/step;buffers[NonUniformResourceIndex(f.more.x)].Store4(id*16,uint4(cells*cells*6,0,0,0));}
    if(id<9)buffers[NonUniformResourceIndex(f.more.z)].Store4(id*16,uint4(foliageVertices(id),0,0,0));
}
[shader("compute")][numthreads(64,1,1)]
void selectMain(uint id:SV_DispatchThreadID){Frame f=frame();if(id>=f.grid.z*f.grid.z)return;
    uint2 tile=uint2(id%f.grid.z,id/f.grid.z);float d=patchDistance(f,tile);uint level=d<=100?0:min(5u,uint(floor(log2(d/100)))+1);
    if(loadWord(f.indices.x,8+id)==0)level=max(level,2u);
    buffers[NonUniformResourceIndex(f.indices.z)].Store(id*4,level);
}
[shader("compute")][numthreads(64,1,1)]
void terrainCull(uint id:SV_DispatchThreadID){Frame f=frame();if(id>=f.grid.z*f.grid.z)return;
    uint2 tile=uint2(id%f.grid.z,id/f.grid.z);float w=f.grid.y*f.params.x;float2 xz=(tile+0.5)*w;float3 p=float3(xz.x,worldHeight(f,xz),xz.y);
    if(!sphereIn(f.vp,p,w*1.5)&&!sphereIn(f.lightVp,p,w*1.5))return;
    uint level=loadWord(f.indices.z,id),dst;buffers[NonUniformResourceIndex(f.more.x)].InterlockedAdd(level*16+4,1,dst);
    buffers[NonUniformResourceIndex(f.indices.w)].Store((level*f.grid.z*f.grid.z+dst)*4,id);
}
struct VertexOut {float4 position:SV_Position;float3 world:TEXCOORD0;float3 normal:TEXCOORD1;float2 uv:TEXCOORD2;nointerpolation uint species:TEXCOORD3;nointerpolation uint lod:TEXCOORD4;float transition:TEXCOORD5;};
struct TerrainOut {float4 position:SV_Position;float3 world:TEXCOORD0;float3 normal:TEXCOORD1;};
struct TerrainDepthOut {float4 position:SV_Position;float3 world:TEXCOORD0;};
[shader("vertex")]
TerrainOut terrainVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){
    Frame f=frame();uint id=loadWord(f.indices.w,root.group*f.grid.z*f.grid.z+instance);uint2 tile=uint2(id%f.grid.z,id/f.grid.z);
    uint step=1u<<root.group;uint edge=f.grid.y/step;uint q=vertex/6;uint corner=vertex%6;
    uint2 offsets[6]={uint2(0,0),uint2(0,1),uint2(1,0),uint2(1,0),uint2(0,1),uint2(1,1)};
    uint2 local=(uint2(q%edge,q/edge)+offsets[corner])*step;
    // Same coarse grid on either side of every shared edge, including >1 LOD gaps.
    if(local.x==0||local.x==f.grid.y){uint neighbor=tileLevel(f,int2(tile)+int2(local.x==0?-1:1,0));uint s=1u<<max(root.group,neighbor);local.y=local.y/s*s;}
    if(local.y==0||local.y==f.grid.y){uint neighbor=tileLevel(f,int2(tile)+int2(0,local.y==0?-1:1));uint s=1u<<max(root.group,neighbor);local.x=local.x/s*s;}
    float2 p=(tile*f.grid.y+local)*f.params.x;
    TerrainOut o;o.world=float3(p.x,worldHeight(f,p),p.y);o.normal=terrainNormal(f,p);
    o.position=mul(root.shadowPass!=0?f.lightVp:f.vp,float4(o.world,1));return o;
}
[shader("vertex")]
TerrainDepthOut terrainDepthVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){TerrainOut v=terrainVertex(vertex,instance);TerrainDepthOut o;o.position=v.position;o.world=v.world;return o;}
void terrainMask(Frame f,float2 p){int2 c=clamp(int2(p/f.params.x),0,int(f.grid.x-2));if(sampleData(f,c).z!=0)discard;}
[shader("fragment")]
void terrainDepth(TerrainDepthOut v){terrainMask(frame(),v.world.xz);}

// ara-material evaluator ABI adapter. The generated AST uses the renderer heap,
// triplanar colour sampling and declaration-ordered parameter rows.
struct AraMaterialInput {float2 uv;float3 worldPosition;float3 worldNormal;float3 worldTangent;float3 worldBitangent;float3 viewDirection;};
struct AraSurface {float3 baseColor;float metallic;float roughness;float3 normal;float3 emissive;float opacityMask;float clearCoat;float clearCoatRoughness;float transmission;float3 color;float opacity;float clip;};
float4 ara_load(uint offset){Frame f=frame();return asfloat(buffers[NonUniformResourceIndex(f.more.w)].Load4(offset*16));}
float3 linearColor(float3 c){return select(c<=0.04045,c/12.92,pow((c+0.055)/1.055,2.4));}
float3 ara_triplanar(uint index,float3 p,float3 n,float scale){
    float3 w=pow(abs(n),4);w/=max(dot(w,1),0.0001);
    float3 a=linearColor(textures[NonUniformResourceIndex(index)].Sample(samplers[0],p.yz*scale).rgb);
    float3 b=linearColor(textures[NonUniformResourceIndex(index)].Sample(samplers[0],p.xz*scale).rgb);
    float3 c=linearColor(textures[NonUniformResourceIndex(index)].Sample(samplers[0],p.xy*scale).rgb);
    return a*w.x+b*w.y+c*w.z;
}
#include "terrain_material.slang"
float shadowAt(Frame f,float3 p,float3 n){
    float4 lp=mul(f.lightVp,float4(p+n*0.15,1));float3 c=lp.xyz/lp.w;float2 uv=c.xy*float2(0.5,-0.5)+0.5;
    if(any(uv<0)||any(uv>1)||c.z<0||c.z>1)return 1;
    float sum=0;for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
        float d=textures[NonUniformResourceIndex(root.shadow)].SampleLevel(samplers[2],uv+float2(x,y)/2048.,0).r;
        sum+=c.z+0.0008>=d?1:0.18;
    }return sum/9;
}
float3 skyRadiance(Frame f,float3 dir){
    float noon=saturate(f.sun.y*2);float3 horizon=lerp(float3(0.45,0.24,0.16),float3(0.64,0.76,0.8),noon);
    float3 zenith=lerp(float3(0.045,0.06,0.13),float3(0.15,0.38,0.65),noon);
    float3 sky=lerp(horizon,zenith,pow(saturate(dir.y),0.45));
    sky+=f.sunColor.rgb*pow(saturate(dot(dir,f.sun.xyz)),600)*4;return sky;
}
float3 shade(Frame f,float3 p,float3 normal,float3 albedo,float transmission){
    float3 n=normalize(normal);float ndl=saturate(dot(n,f.sun.xyz));float shadow=shadowAt(f,p,n);
    float ambient=lerp(0.09,0.3,saturate(f.sun.y*2));float3 color=albedo*(ambient*float3(0.65,0.8,1.0)+f.sunColor.rgb*ndl*shadow);
    color+=albedo*transmission*pow(saturate(dot(-n,f.sun.xyz)),2)*f.sunColor.rgb*shadow*0.35;
    float d=length(p-f.camera.xyz);float fog=1-exp(-d*lerp(0.0007,0.00022,saturate(f.sun.y*2)));
    return lerp(color,skyRadiance(f,normalize(p-f.camera.xyz)),fog);
}
float3 displayColor(float3 c){c=max(c,0);c=(c*(2.51*c+0.03))/(c*(2.43*c+0.59)+0.14);return pow(saturate(c),1./2.2);}
[shader("fragment")]
float4 terrainColor(TerrainOut v):SV_Target{
    Frame f=frame();terrainMask(f,v.world.xz);int2 c=clamp(int2(v.world.xz/f.params.x),0,int(f.grid.x-1));
    uint w=sampleData(f,c).y;float rock=float((w>>8)&255)/255.;
    AraMaterialInput i;i.uv=float2(rock,0);i.worldPosition=v.world;i.worldNormal=normalize(v.normal);i.worldTangent=float3(1,0,0);i.worldBitangent=float3(0,0,1);i.viewDirection=normalize(f.camera.xyz-v.world);
    AraSurface s=ara_evaluate(i);return float4(displayColor(shade(f,v.world,v.normal,s.baseColor*2.4,0)),1);
}

groupshared float hzbReduce[64];
[shader("compute")][numthreads(8,8,1)]
void hzbMain(uint3 group:SV_GroupID,uint3 local:SV_GroupThreadID){
    Frame f=frame();uint2 base=group.xy*32;float minimum=1;
    for(uint y=local.y;y<32;y+=8)for(uint x=local.x;x<32;x+=8){uint2 p=base+uint2(x,y);if(all(p<f.extent.xy))minimum=min(minimum,textures[NonUniformResourceIndex(root.depth)].Load(int3(p,0)).r);}
    // Independent thread reduction through group-shared storage.
    hzbReduce[local.y*8+local.x]=minimum;GroupMemoryBarrierWithGroupSync();
    for(uint stride=32;stride>0;stride/=2){uint id=local.y*8+local.x;if(id<stride)hzbReduce[id]=min(hzbReduce[id],hzbReduce[id+stride]);GroupMemoryBarrierWithGroupSync();}
    if(local.x==0&&local.y==0)images[NonUniformResourceIndex(root.hzbOut)][group.xy]=float4(hzbReduce[0],0,0,0);
}
struct Instance {float4 positionScale;float4 rotationWind;uint4 identity;};
Instance instanceAt(Frame f,uint id){return buffers[NonUniformResourceIndex(f.indices.y)].Load<Instance>(id*48);}
bool occluded(Frame f,float3 p,float radius){
    float2 lo=1e9,hi=-1e9;float nearest=0;
    for(uint corner=0;corner<8;corner++){
        float3 q=p+float3((corner&1)?radius:-radius,(corner&2)?radius:-radius,(corner&4)?radius:-radius);
        float4 c=mul(f.vp,float4(q,1));if(c.w<=0)return false;
        float2 uv=c.xy/c.w*float2(0.5,-0.5)+0.5;lo=min(lo,uv);hi=max(hi,uv);nearest=max(nearest,c.z/c.w);
    }
    if(any(lo<0)||any(hi>=1))return false;
    int2 a=int2(lo*f.extent.xy/32),b=int2(hi*f.extent.xy/32);if(any(b-a>1))return false;
    float depth=1;for(int y=a.y;y<=b.y;y++)for(int x=a.x;x<=b.x;x++)depth=min(depth,textures[NonUniformResourceIndex(root.hzb)].Load(int3(x,y,0)).r);
    return nearest+0.00001<depth;
}
void emitFoliage(Frame f,uint group,uint id){uint dst;buffers[NonUniformResourceIndex(f.more.z)].InterlockedAdd(group*16+4,1,dst);buffers[NonUniformResourceIndex(f.more.y)].Store((group*f.grid.w+dst)*4,id);}
float2 bands(uint species){return species==0?float2(35,95):species==1?float2(180,550):float2(120,400);}
float maximumDistance(uint species){return species==0?220:species==1?2200:1600;}
[shader("compute")][numthreads(64,1,1)]
void foliageCull(uint id:SV_DispatchThreadID){Frame f=frame();if(id>=f.grid.w)return;Instance i=instanceAt(f,id);uint species=i.identity.z;
    float scale=i.positionScale.w;float3 p=i.positionScale.xyz;float radius=i.rotationWind.w+scale;
    float distance=length(p-f.camera.xyz);if(distance>maximumDistance(species))return;
    bool cameraVisible=sphereIn(f.vp,p,radius)&&!occluded(f,p,radius);bool shadowVisible=sphereIn(f.lightVp,p,radius);
    if(!cameraVisible&&!shadowVisible)return;float2 b=bands(species);uint lod=distance<b.x?0:distance<b.y?1:2;
    emitFoliage(f,species*3+lod,id);
    if(lod<2){float boundary=lod==0?b.x:b.y;if(distance>boundary*0.85)emitFoliage(f,species*3+lod+1,id);}
}
float2 quadUV(uint id){float2 uv[6]={float2(0,0),float2(0,1),float2(1,0),float2(1,0),float2(0,1),float2(1,1)};return uv[id%6];}
float3 wind(Frame f,Instance i,float3 local){float bend=sin(f.params.y*1.7+i.rotationWind.y+dot(i.positionScale.xz,float2(0.035,0.027)))*i.rotationWind.z;local.x+=bend*local.y*local.y*0.04;local.z+=bend*local.y*0.05;return local;}
struct AssetVertex{float3 position;float3 normal;float2 uv;};
[shader("vertex")]
VertexOut foliageVertex(uint vertex:SV_VertexID,uint index:SV_InstanceID){Frame f=frame();uint id=loadWord(f.more.y,root.group*f.grid.w+index);Instance i=instanceAt(f,id);
    uint species=root.group/3,lod=root.group%3;float2 uv=quadUV(vertex);float3 p=0,n=float3(0,0,1);uint face=vertex/6;
    float yaw=i.rotationWind.x;float3 right=float3(cos(yaw),0,sin(yaw));
    if(lod>0){float3 toward=normalize(f.camera.xyz-i.positionScale.xyz);right=normalize(float3(toward.z,0,-toward.x));n=normalize(cross(right,float3(0,1,0)));}
    if(f.geometry.x!=0){
        uint first=loadWord(f.geometry.y,root.group*8);uint flags=loadWord(f.geometry.y,root.group*8+3);
        AssetVertex asset=buffers[NonUniformResourceIndex(f.geometry.y)].Load<AssetVertex>(9*32+(first+vertex)*32);
        if((flags&1)!=0){float3 toward=normalize(f.camera.xyz-i.positionScale.xyz);right=normalize(float3(toward.z,0,-toward.x));}
        else right=float3(cos(yaw),0,sin(yaw));float3 forward=float3(-right.z,0,right.x);
        p=right*asset.position.x+float3(0,asset.position.y,0)+forward*asset.position.z;
        n=right*asset.normal.x+float3(0,asset.normal.y,0)+forward*asset.normal.z;uv=asset.uv;
    }
    else if(species==0){if(lod==0&&face==1){right=float3(-right.z,0,right.x);n=normalize(cross(right,float3(0,1,0)));}p=right*((uv.x-0.5)*0.9)+float3(0,uv.y*1.1,0);}
    else if(species==1){
        if(lod==2){p=right*((uv.x-0.5)*7)+float3(0,uv.y*12,0);}
        else if(lod==1){uint layer=face/2;float angle=yaw+(face%2)*1.5707963;right=float3(cos(angle),0,sin(angle));p=right*((uv.x-0.5)*(6.5-layer*1.4))+float3(0,2.5+layer*2.5+uv.y*4.5,0);n=normalize(cross(right,float3(0,1,0)));}
        else if(vertex<36){uint tri=vertex/3,corner=vertex%3;float angle=(tri/2)*1.5707963;float2 offset[3]={float2(0,0),float2(0,1),float2(1,0)};float2 t=offset[corner];if(tri%2==1)t=1-t;float a=angle+t.x*1.5707963;p=float3(cos(a)*0.28,t.y*8,sin(a)*0.28);n=normalize(float3(p.x,0,p.z));uv=float2(t.x,t.y);}
        else{uint k=vertex-36,tri=k/3,corner=k%3,layer=tri/8;float angle=(tri%8)*0.78539816;float radius=3.8-layer*0.85;float y=3.+layer*2.7;
            if(corner==2)p=float3(0,y+5.,0);else{float a=angle+corner*0.78539816;p=float3(cos(a)*radius,y,sin(a)*radius);}n=normalize(float3(p.x,0.6*radius,p.z));uv=float2(corner==0?0:1,corner==2?1:0);}
    }else{
        if(lod==2){p=right*((uv.x-0.5)*2)+float3(0,uv.y*1.6,0);}
        else{uint tri=vertex/3,corner=vertex%3;uint sides=lod==0?8:4;float a=(tri%sides)*6.2831853/sides+corner*6.2831853/sides;
            p=corner==2?float3(0,1.4,0):float3(cos(a),0.1,sin(a));n=normalize(float3(p.x,1,p.z));}
    }
    p=wind(f,i,p)*i.positionScale.w+i.positionScale.xyz;
    VertexOut o;o.world=p;o.normal=n;o.uv=uv;o.species=species;o.lod=lod;
    float distance=length(i.positionScale.xyz-f.camera.xyz);float2 b=bands(species);uint base=distance<b.x?0:distance<b.y?1:2;
    float boundary=base==0?b.x:b.y;float blend=base<2?saturate((distance-boundary*0.85)/(boundary*0.15)):0;
    o.transition=lod>base?-blend:1.-blend;o.position=mul(root.shadowPass!=0?f.lightVp:f.vp,float4(p,1));return o;
}
float coverage(VertexOut v){
    Frame f=frame();if(f.geometry.x!=0){uint slot=loadWord(f.geometry.y,root.group*8+2);float cutoff=asfloat(loadWord(f.geometry.y,root.group*8+7));return slot==0xffffffff?1:textures[NonUniformResourceIndex(slot)].Sample(samplers[0],v.uv).a-cutoff;}
    if(v.species==0){float width=(1-v.uv.y)*0.38+0.015;float blade=abs(v.uv.x-0.5);return width-blade;}
    if(v.species==1&&v.lod>0){float treeWidth=(1-v.uv.y)*0.45+0.025;float trunk=abs(v.uv.x-0.5)<0.025?1:0;
        float branch=0.04*sin(v.uv.y*60)+treeWidth;return max(branch-abs(v.uv.x-0.5),trunk>0?0.01:-1);}
    return 1;
}
void foliageMask(VertexOut v){
    if(coverage(v)<0)discard;
    uint2 p=uint2(v.position.xy);uint value=(p.x*1664525u+p.y*1013904223u);value^=value>>16;float noise=float(value&65535)/65536.;
    if(v.transition<0){if(noise>=-v.transition)discard;}else if(noise<1.-v.transition)discard;
}
[shader("fragment")]
void foliageDepth(VertexOut v){foliageMask(v);}
[shader("fragment")]
float4 foliageColor(VertexOut v,bool front:SV_IsFrontFace):SV_Target{
    foliageMask(v);Frame f=frame();float3 n=normalize(v.normal)*(front?1:-1);
    float3 albedo=v.species==0?lerp(float3(0.07,0.13,0.018),float3(0.23,0.32,0.055),v.uv.y):v.species==1?float3(0.055,0.13,0.035):float3(0.26,0.24,0.20);
    if(f.geometry.x!=0){uint slot=loadWord(f.geometry.y,root.group*8+2);uint flags=loadWord(f.geometry.y,root.group*8+3);float3 color=asfloat(buffers[NonUniformResourceIndex(f.geometry.y)].Load3(root.group*32+16));
        albedo=color;if(slot!=0xffffffff){float3 tex=textures[NonUniformResourceIndex(slot)].Sample(samplers[0],v.uv).rgb;albedo*=((flags&2)!=0)?linearColor(tex):tex;}}
    else if(v.species==1&&v.lod==0&&v.world.y-worldHeight(f,v.world.xz)<3.)albedo=float3(0.12,0.07,0.035);
    return float4(displayColor(shade(f,v.world,n,albedo,v.species<2?0.6:0)),1);
}
struct SkyOut{float4 position:SV_Position;};
[shader("vertex")]
SkyOut skyVertex(uint id:SV_VertexID){SkyOut o;float2 uv=float2((id<<1)&2,id&2);o.position=float4(uv*2-1,0,1);return o;}
[shader("fragment")]
float4 skyColor(SkyOut v):SV_Target{Frame f=frame();float2 xy=v.position.xy/f.extent.xy*float2(2,-2)+float2(-1,1);float4 world=mul(f.inverseVp,float4(xy,0.00001,1));float3 direction=normalize(world.xyz/world.w-f.camera.xyz);return float4(displayColor(skyRadiance(f,direction)),1);}
