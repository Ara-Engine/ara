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
// Height fog uses its own base altitude (look.z), tint and anisotropy (fog row), and on
// volumetric fog (weather.z) its emission and extinction scale (fogLight), ray-traced
// sun shadows and local lights by their volumetric intensity (parameters.w), as the
// raster renderer's fog; clear-air Rayleigh/Mie scattering joins it under the physical sky.
[shader("compute")][numthreads(8,8,1)]
void fogMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();if(any(tid.xy>=uint2(64,36)))return;float2 uv=(float2(tid.xy)+0.5)/float2(64,36);
    float3 d=cameraRay(f,uv*f.sizes.xy),integral=0,trans=1;float previous=0;
    uint rng=hash(tid.x+tid.y*64);bool physical=(f.display.w&2)!=0;
    float3 albedo=f.atmosphere.w*f.fog.rgb;float g=f.fog.w;
    float3 ambient=physical?float3(0):environmentAmbient(f);bool volumetric=f.weather.z>0.5;
    for(uint z=0;z<32;z++){
        float end=2*(exp2(log2(2001.)*float(z+1)/32)-1),ds=end-previous;
        float3 at=f.camera.xyz+d*(previous+ds*0.5);previous=end;
        float height=max(at.y-f.atmosphere.x,0);float profile=exp(-max(at.y-f.look.z,0)/f.atmosphere.z),density=f.atmosphere.y*profile;
        float3 sr=0;float sm=0;float3 e=density*f.fogLight.w;
        if(physical)e+=extinction(height,f.look.w,sr,sm);
        float3 sunlight=physical?solarIrradiance(f,at):f.sunLux.rgb;
        float visibility=any(sunlight>0) && (!volumetric||!occluded(f,at,f.sunDirection.xyz,10000,rng))?1:0;
        float cosine=dot(d,f.sunDirection.xyz);
        float3 source=sunlight*visibility*(sr*phaseR(cosine)+sm*phaseM(cosine,0.8)+density*albedo*phaseM(cosine,g));
        if(physical)source+=(sr+sm+density*albedo)*multiScattering(f,height,f.sunDirection.y)*f.sunLux.rgb;
        else source+=density*albedo*ambient;
        source+=f.fogLight.rgb*profile;
        uint4 head=buffers[f.scene.x].Load<uint4>(0);
        for(uint i=0;i<(volumetric?head.w:0);i++){
            // local-lights.slangh: samples of the light's area (four for rects, discs,
            // spheres and tubes; an ideal point's centre), so beams near area lights and
            // their soft shadows are exact; scaled by the light's volumetric intensity.
            LocalLight light=localLight(f,head.z+i*LIGHT_BYTES);if(light.params.w<=0)continue;
            uint samples=localArea(light)?4:1;float3 scattered=0;
            for(uint k=0;k<samples;k++){
                LocalSample s=sampleLocalLight(light,at,rng);
                if(s.distance>0&&any(s.value>0)&&!occluded(f,at,s.direction,s.distance,rng))scattered+=s.value*phaseM(dot(d,s.direction),g);
            }
            source+=density*albedo*scattered*(light.params.w/float(samples));
        }
        float3 segment=exp(-e*ds);integral+=trans*source*(1-segment)/max(e,float3(1e-9));trans*=segment;
        store(f,FROXELS,uint2(z*128+tid.x,tid.y),float4(integral,1));
        store(f,FROXELS,uint2(z*128+64+tid.x,tid.y),float4(trans,1));
    }
}
