// Ara TSR: zeroes upscaler state FSR clears with clear jobs (tsr/mod.rs TsrPass::Clear):
// on allocation the luma, accumulation and lock images, on every reset the
// accumulation, and the reconstructed previous depth region of the scratch buffer.
// Root words: 1 the storage image or buffer, 5 zero for an image (2 width, 3 height),
// one for a buffer (2 words, 3 groups per row, 6 first word; 64 words a group).
#include "../raster.slangh"
[shader("compute")]
[numthreads(8,8,1)]
void tsrClearMain(uint3 group:SV_GroupID,uint index:SV_GroupIndex,uint3 t:SV_DispatchThreadID){
    if(root.mode==0){
        if(all(t.xy<uint2(root.material,root.entity)))images[NonUniformResourceIndex(root.materials)][t.xy]=float4(0,0,0,0);
    }else{
        uint word=(group.y*root.entity+group.x)*64+index;
        if(word<root.material)buffers[NonUniformResourceIndex(root.materials)].Store((root.sample+word)*4,0);
    }
}
