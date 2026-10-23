#define RASTER 1
#include "common.slangh"
// Explicit capture only: portable readback supports RGBA32F, not the half-float
// production history. No diagnostic allocation or conversion runs in a frame.
[shader("compute")][numthreads(8,8,1)]
void diagnosticMain(uint3 tid:SV_DispatchThreadID){Frame f=frame();if(any(tid.xy>=f.sizes.xy))return;images[NonUniformResourceIndex(root.pad0)][tid.xy]=textures[NonUniformResourceIndex(root.arg)].Load(int3(tid.xy,0));}
