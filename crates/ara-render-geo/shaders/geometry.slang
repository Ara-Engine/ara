// Root32 / bindless v1. Shared with Rust Frame; column-major, reversed-Z.
struct Root {uint frame;uint phase;uint a;uint b;uint c;uint d;uint e;uint f;};
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root:register(b0,space0);
#endif
BIND(1) RWByteAddressBuffer buffers[64]:register(u0,space2);
BIND(3) RWTexture2D<float4> images[64]:register(u0,space3);
struct Frame {
    float4x4 vp;float4x4 inverseVp;float4x4 previousVp;
    float4 cameraError;uint4 extent;uint4 counts;
    uint4 slots0;uint4 slots1;uint4 slots2;uint4 slots3;
    uint4 outputs;float4 minimum;float4 scale;uint4 policy;
    float4 forward;uint4 shadow;
};
struct Cluster {float4 bounds;uint group;uint parent;uint vertices;uint triangles;uint page;uint vb;uint ib;uint level;};
struct Group {float4 bounds;float error;uint first;uint count;uint level;};
struct Instance {float4 positionScale;float4 colorRoughness;uint4 identity;};
Frame frame(){return buffers[NonUniformResourceIndex(root.frame)].Load<Frame>(0);}
uint word(uint slot,uint index){return buffers[NonUniformResourceIndex(slot)].Load(index*4);}
void store(uint slot,uint index,uint value){buffers[NonUniformResourceIndex(slot)].Store(index*4,value);}
void add(uint slot,uint index,uint value){uint ignored;buffers[NonUniformResourceIndex(slot)].InterlockedAdd(index*4,value,ignored);}
void mark(uint slot,uint index){uint ignored;buffers[NonUniformResourceIndex(slot)].InterlockedOr(index*4,1,ignored);}
Cluster cluster(Frame f,uint id){return buffers[NonUniformResourceIndex(f.slots0.y)].Load<Cluster>(id*48);}
Instance instance(Frame f,uint id){return buffers[NonUniformResourceIndex(f.slots0.x)].Load<Instance>(id*48);}
float4 sphere(float4 local,Instance i){return float4(local.xyz*i.positionScale.w+i.positionScale.xyz,local.w*i.positionScale.w);}
uint linear(uint3 group,uint lane){return (group.y*65535+group.x)*64+lane;}
uint queueIndex(uint3 group){return group.y*65535+group.x;}

