struct Screen {float4 clip:SV_Position;float2 uv:TEXCOORD0;};
[shader("vertex")]
Screen screenVertex(uint id:SV_VertexID){Screen o;float2 uv=float2((id<<1)&2,id&2);o.clip=float4(uv*2-1,0,1);o.uv=float2(uv.x,1-uv.y);return o;}
struct Backdrop {float4 color:SV_Target0;float depth:SV_Target1;};
[shader("fragment")]
Backdrop backdropFragment(Screen input){
    Frame f=frame();Backdrop o;o.depth=0;
    if((f.visuals.z&2u)!=0){o.color=float4(textures[NonUniformResourceIndex(f.visuals.y)].Load(int3(uint2(input.clip.xy),0)).rgb,0);return o;}
    float3 ray=normalize(reconstruct(input.uv,0.001)-f.camera.xyz);
    o.color=float4(lerp(float3(.008,.013,.025),float3(.025,.04,.065),saturate(1-input.uv.y)),0);
    if(ray.y<-.0001){float distance=-f.camera.y/ray.y;if(distance>0){float3 world=f.camera.xyz+ray*distance;
        float grid=1-smoothstep(.01,.025,min(abs(frac(world.x+.5)-.5),abs(frac(world.z+.5)-.5)));
        float3 color=lerp(float3(.025,.035,.05),float3(.045,.06,.08),grid*.4);
        float4 clip=mul(f.vp,float4(world,1));o.depth=clip.z/clip.w;o.color=float4(color,0);}}
    return o;
}
// Fidelity's directional octahedral probe packing is 102 float4s per probe.
float3 particleAmbient(float3 world,float3 normal){
    Frame f=frame();if(f.probeCounts.w==0)return f.ambient.rgb;
    float3 grid=(world-f.probeOrigin.xyz)/f.probeSpacing.xyz;
    int3 base=clamp(int3(floor(grid)),int3(0),int3(f.probeCounts.xyz)-2);float3 alpha=saturate(grid-base),sum=0;float total=0;
    normal/=abs(normal.x)+abs(normal.y)+abs(normal.z);float2 oct=normal.xy;if(normal.z<0)oct=(1-abs(oct.yx))*float2(oct.x>=0?1:-1,oct.y>=0?1:-1);
    uint2 texel=min(uint2((oct*.5+.5)*6),uint2(5));
    for(uint k=0;k<8;k++){uint3 d=uint3(k&1,(k>>1)&1,(k>>2)&1),p=uint3(base)+d;uint id=(p.z*f.probeCounts.y+p.y)*f.probeCounts.x+p.x;
        float4 meta=buffers[NonUniformResourceIndex(f.probeCounts.w-1)].Load<float4>((id*102+100)*16);if(meta.w<.5)continue;
        float3 w=lerp(1-alpha,alpha,float3(d));float weight=w.x*w.y*w.z;
        sum+=buffers[NonUniformResourceIndex(f.probeCounts.w-1)].Load<float4>((id*102+texel.y*6+texel.x)*16).rgb*weight;total+=weight;
    }return total>1e-5?sum/(total*3.14159265):f.ambient.rgb;
}
struct ParticleVarying {float4 clip:SV_Position;float3 world:TEXCOORD0;float2 uv:TEXCOORD1;float4 color:TEXCOORD2;float3 normal:TEXCOORD3;float age:TEXCOORD4;nointerpolation uint emitter:TEXCOORD5;float life:TEXCOORD6;};
[shader("vertex")]
ParticleVarying particleVertex(uint vertex:SV_VertexID,uint instance:SV_InstanceID){
    uint e=root.emitter;Config c=config(e);Frame f=frame();uint local=list(c.lists.x+instance*4);Particle p=loadParticle(c.partition.x+local);
    static const float2 corners[6]={float2(-1,-1),float2(1,-1),float2(-1,1),float2(-1,1),float2(1,-1),float2(1,1)};
    ParticleVarying o;float2 corner=corners[vertex%6];float3 world;float3 normal=-f.forward.xyz;
    if(c.render.x==1){uint next=list(c.lists.x+(instance+1)*4);Particle q=loadParticle(c.partition.x+next);float3 tangent=normalize(q.position-p.position+float3(1e-8,0,0));float3 side=normalize(cross(tangent,f.camera.xyz-p.position));world=lerp(p.position,q.position,(corner.y+1)*.5)+side*corner.x*c.surface.z*.5;}
    else if(c.render.x==2){float3 forward=length(p.velocity)>.001?normalize(p.velocity):float3(0,0,1);float3 right=normalize(cross(abs(forward.y)>.99?float3(1,0,0):float3(0,1,0),forward));float3 up=cross(forward,right);float3 vert=meshVertex(e,vertex);world=p.position+p.size*(right*vert.x+up*vert.y+forward*vert.z);normal=normalize(right*vert.x+up*vert.y+forward*vert.z+float3(1e-8,0,0));}
    else {
        float3 right=f.right.xyz,up=f.up.xyz;float width=p.size,height=p.size;float3 centre=p.position;
        if(c.appearance.z==1){float3 projected=p.velocity-f.forward.xyz*dot(p.velocity,f.forward.xyz);up=length(projected)>.0001?normalize(projected):f.up.xyz;right=normalize(cross(up,-f.forward.xyz));height=max(p.size*c.motion.x,p.size+length(projected)*f.clock.x*c.motion.y);centre-=p.velocity*f.clock.x*c.motion.y*.5;}
        if(c.appearance.z==2){right=float3(1,0,0);up=float3(0,0,1);normal=float3(0,1,0);centre.y=max(centre.y,.012);}
        if(c.appearance.x==1){height*=2.;centre.y+=p.size*.25;}
        if(c.appearance.x==2){float angle=float(hash(p.seed)>>8)/16777216.*6.2831853;float3 r=right;right=r*cos(angle)+up*sin(angle);up=up*cos(angle)-r*sin(angle);}
        world=centre+(right*corner.x*width+up*corner.y*height)*.5;
    }
    o.clip=mul(f.vp,float4(world,1));o.world=world;o.uv=(corner+1)*.5;o.color=p.color;o.normal=normal;o.age=p.age;o.life=p.life;o.emitter=e;return o;
}
float4 sheet(uint kind,float2 uv,float frameIndex){
    uint first=min(uint(frameIndex),15u),next=min(first+1,15u);float2 a=float2(first%4,first/4+(kind-1)*4),b=float2(next%4,next/4+(kind-1)*4);
    float2 cell=(clamp(uv,0,1)*123.+2.5)/128.;
    float4 x=textures[NonUniformResourceIndex(frame().visuals.x)].SampleLevel(samplers[2],(cell+a)/float2(4,20),0);
    float4 y=textures[NonUniformResourceIndex(frame().visuals.x)].SampleLevel(samplers[2],(cell+b)/float2(4,20),0);
    return lerp(x,y,frac(frameIndex));
}
// Henyey-Greenstein phase (g = 0.35) for light propagating along a direction whose
// cosine with the outgoing (toward-camera) direction is `cosine`.
float hgPhase(float cosine){float g=.35;return (1-g*g)/(12.56637*pow(max(1+g*g-2*g*cosine,1e-3),1.5));}
// The scene HDR's pre-exposure (ara_render::HdrExposure): the exposure state's
// applied multiplier (visuals.w, index + 1), else the fallback in sunColor.w; 1 when
// the attachment holds linear radiance.
float sceneExposure(Frame f){
    if(f.visuals.w==0)return 1;
    float x=buffers[NonUniformResourceIndex(f.visuals.w-1)].Load<float4>(0).x,fallback=f.sunColor.w;
    return x>0&&x<1e30?x:fallback>0&&fallback<1e30?fallback:1;
}
struct ParticleOutput {float4 color:SV_Target0;float id:SV_Target1;};
[shader("fragment")]
ParticleOutput particleFragment(ParticleVarying input){
    if(input.age<0)discard;
    Config c=config(input.emitter);Frame f=frame();float2 disc=input.uv*2-1;uint kind=c.render.x==0?c.appearance.x:0;
    float4 texel=float4(1,.5,.5,1);float coverage=c.render.x==2?1:pow(saturate(1-dot(disc,disc)),1.6);
    if(kind>0){texel=sheet(kind,input.uv,saturate(input.age/input.life)*15.);coverage=texel.a;}
    if(coverage<.003)discard;
    float2 uv=input.uv;uint frameIndex=uint(max(0,input.age*c.surface.y))%(c.atlas.x*c.atlas.y);
    uv=(uv+float2(frameIndex%c.atlas.x,frameIndex/c.atlas.x))/float2(c.atlas.xy);
    float fade=1;if(f.inputs.z!=0){float z=sceneDepth(uint2(input.clip.xy));if(z>input.clip.z)discard;if(c.surface.x>0&&z>0)fade=saturate((f.clock.z/z-f.clock.z/input.clip.z)/c.surface.x);}
    AraMaterialInput m;m.uv=uv;m.worldPosition=input.world;m.worldNormal=input.normal;m.worldTangent=f.right.xyz;m.worldBitangent=f.up.xyz;m.viewDirection=normalize(f.camera.xyz-input.world);
    if(kind==2)m.worldNormal=normalize(input.normal+f.right.xyz*(texel.g*2-1)*1.8+f.up.xyz*(texel.b*2-1)*1.8);
    AraSurface s=evaluateMaterial(input.emitter,m);if(s.opacityMask<.5||s.clip<.5)discard;
    float3 color;
    if(c.render.z==0 && c.render.x==0 && (kind==0 || kind==2)){
        // Lit soft and smoke sprites are thin participating media, not surfaces
        // (spark/streak sheets such as rain keep surface lighting): single scattering
        // with a forward-biased Henyey-Greenstein phase (bright when backlit, dim
        // when front-lit) plus the probe field's mean radiance. A Lambert surface
        // facing the sun returned four times the light of an isotropic medium.
        float3 albedo=s.baseColor*input.color.rgb;float3 toCamera=m.viewDirection;
        float phase=hgPhase(dot(-f.sunDirection.xyz,toCamera));
        // Local Beer-Lambert density attenuation approximates sprite self shadowing.
        float shadow=kind==2?exp(-texel.r*1.4)*(.65+.35*saturate(dot(m.worldNormal,f.sunDirection.xyz))):1;
        color=albedo*(f.sunColor.rgb*phase*shadow+particleAmbient(input.world,m.worldNormal));
        for(uint e=0;e<f.extent.w;e++){Config light=config(e);if(light.lightColor.w>0&&state(e,0)>0){float3 d=light.lightPosition.xyz-input.world;float d2=max(dot(d,d),.05);float a=saturate(1-sqrt(d2)/light.lightPosition.w);float flicker=.85+.1*sin(float(root.tick)*.73)+.05*sin(float(root.tick)*1.37);color+=albedo*light.lightColor.rgb*(light.lightColor.w*a*a*flicker/(12.56637*d2))*hgPhase(dot(-normalize(d),toCamera));}}
    }
    else if(c.render.z==0){s.baseColor*=input.color.rgb;float3 ambient=particleAmbient(input.world,m.worldNormal);color=ara_engine_light(s,m,f.sunDirection.xyz,f.sunColor.rgb,ambient);
        for(uint e=0;e<f.extent.w;e++){Config light=config(e);if(light.lightColor.w>0&&state(e,0)>0){float3 d=light.lightPosition.xyz-input.world;float d2=max(dot(d,d),.05);float a=saturate(1-sqrt(d2)/light.lightPosition.w);float flicker=.85+.1*sin(float(root.tick)*.73)+.05*sin(float(root.tick)*1.37);color+=ara_engine_light(s,m,normalize(d),light.lightColor.rgb*(light.lightColor.w*a*a*flicker/(12.56637*d2)),float3(0));}}
    }
    else color=s.color*input.color.rgb;
    if(kind==1){float heat=pow(texel.r,.6);color*=lerp(float3(.7,.08,.005),float3(1.6,1.1,.35),heat);}
    float alpha=saturate(input.color.a*s.opacity*coverage*fade);if(alpha<.001)discard;
    ParticleOutput o;o.color=float4(min(color*alpha*f.ambient.w*sceneExposure(f),60000),c.appearance.y==1?0:alpha);o.id=1;return o;
}
[shader("fragment")]
float4 tonemapFragment(Screen input):SV_Target0{
    Frame f=frame();
    float3 c=textures[NonUniformResourceIndex(root.sortK)].Load(int3(uint2(input.uv*f.extent.xy),0)).rgb/sceneExposure(f)*f.clock.y;
    c=(c*(2.51*c+.03))/(c*(2.43*c+.59)+.14);return float4(pow(saturate(c),1./2.2),1);
}