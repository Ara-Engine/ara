#include "common.slangh"
// One thread integrates a frustum column front-to-back. Logarithmic slices
// retain near-field shafts; deterministic quadrature adds no temporal noise.
[shader("compute")][numthreads(8,8,1)]
void fogMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();if(any(tid.xy>=uint2(64,36)))return;float2 uv=(float2(tid.xy)+0.5)/float2(64,36);
    float3 d=cameraRay(f,uv*f.sizes.xy),integral=0,trans=1;float previous=0;
    uint rng=hash(tid.x+tid.y*64);
    for(uint z=0;z<32;z++){
        float end=2*(exp2(log2(2001.)*float(z+1)/32)-1),ds=end-previous;
        float3 at=f.camera.xyz+d*(previous+ds*0.5);previous=end;
        float height=max(at.y-f.atmosphere.x,0);float density=f.atmosphere.y*exp(-height/f.atmosphere.z);
        float3 sr;float sm;float3 e=extinction(height,sr,sm)+density;
        float3 sunlight=solarIrradiance(f,at);
        float visibility=occluded(f,at,f.sunDirection.xyz,10000,rng)?0:1;
        float3 source=sunlight*visibility*(sr*phaseR(dot(d,f.sunDirection.xyz))+sm*phaseM(dot(d,f.sunDirection.xyz),0.8)+density*f.atmosphere.w*phaseM(dot(d,f.sunDirection.xyz),0.3));
        float3 sky=multiScattering(f,height,f.sunDirection.y)*f.sunLux.rgb;
        source+=(sr+sm+density*f.atmosphere.w)*sky;
        uint4 head=buffers[NonUniformResourceIndex(f.scene.x)].Load<uint4>(0);
        for(uint i=0;i<head.w;i++){
            float4 p=buffers[NonUniformResourceIndex(f.scene.x)].Load<float4>(head.z+i*64),c=buffers[NonUniformResourceIndex(f.scene.x)].Load<float4>(head.z+i*64+16);
            float3 delta=p.xyz-at;float distance=length(delta);if(distance>p.w)continue;
            float4 cone=buffers[NonUniformResourceIndex(f.scene.x)].Load<float4>(head.z+i*64+32),params=buffers[NonUniformResourceIndex(f.scene.x)].Load<float4>(head.z+i*64+48);
            float3 l=delta/max(distance,0.001);float spot=params.x<0.5?1:smoothstep(cone.w,max(cone.w+1e-4,params.y),dot(-l,normalize(cone.xyz)));float fade=pow(saturate(1-pow(distance/p.w,4)),2);
            if(!occluded(f,at,l,max(0.001,distance-0.02),rng))source+=density*f.atmosphere.w*c.rgb*c.w/(4*PI*max(distance*distance,0.01))*fade*spot*phaseM(dot(d,l),0.3);
        }
        float3 segment=exp(-e*ds);integral+=trans*source*(1-segment)/max(e,float3(1e-9));trans*=segment;
        store(f,FROXELS,uint2(z*128+tid.x,tid.y),float4(integral,1));
        store(f,FROXELS,uint2(z*128+64+tid.x,tid.y),float4(trans,1));
    }
}
