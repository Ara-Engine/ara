// Ground-truth-based ambient occlusion (look/passes.rs): an independent
// implementation of Jimenez et al. 2016 (GTAO) at half resolution, in world space
// from the G-buffer depth, then a depth-aware denoise. Per-pixel slice rotation
// uses interleaved gradient noise advancing each frame; temporal antialiasing
// integrates it, so the result is stable without stochastic rays.
#include "raster.slangh"
#include "look.slangh"
// Reversed-Z depth at full-resolution pixel `p` to a world position.
float3 worldAt(Frame f,float2 p,float depth){
    float2 uv=p/float2(f.extent.xy);float4 h=mul(f.inverseVp,float4(uv.x*2-1,1-uv.y*2,depth,1));return h.xyz/h.w;
}
float ign(float2 p,uint frame){p+=5.588238*float(frame&63);return frac(52.9829189*frac(dot(p,float2(0.06711056,0.00583715))));}
// Root word 2: half-resolution output pixel count per row is f.extent.x/2 rounded up.
// Output: r ambient visibility, g distance to the camera (for the filters).
[shader("compute")][numthreads(8,8,1)]
void aoTraceMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    int2 p=int2(tid.xy)*2;float depth=pixel(f.extra.x,p).x;
    if(depth<=0){images[NonUniformResourceIndex(l.aoImages.z)][tid.xy]=float4(1,1e9,0,1);return;}
    float2 centre=float2(p)+0.5;float3 P=worldAt(f,centre,depth),N=normalize(pixel(f.gbuffer.y,p).xyz),V=normalize(f.camera.xyz-P);
    float distance=length(f.camera.xyz-P);
    // Screen radius of the world radius at this distance.
    float3 side=normalize(cross(f.forward.xyz,abs(f.forward.y)<0.99?float3(0,1,0):float3(1,0,0)));
    float4 a=mul(f.vp,float4(P,1)),b=mul(f.vp,float4(P+side*l.ao.x,1));
    float radius=length((b.xy/b.w-a.xy/a.w)*0.5*float2(f.extent.xy));
    float noise=ign(float2(tid.xy),f.extent.w),jitter=ign(float2(tid.xy)+float2(37,17),f.extent.w);
    uint slices=uint(l.ao.y),steps=uint(l.ao.z);float visibility=0;
    if(radius<1){images[NonUniformResourceIndex(l.aoImages.z)][tid.xy]=float4(1,distance,0,1);return;}
    for(uint s=0;s<slices;s++){
        float phi=(float(s)+noise)/float(slices)*PI;float2 direction=float2(cos(phi),sin(phi));
        // The slice plane contains V and the world direction of `direction` on screen.
        float3 along=worldAt(f,centre+direction*4,depth)-P;float3 sliceDirection=normalize(along-V*dot(along,V));
        float3 axis=normalize(cross(sliceDirection,V));float3 projected=N-axis*dot(N,axis);float length2=dot(projected,projected);
        if(length2<1e-6)continue;float projectedLength=sqrt(length2);projected/=projectedLength;
        float n=sign(dot(projected,sliceDirection))*acos(clamp(dot(projected,V),-1.,1.));
        float horizons[2]={-1,-1};
        for(uint sideIndex=0;sideIndex<2;sideIndex++){
            float sign=sideIndex==0?1.:-1.;
            for(uint k=0;k<steps;k++){
                float t=(float(k)+jitter)/float(steps);float offset=max(t*t*radius,1+float(k));
                float2 q=centre+direction*sign*offset;if(any(q<0)||any(q>=float2(f.extent.xy)))break;
                float sampleDepth=pixel(f.extra.x,int2(q)).x;if(sampleDepth<=0)continue;
                float3 delta=worldAt(f,floor(q)+0.5,sampleDepth)-P;float len=length(delta);
                float falloff=saturate(1-len*len/(l.ao.x*l.ao.x));float cosine=dot(delta/max(len,1e-5),V);
                horizons[sideIndex]=max(horizons[sideIndex],lerp(-1.,cosine,falloff));
            }
        }
        // The positive side runs along sliceDirection (angles grow towards it).
        float h0=n+max(-acos(horizons[1])-n,-PI/2),h1=n+min(acos(horizons[0])-n,PI/2);
        float arc=(-cos(2*h0-n)+cos(n)+2*h0*sin(n))+(-cos(2*h1-n)+cos(n)+2*h1*sin(n));
        visibility+=projectedLength*arc*0.25;
    }
    visibility=saturate(visibility/max(float(slices),1));
    images[NonUniformResourceIndex(l.aoImages.z)][tid.xy]=float4(visibility,distance,0,1);
}
// 4x4 depth-aware box around each half-resolution pixel (removes the slice noise
// pattern; TAA does the rest).
[shader("compute")][numthreads(8,8,1)]
void aoDenoiseMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();Look l=look(f);uint2 half=(f.extent.xy+1)/2;if(any(tid.xy>=half))return;
    float2 centre=pixel(l.aoImages.x,int2(tid.xy)).rg;float sum=0,total=0;
    for(int y=-2;y<=1;y++)for(int x=-2;x<=1;x++){
        int2 q=clamp(int2(tid.xy)+int2(x,y),int2(0),int2(half)-1);float2 v=pixel(l.aoImages.x,q).rg;
        float w=exp(-abs(v.g-centre.g)/max(centre.g*0.02,0.05));sum+=v.r*w;total+=w;
    }
    images[NonUniformResourceIndex(l.aoImages.w)][tid.xy]=float4(sum/max(total,1e-5),centre.g,0,1);
}
