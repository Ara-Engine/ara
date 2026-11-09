// FP32 coalesced patch gather. One lane owns one receiver; all directions are
// accumulated in ordinal order. Only generation-matching active tiles execute.
#include "raster.slangh"

[shader("compute")]
[numthreads(32,1,1)]
void transportGather(uint3 group:SV_GroupID,uint lane:SV_GroupThreadID) {
    uint record=root.frame;
    uint4 head=buffers[record].Load<uint4>(0); // patch count, source count, tile count, epoch
    uint4 offsets=buffers[record].Load<uint4>(16); // source, weights, transmission, groups
    uint4 rows=buffers[record].Load<uint4>(32); // tiles, active tile ids, albedo, forcing
    uint4 state=buffers[record].Load<uint4>(48); // block generations, active count, spare
    uint dispatch=group.x+group.y*65535;
    if(dispatch>=state.y)return;
    uint tile=buffers[record].Load(rows.y+dispatch*4);
    if(buffers[record].Load(state.x+tile*4)!=head.w)return;
    uint4 t=buffers[record].Load<uint4>(rows.x+tile*16);
    if(lane>=t.y)return;
    uint receiver=t.x+lane;float3 sum=0;
    for(uint ordinal=0;ordinal<t.w;++ordinal) {
        uint3 g=buffers[record].Load<uint3>(offsets.w+(t.z+ordinal)*12);
        if((g.y&(1u<<lane))==0)continue;
        uint link=g.x+countbits(g.y&((1u<<lane)-1));
        uint source=buffers[record].Load(offsets.x+link*4);
        float weight=asfloat(buffers[record].Load(offsets.y+link*4));
        float3 transmission=buffers[record].Load<float3>(offsets.z+link*12);
        if(source<head.x)sum+=buffers[root.materials].Load<float3>(source*12)*weight*transmission;
    }
    float3 albedo=buffers[record].Load<float3>(rows.z+receiver*12);
    float3 forcing=buffers[record].Load<float3>(rows.w+receiver*12);
    buffers[root.material].Store<float3>(receiver*12,forcing+albedo*sum);
}
