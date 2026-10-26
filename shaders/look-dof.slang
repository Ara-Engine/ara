// Depth of field from post-process volumes (look/passes.rs `dof_passes`), at render
// resolution before the temporal resolve, which then smooths it with the rest of the
// frame. A thin-lens circle of confusion (look.dof), a half-resolution gather over a
// fixed golden-angle disc (no noise) with separate background and foreground layers,
// and a composite into the image the resolve reads. Root word 1 (`root.materials`) is
// the scene HDR's sampled index; root word 2 (`root.material`) the pass.
#include "raster.slangh"
#include "look.slangh"

// Signed circle-of-confusion radius in render pixels at pixel `p`: negative in front of
// the focus distance, scaled by the near/far strengths and clamped to the preset's
// largest blur. Pixels without a surface (the sky) are at infinity.
float dofRadius(Frame f,Look l,int2 p){
    float depth=gbufferDepth(f,p),s=1e6;
    if(depth>0)s=max(dot(worldAtDepth(f,float2(p)+0.5,depth)-f.camera.xyz,f.forward.xyz),1e-3);
    float r=l.dof.y*(1-l.dof.x/s);r*=r<0?l.dof.w:l.dofBlur.x;
    return clamp(r,-l.dof.z,l.dof.z);
}
// Pass 0: half-resolution colour (rgb) and signed radius in half-resolution pixels (a):
// the nearest of the four pixels' radii when any is in front of the focus, otherwise
// the largest, so foreground edges keep their spread.
void dofPrepare(Frame f,Look l,uint2 p){
    int2 base=int2(p)*2;float3 c=0;float nearest=0,farthest=0;
    for(uint k=0;k<4;k++){
        int2 q=min(base+int2(k&1,k>>1),int2(f.extent.xy)-1);
        c+=min(pixel(root.materials,q).rgb,float3(60000));float r=dofRadius(f,l,q);nearest=min(nearest,r);farthest=max(farthest,r);
    }
    images[NonUniformResourceIndex(l.dofImages.z)][p]=float4(c/4,(nearest<0?nearest:farthest)*0.5);
}
// Pass 1: the gather. Each sample of the disc (as wide as the largest blur) lands in
// the background layer by the smaller of its own and this pixel's far radius (sharp
// foreground never takes on background blur) and in the foreground layer by its near
// radius (blurred foreground spreads over whatever lies behind it).
static const float GOLDEN_ANGLE=2.39996323;
void dofGather(Frame f,Look l,uint2 p,uint2 half){
    float2 texel=1/float2(half),uv=(float2(p)+0.5)*texel;
    float4 centre=historyTex(l.dofImages.x,uv),background=0,foreground=0;
    float largest=l.dof.z*0.5;uint n=max(uint(l.dofBlur.y),1u);
    for(uint i=0;i<n;i++){
        float r=sqrt((i+0.5)/n)*largest,a=i*GOLDEN_ANGLE;
        float4 s=historyTex(l.dofImages.x,uv+r*float2(cos(a),sin(a))*texel);
        float back=saturate(max(min(s.a,centre.a),0)-r+1),front=saturate(-s.a-r+1);
        background+=float4(s.rgb,1)*back;foreground+=float4(s.rgb,1)*front;
    }
    background.rgb/=max(background.a,1e-4);foreground.rgb/=max(foreground.a,1e-4);
    // Foreground opacity: covered once half the disc's samples land in front.
    float near=saturate(foreground.a*2/n),far=smoothstep(0.5,1.5,centre.a);
    images[NonUniformResourceIndex(l.dofImages.w)][p]=float4(lerp(background.rgb,foreground.rgb,near),max(near,far));
}
// Pass 2: tent-upsampled gather over the sharp scene by the gathered coverage and this
// pixel's own far radius, into the image the temporal resolve reads.
void dofComposite(Frame f,Look l,uint2 p){
    float2 uv=(float2(p)+0.5)/float2(f.extent.xy),texel=2/float2(f.extent.xy);float4 blurred=0;
    for(uint k=0;k<4;k++)blurred+=historyTex(l.dofImages.y,uv+(float2(k&1,k>>1)-0.5)*texel);
    blurred/=4;float far=smoothstep(1,2,dofRadius(f,l,int2(p))),blend=far+blurred.a-far*blurred.a;
    float3 sharp=pixel(root.materials,int2(p)).rgb;
    images[NonUniformResourceIndex(l.dofOutput.x)][p]=float4(min(lerp(sharp,blurred.rgb,blend),float3(60000)),1);
}
[shader("compute")][numthreads(8,8,1)]
void dofMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;uint pass=root.material;
    if(pass==2){if(all(tid.xy<f.extent.xy))dofComposite(f,l,tid.xy);return;}
    if(any(tid.xy>=half))return;
    if(pass==0)dofPrepare(f,l,tid.xy);else dofGather(f,l,tid.xy,half);
}
