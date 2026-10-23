// One 16k virtual domain per sun clipmap / spot / cube face; 128-square pages.
struct Root{uint frame;uint a;uint b;uint c;uint d;uint e;uint f;uint g;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root:register(b0,space0);
#endif
BIND(0) Texture2D<float4> textures[128]:register(t0,space1);
BIND(1) RWByteAddressBuffer buffers[64]:register(u0,space2);
BIND(3) RWTexture2D<float4> images[64]:register(u0,space3);
struct Projection{float4x4 vp;float4 cameraFocal;uint4 control;};
struct Frame{Projection projections[6];uint4 counts;uint4 extent;uint4 slots0;uint4 slots1;uint4 slots2;uint4 geometry;float4 minimum;float4 scale;uint4 inputs;};
struct Cluster{float4 bounds;uint group;uint parent;uint vertices;uint triangles;uint page;uint vb;uint ib;uint level;};
struct Group{float4 bounds;float error;uint first;uint count;uint level;};
struct Instance{float4 positionScale;float4 colorRoughness;uint4 identity;};
Frame frame(){return buffers[NonUniformResourceIndex(root.frame)].Load<Frame>(0);}
uint word(uint slot,uint at){return buffers[NonUniformResourceIndex(slot)].Load(at*4);}
void store(uint slot,uint at,uint value){buffers[NonUniformResourceIndex(slot)].Store(at*4,value);}
void add(uint slot,uint at,uint value){uint ignored;buffers[NonUniformResourceIndex(slot)].InterlockedAdd(at*4,value,ignored);}
float4 sphere(float4 b,Instance i){return float4(b.xyz*i.positionScale.w+i.positionScale.xyz,b.w*i.positionScale.w);}
[shader("compute")][numthreads(64,1,1)]
void shadowReset(uint id:SV_DispatchThreadID){Frame f=frame();
    if(f.extent.z!=0){if(id<98304){store(f.slots0.x,id,0);store(f.slots0.y,id,0);}if(id<f.counts.y)buffers[NonUniformResourceIndex(f.slots0.z)].Store4(id*16,0);}
    if(id<32){store(f.slots1.w,id,0);if(id==0)store(f.slots1.w,0,384);}
    if(id==0){store(f.slots1.x,0,16);store(f.slots1.x,1,16);store(f.slots1.x,2,0);store(f.slots1.x,3,0);store(f.slots1.x,5,0);store(f.slots1.x,7,0);
        if(f.extent.z!=0){store(f.slots1.x,4,0);store(f.slots1.x,6,0);}}
}
bool projectionFor(Frame f,float3 world,out uint index,out float3 uvz){
    index=0;uvz=0;
    for(uint p=0;p<f.counts.x;p++){float4 h=mul(f.projections[p].vp,float4(world,1));if(h.w<=0)continue;
        float3 n=h.xyz/h.w;float3 q=float3(n.x*.5+.5,.5-n.y*.5,n.z);
        if(all(q.xy>2.0/16384)&&all(q.xy<1-2.0/16384)&&q.z>0&&q.z<1){index=p;uvz=q;return true;}}
    return false;
}
[shader("compute")][numthreads(8,8,1)]
void shadowMark(uint3 id:SV_DispatchThreadID){Frame f=frame();if(any(id.xy>=f.extent.xy))return;
    if(textures[NonUniformResourceIndex(f.slots2.z)].Load(int3(id.xy,0)).w==0)return;
    float3 world=textures[NonUniformResourceIndex(f.slots2.y)].Load(int3(id.xy,0)).xyz;
    uint projection;float3 uvz;if(!projectionFor(f,world,projection,uvz))return;int2 texel=int2(uvz.xy*16384);
    for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){
        uint2 page=uint2(texel+int2(x,y))/128;uint address=projection*16384+page.y*128+page.x;
        uint ignored;buffers[NonUniformResourceIndex(f.slots0.y)].InterlockedMax(address*4,f.counts.w,ignored);
    }
}
[shader("compute")][numthreads(64,1,1)]
void shadowAllocate(uint page:SV_DispatchThreadID){Frame f=frame();
        if(page>=f.counts.x*16384||word(f.slots0.y,page)!=f.counts.w)return;
        uint mapped=word(f.slots0.x,page),slot=mapped-1;bool fresh=mapped==0;
        if(fresh){bool found=false;
            for(uint attempt=0;attempt<16;attempt++){
                uint cursor;buffers[NonUniformResourceIndex(f.slots1.x)].InterlockedAdd(16,1,cursor);
                uint candidate=cursor%f.counts.y,old=word(f.slots0.z,candidate*4);
                if(old==0xffffffff || (old!=0&&word(f.slots0.y,old-1)==f.counts.w))continue;
                uint observed;buffers[NonUniformResourceIndex(f.slots0.z)].InterlockedCompareExchange(candidate*16,old,0xffffffff,observed);
                if(observed!=old)continue;
                slot=candidate;found=true;if(old!=0)store(f.slots0.x,old-1,0);break;}
            if(!found){add(f.slots1.x,5,1);return;}
            store(f.slots0.x,page,slot+1);store(f.slots0.z,slot*4,page+1);
        }
        Projection projection=f.projections[page/16384];uint revision=projection.control.y^(projection.control.z*747796405u);
        bool invalid=fresh||word(f.slots0.z,slot*4+3)!=revision;
        store(f.slots0.z,slot*4+1,f.counts.w);store(f.slots0.z,slot*4+3,revision);
        if(invalid){uint dirty;buffers[NonUniformResourceIndex(f.slots1.x)].InterlockedAdd(12,1,dirty);store(f.slots0.z,slot*4+2,f.counts.w);store(f.slots1.y,dirty,slot);}else add(f.slots1.x,7,1);
}
[shader("compute")][numthreads(1,1,1)]
void shadowFinalize(){Frame f=frame();uint dirty=word(f.slots1.x,3);store(f.slots1.x,2,dirty);
    uint total=uint(f.minimum.w)*uint(f.scale.w)*f.counts.x,groups=(total+63)/64;
    buffers[NonUniformResourceIndex(f.slots1.x)].Store3(32,uint3(dirty==0?0:min(groups,65535),max(1,(groups+65534)/65535),1));
}
[shader("compute")][numthreads(8,8,1)]
void shadowClear(uint3 group:SV_GroupID,uint3 id:SV_DispatchThreadID){Frame f=frame();uint slot=word(f.slots1.y,group.z);
    store(f.slots0.w,slot*16384+id.y*128+id.x,0);
}
bool boundsUv(Projection p,float4 b,out float2 low,out float2 high){
    low=1e30;high=-1e30;
    for(uint k=0;k<8;k++){float3 v=b.xyz+b.w*float3((k&1)!=0?1:-1,(k&2)!=0?1:-1,(k&4)!=0?1:-1);float4 h=mul(p.vp,float4(v,1));if(h.w<=0.001)return false;
        float2 uv=float2(h.x/h.w*.5+.5,.5-h.y/h.w*.5);low=min(low,uv);high=max(high,uv);}
    return true;
}
bool dirtyBounds(Frame f,uint projection,float4 b){
    float2 low,high;if(!boundsUv(f.projections[projection],b,low,high))return true;
    if(any(high<0)||any(low>1))return false;
    uint2 a=uint2(saturate(low)*127.999),z=uint2(saturate(high)*127.999);if(any(z-a>16))return true;
    for(uint y=a.y;y<=z.y;y++)for(uint x=a.x;x<=z.x;x++){uint mapped=word(f.slots0.x,projection*16384+y*128+x);
        if(mapped!=0 && word(f.slots0.z,(mapped-1)*4+2)==f.counts.w)return true;}
    return false;
}
bool acceptable(Frame f,uint group,Instance i,uint projection){if(group==0xffffffff)return true;
    Group g=buffers[NonUniformResourceIndex(f.geometry.z)].Load<Group>(group*32);Projection p=f.projections[projection];float4 b=sphere(g.bounds,i);
    if(g.level<=f.inputs.y)return true;
    float distance=p.control.x==0?1:max(length(b.xyz-p.cameraFocal.xyz)-b.w,.001);
    return g.error*i.positionScale.w*p.cameraFocal.w/distance<=1;
}
[shader("compute")][numthreads(64,1,1)]
void shadowSelect(uint3 gid:SV_GroupID,uint lane:SV_GroupIndex){Frame f=frame();uint id=(gid.y*65535+gid.x)*64+lane;
    uint clusters=uint(f.scale.w),instances=uint(f.minimum.w),count=clusters*instances;if(id>=count*f.counts.x)return;
    uint projection=id/count,local=id%count,cid=local%clusters,iid=local/clusters;
    Cluster c=buffers[NonUniformResourceIndex(f.geometry.y)].Load<Cluster>(cid*48);Instance i=buffers[NonUniformResourceIndex(f.geometry.x)].Load<Instance>(iid*48);
    if(!acceptable(f,c.group,i,projection)||(c.parent!=0xffffffff&&acceptable(f,c.parent,i,projection)))return;
    if(!dirtyBounds(f,projection,sphere(c.bounds,i)))return;
    uint index;buffers[NonUniformResourceIndex(f.slots1.w)].InterlockedAdd(4,1,index);
    if(index>=f.counts.z){add(f.slots1.w,4,1);return;}
    buffers[NonUniformResourceIndex(f.slots1.z)].Store4(index*16,uint4(cid,iid,projection,0));
}
[shader("compute")][numthreads(1,1,1)]
void shadowArguments(){Frame f=frame();store(f.slots1.w,1,min(f.counts.z,word(f.slots1.w,1)));}
uint byteAt(uint slot,uint at){return (buffers[NonUniformResourceIndex(slot)].Load(at&~3u)>>((at&3)*8))&255;}
uint pageBase(Frame f,Cluster c){return word(f.inputs.x,c.page/2048)*2048;}
float3 position(Frame f,Cluster c,Instance i,uint vertex){uint2 p=buffers[NonUniformResourceIndex(f.geometry.w)].Load2(pageBase(f,c)+vertex*16);
    return (f.minimum.xyz+float3(p.x&65535,p.x>>16,p.y&65535)*f.scale.xyz)*i.positionScale.w+i.positionScale.xyz;}
