#include "common.slangh"
// Environment-map ambient for fog without a physical sky: the mean radiance over
// fourteen directions (cube faces and corners) stands in for the isotropic in-scatter.
float3 environmentAmbient(Frame f){
    float3 sum=0;
    for(uint i=0;i<14;i++){
        float3 d=i<6?float3(i==0?1.:i==1?-1.:0.,i==2?1.:i==3?-1.:0.,i==4?1.:i==5?-1.:0.):normalize(float3((i&1)!=0?1.:-1.,(i&2)!=0?1.:-1.,(i&4)!=0?1.:-1.));
        sum+=textures[NonUniformResourceIndex(f.scene.z)].SampleLevel(samplers[1],envUv(d),0).rgb;
    }return sum/14;
}
// One thread integrates a frustum column front-to-back. Logarithmic slices
// retain near-field shafts; deterministic quadrature adds no temporal noise.
// Height fog uses its own base altitude (look.z), tint and anisotropy (fog row);
// clear-air Rayleigh/Mie scattering only joins it under the physical sky.
[shader("compute")][numthreads(8,8,1)]
void fogMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();if(any(tid.xy>=uint2(64,36)))return;float2 uv=(float2(tid.xy)+0.5)/float2(64,36);
    float3 d=cameraRay(f,uv*f.sizes.xy),integral=0,trans=1;float previous=0;
    uint rng=hash(tid.x+tid.y*64);bool physical=(f.display.w&2)!=0;
    float3 albedo=f.atmosphere.w*f.fog.rgb;float g=f.fog.w;
    float3 ambient=physical?float3(0):environmentAmbient(f);
    for(uint z=0;z<32;z++){
        float end=2*(exp2(log2(2001.)*float(z+1)/32)-1),ds=end-previous;
        float3 at=f.camera.xyz+d*(previous+ds*0.5);previous=end;
        float height=max(at.y-f.atmosphere.x,0);float density=f.atmosphere.y*exp(-max(at.y-f.look.z,0)/f.atmosphere.z);
        float3 sr=0;float sm=0;float3 e=density;
        if(physical)e+=extinction(height,f.look.w,sr,sm);
        float3 sunlight=physical?solarIrradiance(f,at):f.sunLux.rgb;
        float visibility=any(sunlight>0) && !occluded(f,at,f.sunDirection.xyz,10000,rng)?1:0;
        float cosine=dot(d,f.sunDirection.xyz);
        float3 source=sunlight*visibility*(sr*phaseR(cosine)+sm*phaseM(cosine,0.8)+density*albedo*phaseM(cosine,g));
        if(physical)source+=(sr+sm+density*albedo)*multiScattering(f,height,f.sunDirection.y)*f.sunLux.rgb;
        else source+=density*albedo*ambient;
        uint4 head=buffers[f.scene.x].Load<uint4>(0);
        for(uint i=0;i<head.w;i++){
            float4 p=buffers[f.scene.x].Load<float4>(head.z+i*64),c=buffers[f.scene.x].Load<float4>(head.z+i*64+16);
            float3 delta=p.xyz-at;float distance=length(delta);if(distance>p.w)continue;
            float4 cone=buffers[f.scene.x].Load<float4>(head.z+i*64+32),params=buffers[f.scene.x].Load<float4>(head.z+i*64+48);
            float3 l=delta/max(distance,0.001);float spot=params.x<0.5?1:smoothstep(cone.w,max(cone.w+1e-4,params.y),dot(-l,normalize(cone.xyz)));float fade=pow(saturate(1-pow(distance/p.w,4)),2);
            if(!occluded(f,at,l,max(0.001,distance-0.02),rng))source+=density*albedo*c.rgb*c.w/(4*PI*max(distance*distance,0.01))*fade*spot*phaseM(dot(d,l),g);
        }
        float3 segment=exp(-e*ds);integral+=trans*source*(1-segment)/max(e,float3(1e-9));trans*=segment;
        store(f,FROXELS,uint2(z*128+tid.x,tid.y),float4(integral,1));
        store(f,FROXELS,uint2(z*128+64+tid.x,tid.y),float4(trans,1));
    }
}
