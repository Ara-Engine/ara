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
// raster renderer's fog. Under the physical sky the atmosphere's aerial perspective
// joins it (atmosphereAerial, both atmosphere lights, light 0's single scattering
// shadowed), and the height fog's sun and sky light scale by the height fog
// contribution past the volumetric distance (weather.w; everywhere without volumetric
// fog), as the raster coarse grid behind its near volume.
[shader("compute")][numthreads(8,8,1)]
void fogMain(uint3 tid:SV_DispatchThreadID){
    Frame f=frame();if(any(tid.xy>=uint2(64,36)))return;float2 uv=(float2(tid.xy)+0.5)/float2(64,36);
    float3 d=cameraRay(f,uv*f.sizes.xy),integral=0,trans=1;float previous=0;
    uint rng=hash(tid.x+tid.y*64);bool physical=(f.display.w&2)!=0;
    float3 albedo=f.atmosphere.w*f.fog.rgb;float g=f.fog.w;
    float3 ambient=physical?float3(0):environmentAmbient(f);bool volumetric=f.weather.z>0.5;
    AtmosphereModel a=atmosphereModel();uint transmittance=ri(f,TRANSMITTANCE),ms=ri(f,MULTISCATTER);
    for(uint z=0;z<FROXEL_SLICES;z++){
        float start=previous,end=froxelEnd(z),ds=end-previous;
        float3 at=f.camera.xyz+d*(previous+ds*0.5);previous=end;
        float profile=exp(-max(at.y-f.look.z,0)/f.atmosphere.z),density=f.atmosphere.y*profile;
        float3 e=density*f.fogLight.w;
        float3 sunlight=physical?solarIrradiance(f,at):f.sunLux.rgb;
        // Glass passes sun shafts on, tinted (shadowTransmittance).
        float3 visibility=!any(sunlight>0)?float3(0):volumetric?shadowTransmittance(f,at,f.sunDirection.xyz,10000,rng):float3(1);
        float cosine=dot(d,f.sunDirection.xyz);
        float fogLight=physical && (!volumetric || start>=f.weather.w)?a.skyFactor.w:1;
        float3 source=sunlight*visibility*density*albedo*phaseM(cosine,g)*fogLight;
        if(physical){
            AtmosphereSegment air=atmosphereAerial(a,transmittance,ms,f.camera.xyz,d,start,end,at);
            // The height fog's ambient: the multiple scattering a fog particle sees.
            float3 p=atmospherePosition(a,f.camera.xyz,at);float r=length(p);
            source+=air.sun*visibility*cloudShadow(f,at)+air.source+density*albedo*fogLight*(a.light0Lux.rgb*atmosphereMultiScatter(a,ms,r,dot(p/r,a.light0Direction.xyz))+a.light1Lux.rgb*atmosphereMultiScatter(a,ms,r,dot(p/r,a.light1Direction.xyz)));
            e+=air.extinction;
        }else source+=density*albedo*ambient;
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
                if(s.distance>0&&any(s.value>0))scattered+=s.value*phaseM(dot(d,s.direction),g)*shadowTransmittance(f,at,s.direction,s.distance,rng);
            }
            source+=density*albedo*scattered*(light.params.w/float(samples));
        }
        float3 segment=exp(-e*ds);integral+=trans*source*(1-segment)/max(e,float3(1e-9));trans*=segment;
        store(f,FROXELS,uint2(z*128+tid.x,tid.y),float4(integral,1));
        store(f,FROXELS,uint2(z*128+64+tid.x,tid.y),float4(trans,1));
    }
}