struct ShadowVertex{float4 position:SV_Position;nointerpolation uint projection:TEXCOORD0;};
[shader("vertex")]
ShadowVertex shadowVertex(uint vertex:SV_VertexID,uint iid:SV_InstanceID){Frame f=frame();uint4 item=buffers[NonUniformResourceIndex(f.slots1.z)].Load4(iid*16);
    Cluster c=buffers[NonUniformResourceIndex(f.geometry.y)].Load<Cluster>(item.x*48);Instance i=buffers[NonUniformResourceIndex(f.geometry.x)].Load<Instance>(item.y*48);
    ShadowVertex o;o.position=float4(2,2,0,1);o.projection=item.z;uint tri=vertex/3;if(tri>=c.triangles)return o;
    uint at=pageBase(f,c)+1024+tri*3+vertex%3;uint v=byteAt(f.geometry.w,at);o.position=mul(f.projections[item.z].vp,float4(position(f,c,i,v),1));return o;
}
[shader("fragment")]
void shadowFragment(ShadowVertex input){Frame f=frame();uint2 texel=uint2(input.position.xy);if(any(texel>=16384))return;
    uint page=input.projection*16384+(texel.y/128)*128+texel.x/128,mapped=word(f.slots0.x,page);if(mapped==0)return;uint slot=mapped-1;
    if(word(f.slots0.z,slot*4+2)!=f.counts.w)return;
    uint at=slot*16384+(texel.y%128)*128+texel.x%128;uint ignored;
    buffers[NonUniformResourceIndex(f.slots0.w)].InterlockedMax(at*4,asuint(input.position.z),ignored);
}
[shader("compute")][numthreads(8,8,1)]
void shadowResolve(uint3 id:SV_DispatchThreadID){Frame f=frame();if(any(id.xy>=f.extent.xy))return;
    float4 normal=textures[NonUniformResourceIndex(f.slots2.z)].Load(int3(id.xy,0));float result=1,resident=1;
    if(normal.w!=0){float4 surface=textures[NonUniformResourceIndex(f.slots2.y)].Load(int3(id.xy,0));float3 world=surface.xyz;uint projection;float3 uvz;
        if(projectionFor(f,world,projection,uvz)){int2 texel=int2(uvz.xy*16384);result=0;
            Projection light=f.projections[projection];float4 h=mul(light.vp,float4(world,1));
            float zScale=length(float3(light.vp[2][0],light.vp[2][1],light.vp[2][2]));
            float bias=(surface.w+2.0/max(light.cameraFocal.w,1.0)+0.001)*zScale/max(h.w,0.001);
            for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++){uint2 tap=uint2(texel+int2(x,y)),page=tap/128;uint mapped=word(f.slots0.x,projection*16384+page.y*128+page.x);
                if(mapped==0){resident=0;result+=1.0/9;continue;}
                uint slot=mapped-1;float depth=asfloat(word(f.slots0.w,slot*16384+(tap.y%128)*128+tap.x%128));
                result+=(uvz.z+bias>=depth?1.0:0.0)/9;
            }}
        float4 color=images[NonUniformResourceIndex(f.slots2.w)][id.xy];color.rgb=pow(pow(color.rgb,float3(2.2))*(.2+.8*result),float3(1.0/2.2));images[NonUniformResourceIndex(f.slots2.w)][id.xy]=color;
    }
    images[NonUniformResourceIndex(f.slots2.x)][id.xy]=float4(result,result,result,resident);
}
