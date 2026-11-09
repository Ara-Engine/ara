// The existing deferred path, selecting the software result for every specular
// pixel. Its alpha is one: screen reflections never enter this integral.
#include "baseline.slang"
[shader("fragment")]
float4 reflectLightingFragment(FullscreenVarying v):SV_Target0{return lighting(v,true,true);}
