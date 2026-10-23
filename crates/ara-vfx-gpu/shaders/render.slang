struct Screen {float4 clip:SV_Position;float2 uv:TEXCOORD0;};
[shader("vertex")]
Screen screenVertex(uint id:SV_VertexID){Screen o;float2 uv=float2((id<<1)&2,id&2);o.clip=float4(uv*2-1,0,1);o.uv=float2(uv.x,1-uv.y);return o;}
struct Backdrop {float4 color:SV_Target0;float depth:SV_Target1;};
[shader("fragment")]
Backdrop backdropFragment(Screen input){
    Frame f=frame();float3 ray=normalize(reconstruct(input.uv,0.001)-f.camera.xyz);Backdrop o;
    o.depth=0;o.color=float4(lerp(float3(.008,.013,.025),float3(.025,.04,.065),saturate(1-input.uv.y)),0);
    if(ray.y<-.0001){float distance=-f.camera.y/ray.y;if(distance>0){float3 world=f.camera.xyz+ray*distance;
        float grid=1-smoothstep(.01,.025,min(abs(frac(world.x+.5)-.5),abs(frac(world.z+.5)-.5)));
        float3 color=lerp(float3(.025,.035,.05),float3(.045,.06,.08),grid*.4);
        for(uint e=0;e<f.extent.w;e++){Config c=config(e);if(c.lightColor.w>0&&state(e,0)>0){float3 delta=c.lightPosition.xyz-world;float d2=max(dot(delta,delta),.05);float attenuation=saturate(1-sqrt(d2)/c.lightPosition.w);color+=c.lightColor.rgb*c.lightColor.w/(12.56637*d2)*attenuation*attenuation*.025;}}
        float4 clip=mul(f.vp,float4(world,1));o.depth=clip.z/clip.w;o.color=float4(color,0);}}
    return o;
}
struct ParticleVarying {float4 clip:SV_Position;float3 world:TEXCOORD0;float2 uv:TEXCOORD1;float4 color:TEXCOORD2;float3 normal:TEXCOORD3;float age:TEXCOORD4;nointerpolation uint emitter:TEXCOORD5;};
[shader("vertex")]
ParticleVarying particleVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){
    uint e=root.emitter;Config c=config(e);Frame f=frame();uint local=list(c.lists.x+instance*4);Particle p=loadParticle(c.partition.x+local);
    static const float2 corners[6]={float2(-1,-1),float2(1,-1),float2(-1,1),float2(-1,1),float2(1,-1),float2(1,1)};
    ParticleVarying o;float2 corner=corners[vertex%6];float3 world;float3 normal=-f.forward.xyz;
    if(c.render.x==1){uint next=list(c.lists.x+(instance+1)*4);Particle q=loadParticle(c.partition.x+next);float3 tangent=normalize(q.position-p.position+float3(1e-8,0,0));float3 side=normalize(cross(tangent,f.camera.xyz-p.position));world=lerp(p.position,q.position,(corner.y+1)*.5)+side*corner.x*c.surface.z*.5;}
    else if(c.render.x==2){float3 forward=length(p.velocity)>.001?normalize(p.velocity):float3(0,0,1);float3 right=normalize(cross(abs(forward.y)>.99?float3(1,0,0):float3(0,1,0),forward));float3 up=cross(forward,right);float3 vert=meshVertex(e,vertex);world=p.position+p.size*(right*vert.x+up*vert.y+forward*vert.z);normal=normalize(right*vert.x+up*vert.y+forward*vert.z+float3(1e-8,0,0));}
    else world=p.position+(f.right.xyz*corner.x+f.up.xyz*corner.y)*p.size*.5;
    o.clip=mul(f.vp,float4(world,1));o.world=world;o.uv=(corner+1)*.5;o.color=p.color;o.normal=normal;o.age=p.age;o.emitter=e;return o;
}
[shader("fragment")]
float4 particleFragment(ParticleVarying input):SV_Target0{
    if(input.age<0)discard;
    Config c=config(input.emitter);Frame f=frame();float2 disc=input.uv*2-1;
    float coverage=c.render.x==2?1:pow(saturate(1-dot(disc,disc)),1.6);if(coverage<.001)discard;
    float2 uv=input.uv;uint frameIndex=uint(max(0,input.age*c.surface.y))%(c.atlas.x*c.atlas.y);
    uv=(uv+float2(frameIndex%c.atlas.x,frameIndex/c.atlas.x))/float2(c.atlas.xy);
    float fade=1;if(f.inputs.z!=0){float z=textures[NonUniformResourceIndex(f.inputs.y)].Load(int3(uint2(input.clip.xy),0)).r;if(z>input.clip.z)discard;if(c.surface.x>0&&z>0)fade=saturate((f.clock.z/z-f.clock.z/input.clip.z)/c.surface.x);}
    AraMaterialInput m;m.uv=uv;m.worldPosition=input.world;m.worldNormal=input.normal;m.worldTangent=f.right.xyz;m.worldBitangent=f.up.xyz;m.viewDirection=normalize(f.camera.xyz-input.world);
    AraSurface s=evaluateMaterial(input.emitter,m);if(s.opacityMask<.5||s.clip<.5)discard;
    float3 color;
    if(c.render.z==0){s.baseColor*=input.color.rgb;color=ara_engine_light(s,m,normalize(float3(.4,1,.3)),float3(1.5,1.3,1.1),float3(.12,.17,.24));
        for(uint e=0;e<f.extent.w;e++){Config light=config(e);if(light.lightColor.w>0&&state(e,0)>0){float3 d=light.lightPosition.xyz-input.world;float d2=max(dot(d,d),.05);float a=saturate(1-sqrt(d2)/light.lightPosition.w);color+=ara_engine_light(s,m,normalize(d),light.lightColor.rgb*(light.lightColor.w*a*a/(12.56637*d2)),float3(0));}}}
    else color=s.color*input.color.rgb;
    float alpha=saturate(input.color.a*s.opacity*coverage*fade);return float4(color*alpha,alpha);
}
struct Display {float4 color:SV_Target0;float id:SV_Target1;};
[shader("fragment")]
Display tonemapFragment(Screen input){
    float4 hdr=textures[NonUniformResourceIndex(root.sortK)].Load(int3(uint2(input.uv*frame().extent.xy),0));float3 c=hdr.rgb*frame().clock.y;
    c=(c*(2.51*c+.03))/(c*(2.43*c+.59)+.14);c=saturate(c);c=pow(c,1./2.2);Display o;o.color=float4(c,1);o.id=hdr.a>0.00001?1:0;return o;
}
