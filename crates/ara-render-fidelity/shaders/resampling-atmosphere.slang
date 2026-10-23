#define SKY_ATMOSPHERE 1
#define MATERIAL_LAYERS 1
#define reservoirInit reservoirInitAtmosphere
#define reservoirSpatial reservoirSpatialAtmosphere
#define specularTrace specularTraceAtmosphere
#define reservoirTrace reservoirTraceAtmosphere
#include "resampling.slang"
