// Ara-owned ABI. All counts are element counts; all positions are Y-up metres.
#pragma once
#include <cstdint>

struct AnConfig {
    float min[3], max[3];
    float cell_size, cell_height, radius, height, climb, slope, error;
    int tile_size, tile_x, tile_z;
};
struct AnLink {
    float start[3], end[3], radius;
    unsigned int id;
    unsigned char area, bidirectional;
};
// area: 0..15 relabels walkable spans; 255 carves them out.
struct AnBox { float min[3], max[3]; unsigned char area; };
// blocked: ascending off-mesh link user IDs that queries must not traverse.
struct AnFilter {
    float costs[16]; unsigned short include, exclude;
    const unsigned int* blocked; int blocked_count;
};
struct AnCorner { float position[3]; unsigned int polygon; unsigned char flags; };
struct AnAgent {
    float position[3], velocity[3], desired[3];
    float radius, height, speed, acceleration;
    unsigned short include, exclude;
};
struct AnDebug { float a[3], b[3], c[3]; unsigned char area; };
struct AnWall { float a[3], b[3]; };
// side: Detour portal direction (0 = +X, 2 = +Z, 4 = -X, 6 = -Z).
struct AnBorder {
    float a[3], b[3];
    int tile_x, tile_z;
    unsigned char side, linked, area;
};

extern "C" {
int an_bake(const AnConfig*, const float*, int, const int*, const unsigned char*, int,
            const AnLink*, int, const AnBox*, int, void**);
const unsigned char* an_tile_bytes(const void*, int*);
void an_tile_free(void*);
void* an_mesh_new(const float*, float, int);
void an_mesh_free(void*);
int an_mesh_add(void*, const unsigned char*, int);
void* an_query_new(const void*, int);
void an_query_free(void*);
unsigned int an_nearest(void*, const float*, const float*, const AnFilter*, float*);
unsigned int an_path(void*, const float*, const float*, const float*, const AnFilter*,
                     AnCorner*, int, int*, unsigned char*, float*);
unsigned int an_raycast(void*, const float*, const float*, const float*, const AnFilter*, float*, float*);
int an_visible(void*, const float*, const float*, const float*, const AnFilter*);
int an_polygon_area(void*, unsigned int);
int an_debug_count(const void*);
int an_debug(const void*, AnDebug*, int);
int an_crowd_step(const void*, AnAgent*, int, float);
int an_walls(const void*, AnWall*, int);
int an_borders(const void*, AnBorder*, int);
}
