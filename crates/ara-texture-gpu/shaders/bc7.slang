// Deterministic BC7: one 64-lane group per block, one candidate per lane/stride.
// Keep arithmetic, enumeration and ties in sync with src/bc7_reference.rs.
// HQ: all 285 mode/partition/rotation/index-selection combinations. Balanced: 85.
// Fixed-order reductions compare (RGBA squared error, candidate ID).
#include "bc7_tables.slangh"
#include "bc7_quant.slangh"

struct Root { uint input; uint output; uint blockCount; uint preset; uint a; uint b; uint c; uint d; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(1) RWByteAddressBuffer buffers[64] : register(u0,space2);

// ceil(65536/n) gives exact floor(sum/n) for 0 <= sum <= 255*n, n <=16.
static const uint MEAN_RECIP[17] = {0,65536,32768,21846,16384,13108,10923,9363,8192,7282,6554,5958,5462,5042,4682,4370,4096};
static const uint SUBSETS[8] = {3,2,3,2,1,1,1,2};
static const uint PARTITION_BITS[8] = {4,6,6,6,0,0,0,6};
static const uint COLOR_BITS[8] = {4,6,5,7,5,7,7,5};
static const uint ALPHA_BITS[8] = {0,0,0,0,6,8,7,5};
static const uint INDEX_BITS[8] = {3,3,2,2,2,2,4,2};
static const uint PBITS[8] = {2,1,0,2,0,0,2,2};
static const int WEIGHTS[48] = {
    0,21,43,64,0,0,0,0,0,0,0,0,0,0,0,0,
    0,9,18,27,37,46,55,64,0,0,0,0,0,0,0,0,
    0,4,9,13,17,21,26,30,34,38,43,47,51,55,60,64
};

groupshared uint texels[16];
groupshared uint errors[64];
groupshared uint candidates[64];
groupshared uint4 blocks[64];
groupshared uint coarseErrors[285];
groupshared uint finalists[17];

// Nibble-packed selectors and vector endpoints avoid dynamically indexed private
// scalar arrays in the inner search. This representation does not change ties.
struct Fit {
    uint4 q0; uint4 q1;
    int4 ep0; int4 ep1;
    uint p0; uint p1;
    uint2 indices;
    uint error;
};
typedef vector<int64_t,4> Wide4;
// Explicit products avoid requiring Vulkan shaderIntegerDotProduct.
int dot4(int4 a, int4 b) { int4 p = a*b; return p.x+p.y+p.z+p.w; }
Fit emptyFit() { Fit f = (Fit)0; f.error = 0xffffffff; return f; }
uint getIndex(Fit f, uint i) { return ((i < 8 ? f.indices.x : f.indices.y) >> ((i & 7)*4)) & 15; }
void setIndex(inout Fit f, uint i, uint index) {
    uint shift = (i & 7)*4;
    uint mask = 15u << shift;
    if (i < 8) f.indices.x = (f.indices.x & ~mask) | (index << shift);
    else f.indices.y = (f.indices.y & ~mask) | (index << shift);
}
int4 activeChannels(uint first, uint channels) {
    return int4(first == 0, first <= 1 && first+channels > 1,
        first <= 2 && first+channels > 2, first+channels > 3);
}
int expandEndpoint(uint q, uint bits) {
    uint v = q << (8-bits);
    return int(v | (v >> bits));
}
void quantizeEndpoint(int value, uint bits, bool hasP, uint p, out uint qBest, out int decodedBest) {
    if (bits == 8) { qBest = uint(value); decodedBest = value; return; }
    uint table = hasP ? (bits-4)*2+p : 8+bits-5;
    uint packed = BC7_QUANT[table*64+(uint(value) >> 2)];
    qBest = (packed >> ((uint(value) & 3)*8)) & 255;
    uint hb = hasP ? 1 : 0;
    decodedBest = expandEndpoint((qBest << hb) | p,bits+hb);
}
void seedEndpoints(int4 pixels[16], uint mask, uint first, uint channels, bool bbox, out int4 loEndpoint, out int4 hiEndpoint, out bool uniform) {
    int4 lo = int4(255), hi = int4(0), sum = int4(0);
    int count = int(countbits(mask));
    int4 active = activeChannels(first,channels);
    for (uint i = 0; i < 16; ++i) if ((mask & (1u << i)) != 0) {
        lo = min(lo,pixels[i]); hi = max(hi,pixels[i]); sum += pixels[i];
    }
    uniform = all((hi-lo)*active == 0);
    if (bbox || uniform) { loEndpoint = lo; hiEndpoint = hi; return; }
    uint major = first;
    [unroll] for (uint c = 0; c < 4; ++c) if (active[c] != 0 && hi[c]-lo[c] > hi[major]-lo[major]) major = c;
    int4 axis = int4(0); axis[major] = 64;
    int4 cov0 = int4(0), cov1 = int4(0), cov2 = int4(0), cov3 = int4(0);
    int4 mean = int4((uint4(sum)*MEAN_RECIP[count]) >> 16);
    for (uint i = 0; i < 16; ++i) if ((mask & (1u << i)) != 0) {
        int4 d = (pixels[i]-mean)*active;
        cov0 += d.x*d; cov1 += d.y*d; cov2 += d.z*d; cov3 += d.w*d;
    }
    // |axis| <=127 and |cov| <=260116: four-term dots fit int32.
    // A power-of-two scale preserves direction without integer division.
    for (uint iteration = 0; iteration < 4; ++iteration) {
        int4 next = int4(dot4(cov0,axis),dot4(cov1,axis),dot4(cov2,axis),dot4(cov3,axis));
        int4 magnitude = abs(next);
        int scale = max(max(magnitude.x,magnitude.y),max(magnitude.z,magnitude.w));
        if (scale != 0) {
            uint shift = uint(max(firstbithigh(uint(scale))-6,0));
            int4 normalized = int4(uint4(magnitude) >> shift);
            axis = select(next < 0,-normalized,normalized);
        }
    }
    loEndpoint = int4(0); hiEndpoint = int4(0);
    int minimum = 0x7fffffff, maximum = -2147483647-1;
    for (uint i = 0; i < 16; ++i) if ((mask & (1u << i)) != 0) {
        int projection = dot4(pixels[i],axis);
        if (projection < minimum) { minimum = projection; loEndpoint = pixels[i]; }
        if (projection > maximum) { maximum = projection; hiEndpoint = pixels[i]; }
    }
}
void assignIndices(int4 pixels[16], uint mask, uint first, uint channels, uint indexBits, bool uniform, inout Fit f) {
    int4 palette[16];
    int4 active = activeChannels(first,channels);
    for (uint j = 0; j < (1u << indexBits); ++j) {
        int w = WEIGHTS[(indexBits-2)*16+j];
        palette[j] = ((64-w)*f.ep0+w*f.ep1+32)>>6;
    }
    if (uniform) {
        // All fitted channels match: evaluate their common selector once, then
        // scale the error. Later p-bit re-quantization rounds still run.
        int4 pixel = pixels[firstbitlow(mask)];
        uint error = 0xffffffff, index = 0;
        for (uint j = 0; j < (1u << indexBits); ++j) {
            int4 d = (pixel-palette[j])*active;
            uint e = uint(dot4(d,d));
            if (e < error) { error = e; index = j; }
        }
        f.error = error*countbits(mask);
        f.indices = uint2(index*0x11111111u);
        return;
    }
    f.error = 0;
    for (uint i = 0; i < 16; ++i) if ((mask & (1u << i)) != 0) {
        uint error = 0xffffffff, index = 0;
        for (uint j = 0; j < (1u << indexBits); ++j) {
            int4 d = (pixels[i]-palette[j])*active;
            uint e = uint(dot4(d,d));
            if (e < error) { error = e; index = j; }
        }
        setIndex(f,i,index); f.error += error;
    }
}
// Exact clamped floor(numerator/det), with eight fixed subtract/compare steps.
// The result is a byte, so general 64-bit integer division is unnecessary.
int4 divideEndpoint(Wide4 numerator, int det) {
    int4 quotient = int4(0);
    [unroll] for (int bit = 7; bit >= 0; --bit) {
        Wide4 threshold = Wide4(int64_t(det) << bit);
        bool4 take = numerator >= threshold;
        numerator -= select(take, threshold, Wide4(0));
        quotient |= int4(take)*(1 << bit);
    }
    return quotient;
}
void refitEndpoints(int4 pixels[16], uint mask, uint first, uint channels, uint indexBits, Fit f, out int4 loEndpoint, out int4 hiEndpoint) {
    // Sum of weights <=1024, square sums <=65536, RHS <=261120.
    // aa*bb <= (65536/2)^2, hence det also fits signed 32 bits.
    int aa = 0, ab = 0, bb = 0;
    int4 ax = int4(0), bx = int4(0);
    for (uint i = 0; i < 16; ++i) if ((mask & (1u << i)) != 0) {
        int b = WEIGHTS[(indexBits-2)*16+getIndex(f,i)], a = 64-b;
        aa += a*a; ab += a*b; bb += b*b;
        ax += a*pixels[i]; bx += b*pixels[i];
    }
    int det = aa*bb-ab*ab;
    loEndpoint = f.ep0; hiEndpoint = f.ep1;
    if (det > 0) {
        loEndpoint = divideEndpoint(64*(Wide4(ax)*bb-Wide4(bx)*ab)+det/2,det);
        hiEndpoint = divideEndpoint(64*(Wide4(bx)*aa-Wide4(ax)*ab)+det/2,det);
    }
}
Fit fitSubset(int4 pixels[16], uint mask, uint first, uint channels, uint bits, uint indexBits, uint pbits, uint preset) {
    uint rounds = preset == 3 ? 2 : (preset >= 2 ? 4 : 3), seeds = preset == 0 || preset == 3 ? 1 : 2;
    int4 active = activeChannels(first,channels);
    Fit best = emptyFit();
    for (uint s = 0; s < seeds; ++s) {
        int4 loEndpoint, hiEndpoint;
        bool uniform;
        seedEndpoints(pixels,mask,first,channels,s != 0,loEndpoint,hiEndpoint,uniform);
        for (uint iteration = 0; iteration < rounds; ++iteration) {
            Fit current = emptyFit();
            for (uint pattern = 0; pattern < (1u << pbits); ++pattern) {
                Fit trial = emptyFit();
                trial.p0 = pbits == 1 ? pattern : pattern & 1;
                trial.p1 = pbits == 1 ? pattern : (pattern >> 1) & 1;
                [unroll] for (uint c = 0; c < 4; ++c) if (active[c] != 0) {
                    quantizeEndpoint(loEndpoint[c],bits,pbits != 0,trial.p0,trial.q0[c],trial.ep0[c]);
                    quantizeEndpoint(hiEndpoint[c],bits,pbits != 0,trial.p1,trial.q1[c],trial.ep1[c]);
                }
                assignIndices(pixels,mask,first,channels,indexBits,uniform,trial);
                if (trial.error < current.error) current = trial;
                if (current.error == 0) return current;
            }
            if (current.error < best.error) best = current;
            int4 nextLo, nextHi;
            refitEndpoints(pixels,mask,first,channels,indexBits,current,nextLo,nextHi);
            bool converged = all((nextLo-loEndpoint)*active == 0) && all((nextHi-hiEndpoint)*active == 0);
            loEndpoint = nextLo; hiEndpoint = nextHi;
            if (converged) break;
        }
        // The box seed repeats the same active endpoints. Preserve all p-bit
        // refinement rounds: re-quantization can improve a singular fit.
        if (uniform) return best;
    }
    return best;
}

void putBits(inout uint4 words, inout uint cursor, uint value, uint count) {
    if (count == 0) return;
    uint word = cursor/32;
    uint shift = cursor%32;
    words[word] |= value << shift;
    if (shift+count > 32) words[word+1] |= value >> (32-shift);
    cursor += count;
}

uint subsetFor(uint mode, uint partition, uint texel) {
    if (SUBSETS[mode] == 2) return (PARTITION2[partition] >> (texel*2)) & 3;
    if (SUBSETS[mode] == 3) return (PARTITION3[partition] >> (texel*2)) & 3;
    return 0;
}

uint anchorFor(uint mode, uint partition, uint s) {
    if (s == 0) return 0;
    if (SUBSETS[mode] == 2) return ANCHOR2[partition];
    return s == 1 ? ANCHOR31[partition] : ANCHOR32[partition];
}

uint encodeCandidate(uint mode, uint partition, uint rotation, uint selection, uint preset, out uint4 words) {
    bool dual = mode == 4 || mode == 5;
    uint colorIndex = INDEX_BITS[mode]+selection;
    uint alphaIndex = mode == 4 ? 3-selection : 2;
    int4 pixels[16];
    for (uint i = 0; i < 16; ++i) {
        uint packed = texels[i];
        pixels[i] = int4(packed & 255,(packed >> 8) & 255,(packed >> 16) & 255,packed >> 24);
        if (rotation != 0) { int swap = pixels[i].w; pixels[i].w = pixels[i][rotation-1]; pixels[i][rotation-1] = swap; }
    }
    Fit fits[3];
    Fit scalar = emptyFit();
    uint error = 0;
    for (uint s = 0; s < SUBSETS[mode]; ++s) {
        uint mask = 0;
        for (uint i = 0; i < 16; ++i) if (subsetFor(mode, partition, i) == s) mask |= 1u << i;
        fits[s] = fitSubset(pixels, mask, 0, ALPHA_BITS[mode] != 0 && !dual ? 4 : 3, COLOR_BITS[mode], colorIndex, PBITS[mode], preset);
        error += fits[s].error;
        if ((getIndex(fits[s],anchorFor(mode, partition, s)) & (1u << (colorIndex-1))) != 0) {
            uint4 qSwap = fits[s].q0; fits[s].q0 = fits[s].q1; fits[s].q1 = qSwap;
            uint swap = fits[s].p0; fits[s].p0 = fits[s].p1; fits[s].p1 = swap;
            fits[s].indices ^= uint2(0x11111111u*((1u << colorIndex)-1));
        }
    }
    if (dual) {
        scalar = fitSubset(pixels, 65535, 3, 1, ALPHA_BITS[mode], alphaIndex, 0, preset);
        error += scalar.error;
        if ((getIndex(scalar,0) & (1u << (alphaIndex-1))) != 0) {
            uint4 qSwap = scalar.q0; scalar.q0 = scalar.q1; scalar.q1 = qSwap;
            scalar.indices ^= uint2(0x11111111u*((1u << alphaIndex)-1));
        }
        fits[0].q0.w = scalar.q0.w; fits[0].q1.w = scalar.q1.w;
    } else if (ALPHA_BITS[mode] == 0) {
        for (uint i = 0; i < 16; ++i) { int d = 255-pixels[i].w; error += uint(d*d); }
    }
    words = uint4(0);
    uint cursor = 0;
    putBits(words, cursor, 1u << mode, mode+1);
    putBits(words, cursor, partition, PARTITION_BITS[mode]);
    if (dual) putBits(words, cursor, rotation, 2);
    if (mode == 4) putBits(words, cursor, selection, 1);
    for (uint c = 0; c < 4; ++c) {
        uint precision = c == 3 ? ALPHA_BITS[mode] : COLOR_BITS[mode];
        if (precision != 0) {
            for (uint s = 0; s < SUBSETS[mode]; ++s) for (uint e = 0; e < 2; ++e) putBits(words, cursor, (e == 0 ? fits[s].q0[c] : fits[s].q1[c]), precision);
        }
    }
    for (uint s = 0; s < SUBSETS[mode]; ++s) {
        if (PBITS[mode] != 0) { putBits(words, cursor, fits[s].p0, 1); if (PBITS[mode] == 2) putBits(words, cursor, fits[s].p1, 1); }
    }
    for (uint stream = 0; stream < (dual ? 2u : 1u); ++stream) {
        bool useScalar = dual && stream == 1-selection;
        uint precision = useScalar ? alphaIndex : colorIndex;
        for (uint i = 0; i < 16; ++i) {
            uint s = subsetFor(mode, partition, i);
            uint index = useScalar ? getIndex(scalar,i) : getIndex(fits[s],i);
            putBits(words, cursor, index, precision - (i == anchorFor(mode, partition, s) ? 1u : 0u));
        }
    }
    return error;
}

uint4 candidateParameters(uint id, uint preset) {
    if (id == 0) return uint4(6,0,0,0);
    id -= 1;
    if (id < 8) return uint4(4,0,id/2,id&1);
    id -= 8;
    if (id < 4) return uint4(5,0,id,0);
    id -= 4;
    for (uint m = 0; m < 8; ++m) {
        if (m == 4 || m == 5 || m == 6) continue;
        uint count = m == 0 ? (preset == 1 ? 8 : 16) : (preset == 1 ? 16 : 64);
        if (id < count) return uint4(m,id,0,0);
        id -= count;
    }
    return uint4(6,0,0,0); // Unreachable for bounded candidate IDs.
}

// All modes/partitions get two alternating fit passes. Refine the best eight
// plus mode 6; HQ also includes Balanced's eight, preserving preset quality order.
// Coarse ranking is lexicographic (error, ID); a lane owns each finalist slot.
bool balancedCandidate(uint id) {
    // HQ IDs: dual-plane/mode6 0..12, mode0 13..28, then four 64-entry modes.
    return id < 21 || (id >= 29 && ((id-29) & 63) < 16);
}

[shader("compute")]
[numthreads(64,1,1)]
void encodeBc7(uint3 group : SV_GroupID, uint lane : SV_GroupIndex) {
    if (group.x >= root.blockCount) return;
    if (lane < 16) texels[lane] = buffers[NonUniformResourceIndex(root.input)].Load(group.x*64+lane*4);
    GroupMemoryBarrierWithGroupSync();
    uint preset = min(root.preset,2u);
    uint count = preset == 0 ? 1 : (preset == 1 ? 85 : 285);
    uint finalistCount = preset == 0 ? 1 : (preset == 1 ? 9 : 17);
    if (preset != 0) {
        for (uint id = lane; id < count; id += 64) {
            uint4 p = candidateParameters(id,preset);
            uint4 ignored;
            coarseErrors[id] = encodeCandidate(p.x,p.y,p.z,p.w,3,ignored);
        }
        GroupMemoryBarrierWithGroupSync();
        for (uint id = lane; id < count; id += 64) {
            uint rank = 0;
            uint balancedRank = preset == 2 && balancedCandidate(id) ? 0 : 8;
            for (uint other = 0; other < count; ++other) {
                if (coarseErrors[other] < coarseErrors[id] || (coarseErrors[other] == coarseErrors[id] && other < id)) {
                    rank += 1;
                    if (preset == 2 && balancedCandidate(other)) balancedRank += 1;
                }
                if (rank >= 8 && balancedRank >= 8) break;
            }
            if (rank < 8) finalists[rank] = id;
            if (preset == 2 && balancedRank < 8) finalists[8+balancedRank] = id;
        }
        if (lane == 0) finalists[finalistCount-1] = 0;
        GroupMemoryBarrierWithGroupSync();
    } else if (lane == 0) finalists[0] = 0;
    uint bestError = 0xffffffff, bestId = 0xffffffff;
    uint4 bestWords = uint4(0);
    if (lane < finalistCount) {
        uint id = finalists[lane];
        uint4 p = candidateParameters(id,preset);
        bestError = encodeCandidate(p.x,p.y,p.z,p.w,preset,bestWords);
        bestId = id;
    }
    errors[lane] = bestError; candidates[lane] = bestId; blocks[lane] = bestWords;
    GroupMemoryBarrierWithGroupSync();
    for (uint stride = 32; stride > 0; stride >>= 1) {
        if (lane < stride) {
            uint other = lane+stride;
            if (errors[other] < errors[lane] || (errors[other] == errors[lane] && candidates[other] < candidates[lane])) {
                errors[lane] = errors[other]; candidates[lane] = candidates[other]; blocks[lane] = blocks[other];
            }
        }
        GroupMemoryBarrierWithGroupSync();
    }
    if (lane == 0) buffers[NonUniformResourceIndex(root.output)].Store4(group.x*16,blocks[0]);
}

#include "bc7_split.slangh"
#include "bc7_dot.slangh"
