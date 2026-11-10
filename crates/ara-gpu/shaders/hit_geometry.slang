// Retained position fetch, shared unchanged by Slang and the native MSL adapter.
#ifndef ARA_HIT_GEOMETRY
#define ARA_HIT_GEOMETRY
#ifndef ARA_HIT_CONTEXT
#define ARA_HIT_CONTEXT
#endif
#ifndef ARA_HIT_LOAD
#define ARA_HIT_LOAD(slot, offset) buffers[NonUniformResourceIndex(slot)].Load(offset)
#endif
#ifndef ARA_HIT_FLOAT3
#define ARA_HIT_FLOAT3(a, b, c) asfloat(uint3(a, b, c))
#endif
struct AraHitTriangle { float3 a; float3 b; float3 c; uint valid; };
// `instance` is the query's instance ordinal, `primitive` its local triangle index.
// Table offsets and all mesh offsets are bytes. Query status must be a triangle hit.
AraHitTriangle araHitTriangle(ARA_HIT_CONTEXT uint table, uint tableOffset,
                             uint instanceCount, uint instance, uint primitive) {
    AraHitTriangle result;
    result.a = float3(0); result.b = float3(0); result.c = float3(0); result.valid = 0;
    if (instance >= instanceCount) return result;
    uint base = tableOffset + instance * 32;
    uint vb = ARA_HIT_LOAD(table, base);
    uint ib = ARA_HIT_LOAD(table, base + 4);
    uint vo = ARA_HIT_LOAD(table, base + 8);
    uint io = ARA_HIT_LOAD(table, base + 12);
    uint stride = ARA_HIT_LOAD(table, base + 16);
    uint vertices = ARA_HIT_LOAD(table, base + 20);
    uint indices = ARA_HIT_LOAD(table, base + 24);
    if (primitive >= indices / 3) return result;
    uint at = io + primitive * 12;
    uint a = ARA_HIT_LOAD(ib, at), b = ARA_HIT_LOAD(ib, at + 4), c = ARA_HIT_LOAD(ib, at + 8);
    if (a >= vertices || b >= vertices || c >= vertices) return result;
    a = vo + a * stride; b = vo + b * stride; c = vo + c * stride;
    result.a = ARA_HIT_FLOAT3(ARA_HIT_LOAD(vb,a), ARA_HIT_LOAD(vb,a+4), ARA_HIT_LOAD(vb,a+8));
    result.b = ARA_HIT_FLOAT3(ARA_HIT_LOAD(vb,b), ARA_HIT_LOAD(vb,b+4), ARA_HIT_LOAD(vb,b+8));
    result.c = ARA_HIT_FLOAT3(ARA_HIT_LOAD(vb,c), ARA_HIT_LOAD(vb,c+4), ARA_HIT_LOAD(vb,c+8));
    result.valid = 1;
    return result;
}
#endif