bool projected(float4x4 vp,float4 b,out float2 lo,out float2 hi,out float nearest) {
    lo=float2(1e30);hi=float2(-1e30);nearest=0;
    for(uint c=0;c<8;c++) {
        float3 p=b.xyz+b.w*float3((c&1)!=0?1:-1,(c&2)!=0?1:-1,(c&4)!=0?1:-1);
        float4 h=mul(vp,float4(p,1));if(h.w<=0.001)return false;
        float3 n=h.xyz/h.w;float2 uv=float2(n.x*.5+.5,.5-n.y*.5);
        lo=min(lo,uv);hi=max(hi,uv);nearest=max(nearest,n.z);
    }return true;
}
bool frustum(float4x4 vp,float4 b){
    // Sphere support against the six clip-space half-planes. Near is z<=w;
    // far is z>=0 in reversed-Z. No normalized-plane assumption.
    float4 p=float4(b.xyz,1);
    float4 r0=float4(vp[0][0],vp[0][1],vp[0][2],vp[0][3]);
    float4 r1=float4(vp[1][0],vp[1][1],vp[1][2],vp[1][3]);
    float4 r2=float4(vp[2][0],vp[2][1],vp[2][2],vp[2][3]);
    float4 r3=float4(vp[3][0],vp[3][1],vp[3][2],vp[3][3]);
    float4 planes[6]={r3+r0,r3-r0,r3+r1,r3-r1,r2,r3-r2};
    for(uint k=0;k<6;k++)if(dot(planes[k],p)<-b.w*length(planes[k].xyz))return false;
    return true;
}
bool occluded(Frame f,float4x4 vp,float4 b){
    float2 lo,hi;float nearest;if(!projected(vp,b,lo,hi,nearest))return false;
    if(any(lo<0)||any(hi>=1)||nearest<=0||nearest>=1)return false;
    uint2 a=uint2(lo*f.extent.xy)/16,bp=uint2(hi*f.extent.xy)/16;
    if(any(bp-a>3))return false; // conservative large-object escape
    uint z=0xffffffff;
    for(uint y=a.y;y<=bp.y;y++)for(uint x=a.x;x<=bp.x;x++)z=min(z,word(f.slots2.x,y*f.extent.z+x));
    return nearest+1e-6<asfloat(z);
}
[shader("compute")][numthreads(64,1,1)]
void resetMain(uint3 gid:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint id=linear(gid,lane);
    if(root.phase==1){if(id==0){uint base=word(f.slots1.z,12);store(f.slots1.z,1,0);store(f.slots1.z,11,base);}return;}
    if(id<32)store(f.slots1.z,id,0);
    if(id==0)store(f.slots1.z,0,384);
    if(id<f.extent.x*f.extent.y){buffers[NonUniformResourceIndex(f.slots1.w)].Store<uint64_t>(id*8,0);store(f.slots2.z,id,0);}
    if(id<f.counts.y+8)store(f.slots2.y,id,0);
}
[shader("compute")][numthreads(64,1,1)]
void instanceMain(uint3 gid:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint id=linear(gid,lane);if(id>=f.counts.x)return;
    if(root.phase==1 && word(f.slots1.x,id)!=0)return;
    Instance i=instance(f,id);float4 bounds=buffers[NonUniformResourceIndex(f.slots3.x)].Load<float4>(0);
    float4 b=sphere(bounds,i);uint state=3;
    if(frustum(f.vp,b)){
        bool hidden=(root.phase==1 || f.policy.x==0)&&occluded(f,root.phase==0?f.previousVp:f.vp,b);
        state=hidden?(root.phase==0?0:3):(root.phase==0?1:2);
        if(hidden)add(f.slots1.z,root.phase==0?17:18,1);
    }else add(f.slots1.z,16,1);
    store(f.slots1.x,id,state);
}
bool acceptable(Frame f,uint id,Instance i){
    if(id==0xffffffff)return true;
    Group g=buffers[NonUniformResourceIndex(f.slots0.z)].Load<Group>(id*32);
    if(g.level<=f.slots3.z)return true;
    float4 b=sphere(g.bounds,i);float distance=max(length(b.xyz-f.cameraError.xyz)-b.w,0.001);
    return g.error*i.positionScale.w*f.minimum.w/distance<=f.cameraError.w;
}
bool desired(Frame f,uint id,Instance i){
    if(id==0xffffffff)return true;Group g=buffers[NonUniformResourceIndex(f.slots0.z)].Load<Group>(id*32);
    float4 b=sphere(g.bounds,i);float distance=max(length(b.xyz-f.cameraError.xyz)-b.w,0.001);
    return g.error*i.positionScale.w*f.minimum.w/distance<=f.cameraError.w;
}
[shader("compute")][numthreads(64,1,1)]
void streamUpload(uint3 group:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint packet=group.x*516;uint4 meta=buffers[NonUniformResourceIndex(f.slots3.w)].Load4(packet*4);
    for(uint i=lane;i<512;i+=64)store(f.slots0.w,meta.y*512+i,word(f.slots3.w,packet+4+i));
    DeviceMemoryBarrierWithGroupSync();
    if(lane==0){if(meta.z!=0xffffffff)store(f.slots3.y,meta.z,0xffffffff);store(f.slots3.y,meta.x,meta.y);}
}
[shader("compute")][numthreads(64,1,1)]
void selectMain(uint3 gid:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint id=linear(gid,lane);if(id>=f.counts.x*f.counts.y)return;
    uint iid=id/f.counts.y,cid=id%f.counts.y;if(word(f.slots1.x,iid)!=(root.phase==0?1:2))return;
    Cluster c=cluster(f,cid);Instance i=instance(f,iid);
    if(f.slots3.z>0){bool wanted=f.policy.w!=0?c.group==0xffffffff:desired(f,c.group,i)&&(c.parent==0xffffffff||!desired(f,c.parent,i));
        if(wanted&&frustum(f.vp,sphere(c.bounds,i)))mark(f.slots2.y,cid);}
    bool chosen=f.policy.w!=0&&f.slots3.z==0?c.group==0xffffffff:acceptable(f,c.group,i)&&(c.parent==0xffffffff||!acceptable(f,c.parent,i));
    if(!chosen)return;
    if(!frustum(f.vp,sphere(c.bounds,i))){add(f.slots1.z,19,1);return;}
    mark(f.slots2.y,cid);
    uint index;buffers[NonUniformResourceIndex(f.slots1.z)].InterlockedAdd(4,1,index);
    uint base=word(f.slots1.z,11);if(base+index>=f.counts.w){add(f.slots1.z,15,1);return;}
    buffers[NonUniformResourceIndex(f.slots1.y)].Store2((base+index)*8,uint2(cid,iid));
}
[shader("compute")][numthreads(1,1,1)]
void argumentMain(){
    Frame f=frame();uint base=word(f.slots1.z,11),n=min(word(f.slots1.z,1),f.counts.w-base);
    store(f.slots1.z,1,n);store(f.slots1.z,12,base+n);
    uint3 dispatch=uint3(min(n,65535),max(1,(n+65534)/65535),1);
    buffers[NonUniformResourceIndex(f.slots1.z)].Store3(16,dispatch);
    buffers[NonUniformResourceIndex(f.slots1.z)].Store3(32,dispatch);
}
uint byteAt(uint slot,uint at){return (buffers[NonUniformResourceIndex(slot)].Load(at&~3u)>>((at&3)*8))&255;}
uint pageBase(Frame f,Cluster c){return word(f.slots3.y,c.page/2048)*2048;}
uint3 triangle(Frame f,Cluster c,uint t){uint at=pageBase(f,c)+1024+t*3;return uint3(byteAt(f.slots0.w,at),byteAt(f.slots0.w,at+1),byteAt(f.slots0.w,at+2));}
float3 localPosition(Frame f,Cluster c,uint vertex){
    uint2 p=buffers[NonUniformResourceIndex(f.slots0.w)].Load2(pageBase(f,c)+vertex*16);
    return f.minimum.xyz+float3(p.x&65535,p.x>>16,p.y&65535)*f.scale.xyz;
}
float3 vertexNormal(Frame f,Cluster c,uint vertex){
    uint3 p=buffers[NonUniformResourceIndex(f.slots0.w)].Load3(pageBase(f,c)+vertex*16);
    int3 n=int3(int(p.y)>>16,int(p.z<<16)>>16,int(p.z)>>16);return normalize(float3(n)/32767.0);
}
float3 worldPosition(Frame f,Cluster c,Instance i,uint vertex){return localPosition(f,c,vertex)*i.positionScale.w+i.positionScale.xyz;}
float2 screen(Frame f,float4 p){return float2(p.x/p.w*.5+.5,.5-p.y/p.w*.5)*f.extent.xy;}
bool smallTriangle(Frame f,float4 a,float4 b,float4 c){
    if(f.policy.z!=0 || min(a.w,min(b.w,c.w))<=0.001 || min(a.z,min(b.z,c.z))<0 || max(a.z-a.w,max(b.z-b.w,c.z-c.w))>0)return false;
    float2 sa=screen(f,a),sb=screen(f,b),sc=screen(f,c);float2 low=min(sa,min(sb,sc)),high=max(sa,max(sb,sc));
    return all(ceil(high)-floor(low)<=8);
}
void visibility(Frame f,uint2 pixel,float depth,uint draw,uint tri){
    if(any(pixel>=f.extent.xy)||depth<=0||depth>1)return;
    uint index=pixel.y*f.extent.x+pixel.x;
    uint64_t value=(uint64_t(asuint(depth))<<32)|uint64_t((draw<<7)|tri);
    uint64_t ignored;buffers[NonUniformResourceIndex(f.slots1.w)].InterlockedMax64(index*8,value,ignored);
    add(f.slots2.z,index,1);
}
int edge(int2 a,int2 b,int2 p){int2 u=b-a,v=p-a;return u.x*v.y-u.y*v.x;}
int bias(int2 a,int2 b){int2 d=b-a;return d.y<0||(d.y==0&&d.x>0)?0:-1;}
[shader("compute")][numthreads(128,1,1)]
void softwareMain(uint3 gid:SV_GroupID,uint lane:SV_GroupIndex){
    Frame f=frame();uint q=queueIndex(gid);if(q>=word(f.slots1.z,1))return;q+=word(f.slots1.z,11);
    uint2 item=buffers[NonUniformResourceIndex(f.slots1.y)].Load2(q*8);Cluster c=cluster(f,item.x);if(lane>=c.triangles)return;
    Instance i=instance(f,item.y);uint3 ids=triangle(f,c,lane);
    float4 a=mul(f.vp,float4(worldPosition(f,c,i,ids.x),1)),b=mul(f.vp,float4(worldPosition(f,c,i,ids.y),1)),d=mul(f.vp,float4(worldPosition(f,c,i,ids.z),1));
    bool small=smallTriangle(f,a,b,d);add(f.slots2.y,f.counts.y+(small?0:1),1);if(!small)return;
    int2 pa=int2(round(screen(f,a)*256)),pb=int2(round(screen(f,b)*256)),pc=int2(round(screen(f,d)*256));
    float3 z=float3(a.z/a.w,b.z/b.w,d.z/d.w);int area=edge(pa,pb,pc);if(area==0)return;
    if(area<0){int2 swap=pb;pb=pc;pc=swap;float temp=z.y;z.y=z.z;z.z=temp;area=-area;}
    int2 lo=max(int2(0),int2(floor(float2(min(pa,min(pb,pc)))/256))),hi=min(int2(f.extent.xy)-1,int2(ceil(float2(max(pa,max(pb,pc)))/256)));
    for(int y=lo.y;y<=hi.y;y++)for(int x=lo.x;x<=hi.x;x++){
        int2 p=int2(x,y)*256+128;int3 e=int3(edge(pb,pc,p),edge(pc,pa,p),edge(pa,pb,p));
        if(any(e+int3(bias(pb,pc),bias(pc,pa),bias(pa,pb))<0))continue;
        visibility(f,uint2(x,y),dot(float3(e)/area,z),q,lane);
    }
}
struct RasterVertex {float4 position:SV_Position;nointerpolation uint draw:TEXCOORD0;nointerpolation uint tri:TEXCOORD1;};
[shader("vertex")]
RasterVertex hardwareVertex(uint vertex:SV_VertexID,uint instanceId:SV_InstanceID){
    Frame f=frame();uint q=instanceId+word(f.slots1.z,11),t=vertex/3;
    uint2 item=buffers[NonUniformResourceIndex(f.slots1.y)].Load2(q*8);Cluster c=cluster(f,item.x);Instance i=instance(f,item.y);
    RasterVertex o;o.draw=q;o.tri=t;o.position=float4(2,2,0,1);if(t>=c.triangles)return o;
    uint3 ids=triangle(f,c,t);float4 a=mul(f.vp,float4(worldPosition(f,c,i,ids.x),1)),b=mul(f.vp,float4(worldPosition(f,c,i,ids.y),1)),d=mul(f.vp,float4(worldPosition(f,c,i,ids.z),1));
    if(!smallTriangle(f,a,b,d))o.position=vertex%3==0?a:vertex%3==1?b:d;return o;
}
struct MeshVertex {float4 position:SV_Position;nointerpolation uint draw:TEXCOORD0;};
struct MeshPrimitive {uint tri:SV_PrimitiveID;};
[shader("mesh")][numthreads(64,1,1)][outputtopology("triangle")]
void meshMain(uint3 gid:SV_GroupID,uint lane:SV_GroupIndex,out vertices MeshVertex verts[64],out indices uint3 tris[128],out primitives MeshPrimitive attributes[128]){
    SetMeshOutputCounts(64,128);
    Frame f=frame();uint local=queueIndex(gid);bool valid=local<word(f.slots1.z,1);
    uint q=(valid?local:0)+word(f.slots1.z,11);uint2 item=buffers[NonUniformResourceIndex(f.slots1.y)].Load2(q*8);Cluster c=cluster(f,item.x);Instance i=instance(f,item.y);
    MeshVertex v;v.position=float4(2,2,0,1);v.draw=q;
    if(valid && lane<c.vertices)v.position=mul(f.vp,float4(worldPosition(f,c,i,lane),1));verts[lane]=v;
    for(uint t=lane;t<128;t+=64){uint3 ids=uint3(0);bool active=valid&&t<c.triangles;if(active)ids=triangle(f,c,t);
        float4 a=mul(f.vp,float4(worldPosition(f,c,i,ids.x),1)),b=mul(f.vp,float4(worldPosition(f,c,i,ids.y),1)),d=mul(f.vp,float4(worldPosition(f,c,i,ids.z),1));
        tris[t]=!active||smallTriangle(f,a,b,d)?uint3(0):ids;MeshPrimitive p;p.tri=t;attributes[t]=p;}
}
[shader("fragment")]
void hardwareFragment(float4 position:SV_Position,nointerpolation uint draw:TEXCOORD0,nointerpolation uint tri:TEXCOORD1){visibility(frame(),uint2(position.xy),position.z,draw,tri);}
[shader("fragment")]
void meshFragment(float4 position:SV_Position,nointerpolation uint draw:TEXCOORD0,uint tri:SV_PrimitiveID){visibility(frame(),uint2(position.xy),position.z,draw,tri);}
groupshared uint minimumDepth;
[shader("compute")][numthreads(16,16,1)]
void hzbMain(uint3 gid:SV_GroupID,uint3 tid:SV_DispatchThreadID,uint lane:SV_GroupIndex){
    Frame f=frame();if(lane==0)minimumDepth=0xffffffff;GroupMemoryBarrierWithGroupSync();
    uint z=0;if(all(tid.xy<f.extent.xy))z=uint(buffers[NonUniformResourceIndex(f.slots1.w)].Load<uint64_t>((tid.y*f.extent.x+tid.x)*8)>>32);
    InterlockedMin(minimumDepth,z);GroupMemoryBarrierWithGroupSync();if(lane==0)store(f.slots2.x,gid.y*f.extent.z+gid.x,minimumDepth);
}
float3 colour(uint id){id=id*747796405u+2891336453u;id=((id>>((id>>28)+4))^id)*277803737u;id=(id>>22)^id;return .15+.85*float3(id&255,(id>>8)&255,(id>>16)&255)/255.0;}
float3 barycentric(float2 p,float2 a,float2 b,float2 c){float2 u=b-a,v=c-a,w=p-a;float det=u.x*v.y-u.y*v.x;if(abs(det)<1e-12)return float3(1,0,0);float y=(w.x*v.y-w.y*v.x)/det,z=(u.x*w.y-u.y*w.x)/det;return float3(1-y-z,y,z);}
[shader("compute")][numthreads(8,8,1)]
void resolveMain(uint3 id:SV_DispatchThreadID){
    Frame f=frame();if(any(id.xy>=f.extent.xy))return;uint pixel=id.y*f.extent.x+id.x;
    uint64_t value=buffers[NonUniformResourceIndex(f.slots1.w)].Load<uint64_t>(pixel*8);
    float4 base=0,normal=0,world=0;float3 color=float3(.035,.055,.085);uint2 identity=0;
    if(value!=0){uint packed=uint(value),q=packed>>7,t=packed&127;uint2 item=buffers[NonUniformResourceIndex(f.slots1.y)].Load2(q*8);
        Cluster c=cluster(f,item.x);Instance i=instance(f,item.y);uint3 ids=triangle(f,c,t);
        float3 a=worldPosition(f,c,i,ids.x),b=worldPosition(f,c,i,ids.y),d=worldPosition(f,c,i,ids.z);
        float4 ca=mul(f.vp,float4(a,1)),cb=mul(f.vp,float4(b,1)),cd=mul(f.vp,float4(d,1));
        float3 weights=barycentric(float2(id.xy)+.5,screen(f,ca),screen(f,cb),screen(f,cd))/float3(ca.w,cb.w,cd.w);weights/=dot(weights,1);
        float3 position=a*weights.x+b*weights.y+d*weights.z;
        float3 n=normalize(vertexNormal(f,c,ids.x)*weights.x+vertexNormal(f,c,ids.y)*weights.y+vertexNormal(f,c,ids.z)*weights.z);
        float error=c.group==0xffffffff?length(f.scale.xyz)*.5:buffers[NonUniformResourceIndex(f.slots0.z)].Load<Group>(c.group*32).error;
        base=i.colorRoughness;normal=float4(n,dot(position-f.cameraError.xyz,f.forward.xyz));world=float4(position,error*i.positionScale.w);
        identity=uint2(item.y+1,item.x*128+t);color=base.rgb*(.15+.85*saturate(dot(n,normalize(float3(-.6,1,.3)))));
        if(f.policy.y==1)color=colour(item.x);if(f.policy.y==2)color=colour(c.level+1);
        if(f.policy.y==4)color=smallTriangle(f,ca,cb,cd)?float3(.95,.95,.95):float3(.1,.3,.9);
    }
    if(f.policy.y==3){float over=word(f.slots2.z,pixel)/8.0;color=float3(saturate(over),saturate(over-1),0);}
    buffers[NonUniformResourceIndex(f.slots2.w)].Store2(pixel*8,identity);
    images[NonUniformResourceIndex(f.outputs.x)][id.xy]=base;images[NonUniformResourceIndex(f.outputs.y)][id.xy]=normal;images[NonUniformResourceIndex(f.outputs.z)][id.xy]=world;
    images[NonUniformResourceIndex(f.outputs.w)][id.xy]=float4(pow(saturate(color),float3(1.0/2.2)),1);
}
