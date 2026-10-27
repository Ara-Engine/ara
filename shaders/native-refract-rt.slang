// Hybrid refraction (D29: High/Epic on devices with ray queries; renderer/rt.rs
// creates `native_forward_refract_rt` only there, so raster PSOs carry none of it).
// The raster composition of look-refract.slangh, with rays where it estimates: one
// ray against blended and transmissive instances finds a volume's far side (its own
// mesh; open meshes keep the slab estimate), and one against the opaque scene finds
// what lies behind. That hit reads the copied lit scene where the copy shows it (with
// the rough lobe's blur), else shades from the lighting caches (rtShadeHit), or the
// sky (rtMiss). Deterministic: at most two interfaces and two rays per pixel.
#include "native-raster.slang"
#include "look-rt.slangh"
// Radiance in the HDR target's space arriving along (o, d); `travelled`: metres
// already crossed inside the medium (widens the footprint by `spread`).
float3 refractRay(Frame f,Look l,float3 o,float3 d,float spread,float travelled,float roughness){
    RtHit hit;float bias=l.rtParams[1].x+l.rtParams[1].y*length(o-f.camera.xyz);
    if(!rtTrace(f,l,o+d*bias,d,l.rtParams[0].y,hit,RT_OPAQUE|RT_MASKED,max(spread,0.002)))return rtMiss(f,l,d,roughness)*refractExposure(f,l);
    float3 at=o+d*(bias+hit.distance);float depth=refractViewDepth(f,at);float2 px=refractPixel(f,at);
    float copied=refractDepthAt(l,int2(px));
    if(copied>0 && abs(copied-depth)<max(0.05,0.02*depth))
        return refractSample(f,l,px/float2(f.extent.xy),refractLevelFor(f,spread*(travelled+hit.distance),depth));
    float3 position,geometric;Surface h=rtSurface(l,hit,d,max(spread,0.002)*hit.distance,position,geometric);
    return rtShadeHit(f,l,h,position,geometric,-d)*refractExposure(f,l);
}
// refractColour with traced interfaces (see above); always the High/Epic tier's shading.
float4 refractColourRt(Frame f,Look l,Surface s,uint2 volume,float scale,float3 world,float3 geometric,int2 p){
    RefractVolume m=refractVolume(volume);float3 v=normalize(f.camera.xyz-world);
    float f0=refractF0(m.ior),through;float3 reflected=refractSurface(f,l,s,f0,world,p,true,through);
    float spread=refractSpread(s.roughness,m.ior);float3 transmitted;
    if(m.thickness<=0)transmitted=refractRay(f,l,world,-v,spread,0,s.roughness)*s.albedo;
    else{
        float thickness=m.thickness*scale;float3 t1=refract(-v,s.normal,1/m.ior);if(dot(t1,t1)<1e-6)t1=-v;
        float chord=thickness/max(-dot(t1,geometric),0.05);float3 n2=geometric;RtHit back;
        float bias=l.rtParams[1].x+l.rtParams[1].y*length(world-f.camera.xyz);
        if(rtTrace(f,l,world+t1*bias,t1,thickness*4+bias,back,RT_TRANSPARENT)){
            RtTriangle t=rtTriangle(l,back.instance,back.primitive,back.barycentrics);
            float3 nn=t.a.normal*t.w.x+t.b.normal*t.w.y+t.c.normal*t.w.z;
            chord=bias+back.distance;n2=dot(nn,nn)>1e-12?normalize(nn):t1;if(dot(n2,t1)<0)n2=-n2;
        }
        float3 exit=world+t1*chord,t2=refract(t1,-n2,m.ior);float leave;
        if(dot(t2,t2)<1e-6){t2=t1;leave=1-f0;}else leave=refractLeave(f0,t2,n2);
        transmitted=refractRay(f,l,exit,t2,spread,chord,s.roughness)*exp(-m.absorption*chord)*s.albedo*s.albedo*leave;
    }
    return float4(refractFinite((reflected+s.transmission*through*transmitted)*s.alpha),s.alpha);
}
[shader("fragment")]
ForwardLit nativeForwardRefractRtFragment(Varying v){
    NativeMaterial m=material(v.material);uint2 volume=uint2(asuint(m.factors.z),asuint(m.factors.w));
    bool front=facesViewer(v);if(!front && (volume.x&0xffff)!=0)discard;
    Surface s=forwardSurface<0>(v);float3 geometric=safeNormalize(v.normal,float3(0,1,0))*(front?1:-1);
    float4x4 world=buffers[NonUniformResourceIndex(root.material)].Load<float4x4>(128+(v.id-1)*192+16);
    float scale=pow(abs(determinant((float3x3)world)),1./3.);
    Frame f=frame();Look l=look(f);ForwardLit o;o.color=refractColourRt(f,l,s,volume,scale,v.world,geometric,int2(v.clip.xy));
    o.id=float(v.id);o.velocity=float4(motion(v),0,1);return o;
}
