// ara-ui-render's 64-byte instance ABI. Rounded ancestors intersect analytic masks.
struct Instance { float4 rect; float4 uv; float4 color; float4 parameters; };
[[vk::binding(0,0)]] StructuredBuffer<Instance> instances;
[[vk::binding(1,0)]] Texture2D<float4> textures[256];
[[vk::binding(2,0)]] SamplerState linearClamp;
// Two float4 records per rounded ancestor: rect and radius. Rect clips also use scissor.
[[vk::binding(3,0)]] StructuredBuffer<float4> masks;
struct Push { uint firstInstance; uint textureIndex; uint mode; uint maskOffset; float2 viewport; uint maskCount; float dpi; };
[[vk::push_constant]] ConstantBuffer<Push> pc;
struct Varying { float4 position:SV_Position; float2 local:TEXCOORD0; float2 uv:TEXCOORD1; nointerpolation uint instanceId:TEXCOORD2; };
float roundedDistance(float2 p,float2 size,float radius) {
    radius=clamp(radius,0,min(size.x,size.y)*0.5);float2 q=abs(p-size*0.5)-size*0.5+radius;
    return length(max(q,0))+min(max(q.x,q.y),0)-radius;
}
// What an outline or hover compares for entity-ID channel `id`: the channel itself, or
// with a group table (slot + 1, NativeUi::outline_groups) its hover group (red + green
// × 256) or outline colour group (blue's low 7 bits); bit 8 of the result marks ground
// (blue's bit 7: terrain). Texel: id % 4096, id / 4096.
uint outlineKey(uint table,uint id,bool hover){
    if(table==0||id==0)return id;
    uint4 g=uint4(round(textures[NonUniformResourceIndex(table-1)].Load(int3(id%4096,id/4096,0))*255));
    return (hover?g.r|(g.g<<8):g.b&127)|((g.b&128)<<9);
}
// Mode 9 fills (ara_ui::paint::Fill): sRGB transfer, full-saturation hue.
float3 srgbToLinear(float3 c){return select(c<=0.04045,c/12.92,pow((c+0.055)/1.055,2.4));}
float3 linearToSrgb(float3 c){return select(c<=0.0031308,c*12.92,1.055*pow(max(c,0),1.0/2.4)-0.055);}
float3 hueColor(float h){return saturate(abs(fmod(h*6+float3(0,4,2),6)-3)-1);}
// Linear gradient (kind 0; flags 1 vertical, 2 linear light), hue strip (1), hue ring
// (2: annulus of thickness z, 0 deg red at +x, counterclockwise) or checkerboard (3).
float4 fillColor(Instance i,float2 p,inout float shape){
    uint kind=uint(i.parameters.y+0.5);float2 size=i.rect.zw;
    if(kind==0){
        uint flags=uint(i.parameters.w+0.5);
        float t=saturate((flags&1)!=0?p.y/max(size.y,0.0001):p.x/max(size.x,0.0001));
        float4 c=lerp(i.color,i.uv,t);
        if((flags&2)!=0)c.rgb=linearToSrgb(lerp(srgbToLinear(i.color.rgb),srgbToLinear(i.uv.rgb),t));
        return c;
    }
    if(kind==1)return float4(hueColor(saturate(p.x/max(size.x,0.0001))),i.color.a);
    if(kind==2){
        float2 d=p-size*0.5;float r=length(d);float outer=min(size.x,size.y)*0.5;float inner=outer-i.parameters.z;
        float aa=max(fwidth(r),0.0001);
        shape=saturate((outer-r)/aa+0.5)*saturate((r-inner)/aa+0.5);
        float h=frac(atan2(-d.y,d.x)/6.28318530718+1);
        return float4(hueColor(h),i.color.a);
    }
    float cell=max(i.parameters.z,1);int2 q=int2(floor(p/cell));
    return ((q.x+q.y)&1)==0?i.color:i.uv;
}
[shader("vertex")]
Varying vertexMain(uint vertex:SV_VertexID,uint instance:SV_InstanceID) {
    static const float2 corners[6]={float2(0,0),float2(1,0),float2(0,1),float2(0,1),float2(1,0),float2(1,1)};
    Instance i=instances[pc.firstInstance+instance];float2 local=corners[vertex];float2 p=i.rect.xy+local*i.rect.zw;
    Varying o;o.position=float4(p/pc.viewport*float2(2,2)-1,0,1);o.local=local*i.rect.zw;o.uv=i.uv.xy+local*i.uv.zw;o.instanceId=pc.firstInstance+instance;return o;
}
[shader("fragment")]
float4 fragmentMain(Varying input):SV_Target {
    float coverage=1;
    for(uint n=0;n<pc.maskCount;n++) {
        float4 rect=masks[pc.maskOffset+2*n];float radius=masks[pc.maskOffset+2*n+1].x;
        float d=roundedDistance(input.position.xy/pc.dpi-rect.xy,rect.zw,radius);
        coverage=min(coverage,saturate(0.5-d/max(fwidth(d),0.0001)));
    }
    Instance i=instances[input.instanceId];float4 color=i.color;
    if(pc.mode==0) {
        float d=roundedDistance(input.local,i.rect.zw,i.parameters.x);float aa=max(fwidth(d),0.0001);
        float shape=saturate(0.5-d/aa);
        if(i.parameters.y>0) {float b=i.parameters.y;float2 inner=i.rect.zw-2*b;float hole=0;if(all(inner>0))hole=saturate(0.5-roundedDistance(input.local-b,inner,max(i.parameters.x-b,0))/aa);shape-=hole;}
        color.a*=shape;
    }else if(pc.mode==7){
        float2 screen=input.local/i.rect.zw;
        float2 ndc=screen*float2(2,-2)+float2(-1,1);
        float sy=sin(i.uv.w),cy=cos(i.uv.w),sp=sin(i.parameters.x),cp=cos(i.parameters.x);
        float3 forward=float3(-sy*cp,-sp,-cy*cp),right=float3(cy,0,-sy),up=float3(-sy*sp,cp,-cy*sp);
        float3 ray=normalize(forward+right*(ndc.x*i.parameters.y*i.rect.z/i.rect.w)+up*(ndc.y*i.parameters.y));
        float distance=-i.uv.y/(abs(ray.y)<0.00001?-0.00001:ray.y);
        float2 ground=(i.uv.xyz+ray*distance).xz;
        float2 cell=ground/i.parameters.z;
        float2 derivative=max(fwidth(cell),0.00001);
        float2 line=abs(frac(cell+0.5)-0.5)/derivative;
        float minor=1-saturate(min(line.x,line.y));
        float2 majorCell=cell/10;
        float2 major=abs(frac(majorCell+0.5)-0.5)/max(fwidth(majorCell),0.00001);
        color.a*=max(minor*0.45,(1-saturate(min(major.x,major.y)))*0.8);
        Instance axes=instances[input.instanceId+1];
        float xAxis=1-saturate(abs(ground.y)/max(fwidth(ground.y),0.0001));
        float zAxis=1-saturate(abs(ground.x)/max(fwidth(ground.x),0.0001));
        if(xAxis>0){color=axes.rect;color.a*=xAxis;}
        if(zAxis>0){color=axes.uv;color.a*=zAxis;}
        uint width,height;textures[NonUniformResourceIndex(pc.textureIndex)].GetDimensions(width,height);
        int2 pixel=clamp(int2(screen*float2(width,height)),int2(0,0),int2(width-1,height-1));
        bool covered=textures[NonUniformResourceIndex(pc.textureIndex)].Load(int3(pixel,0)).r>0;
        color.a*=distance>0&&!covered?exp(-distance/max(i.parameters.w,0.01))*smoothstep(0.01,0.1,abs(ray.y)):0;
    }else if(pc.mode==9){
        float d=roundedDistance(input.local,i.rect.zw,i.parameters.x);
        float shape=saturate(0.5-d/max(fwidth(d),0.0001));
        color=fillColor(i,input.local,shape);color.a*=shape;
    }else if(pc.mode==5){
        // Outline (z: the channel or colour group) or hover (bit 0 of w: the group under
        // xy). w's other bits: the outline group table's slot + 1 (NativeUi::outline_groups).
        uint width,height;textures[NonUniformResourceIndex(pc.textureIndex)].GetDimensions(width,height);
        int2 pixel=clamp(int2(input.uv*float2(width,height)),int2(0,0),int2(width-1,height-1));
        uint flags=uint(i.parameters.w+0.5);bool hover=(flags&1)!=0;uint table=flags>>1;
        uint wanted=uint(i.parameters.z);
        if(hover){int2 at=clamp(int2(i.parameters.xy*float2(width,height)),int2(0,0),int2(width-1,height-1));wanted=outlineKey(table,uint(textures[NonUniformResourceIndex(pc.textureIndex)].Load(int3(at,0)).r),true);}
        // The ground bit only exists with a table; raw channel IDs use every bit.
        uint GROUND=table!=0?(1u<<16):0;
        wanted&=~GROUND;
        uint centreId=uint(textures[NonUniformResourceIndex(pc.textureIndex)].Load(int3(pixel,0)).r);
        uint centre=outlineKey(table,centreId,hover);
        bool edge=false;
        static const int2 offsets[4]={int2(-1,0),int2(1,0),int2(0,-1),int2(0,1)};
        for(uint n=0;n<4;n++){
            int2 other=clamp(pixel+offsets[n]*max(1,int(round(1.25*pc.dpi))),int2(0,0),int2(width-1,height-1));
            uint otherId=uint(textures[NonUniformResourceIndex(pc.textureIndex)].Load(int3(other,0)).r);
            uint neighbor=outlineKey(table,otherId,hover);
            bool inside=(centre&~GROUND)==wanted;
            if(inside==((neighbor&~GROUND)==wanted))continue;
            // Ground (terrain) outlines against the sky, the void and other ground only:
            // what stands on it is in front of it, not its edge.
            uint inner=inside?centre:neighbor;uint outerId=inside?otherId:centreId;uint outer=inside?neighbor:centre;
            edge=edge||(inner&GROUND)==0||outerId==0||(outer&GROUND)!=0;
        }
        color.a*=edge&&wanted>0?1:0;
    }else if(pc.mode==8){
        // Convex quad: signed distance to the nearest edge, oriented by the winding.
        float2 corners[4]={i.uv.xy,i.uv.zw,i.parameters.xy,i.parameters.zw};float area=0;
        for(uint n=0;n<4;n++){float2 a=corners[n],b=corners[(n+1)%4];area+=a.x*b.y-b.x*a.y;}
        float side=area<0?-1:1;float inside=1e9;
        for(uint n=0;n<4;n++){float2 a=corners[n];float2 e=corners[(n+1)%4]-a;float l=length(e);if(l<0.0001)continue;inside=min(inside,side*(e.x*(input.local.y-a.y)-e.y*(input.local.x-a.x))/l);}
        color.a*=saturate(inside*pc.dpi+0.5);
    }else if(pc.mode==6){
        float2 a=i.uv.xy,b=i.uv.zw;float2 d=b-a;float t=saturate(dot(input.local-a,d)/max(dot(d,d),0.0001));float distance=length(input.local-a-t*d);
        color.a*=saturate((i.parameters.x-distance)*pc.dpi+0.5);
    }else{
        float4 sample=textures[NonUniformResourceIndex(pc.textureIndex)].Sample(linearClamp,input.uv);
        if(pc.mode==2)color.a*=sample.a;
        else if(pc.mode==3){float median=max(min(sample.r,sample.g),min(max(sample.r,sample.g),sample.b));uint width,height;textures[NonUniformResourceIndex(pc.textureIndex)].GetDimensions(width,height);float2 unitRange=2*i.parameters.z/float2(width,height);float screenRange=max(0.5*dot(unitRange,1/fwidth(input.uv)),1);color.a*=saturate((median-0.5)*screenRange+0.5);}
        else color*=sample;
    }
    color.a*=coverage;return float4(color.rgb*color.a,color.a);
}
