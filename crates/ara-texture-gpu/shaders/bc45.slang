// Deterministic BC4/BC5 UNORM. See ../src/bc45_reference.rs for the CPU oracle.
// Original encoder; no third-party implementation is incorporated.
// 64 threads/block: 32 candidates for each endpoint order, fixed tree reductions.
// Fast: 2 seeds/order, 1 fit. Balanced: 8 seeds/order, 3 fits, 3x3 refinement.
// HQ retains Balanced, searches all 32 seeds/order with 6 fits, then two 7x7
// refinements. This is a fitted multi-start search, not exhaustive 256x256 search.
// Rational interpolation is exact in byte*35 units; all arithmetic is 32-bit.

struct Root { uint input; uint output; uint blockCount; uint preset; uint a; uint b; uint c; uint d; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(1) RWByteAddressBuffer buffers[64] : register(u0,space2);

struct Candidate { uint error; uint first; uint second; };
groupshared uint samples[32];
groupshared Candidate candidates[64];

Candidate emptyCandidate() {
    Candidate result;
    result.error = 0xffffffff;
    result.first = 255;
    result.second = 255;
    return result;
}

bool better(Candidate a, Candidate b) {
    return a.error < b.error || (a.error == b.error &&
        (a.first < b.first || (a.first == b.first && a.second < b.second)));
}

uint2 ordered(int first, int second, bool six) {
    uint low = uint(clamp(min(first, second), 0, 255));
    uint high = uint(clamp(max(first, second), 0, 255));
    if (six) return uint2(low, high);
    if (high == low) return high == 255 ? uint2(255, 254) : uint2(high + 1, low);
    return uint2(high, low);
}

void palette(uint first, uint second, out int decoded[8]) {
    int a = int(first), b = int(second);
    decoded[0] = a * 35;
    decoded[1] = b * 35;
    if (a > b) {
        [unroll] for (int i = 2; i < 8; ++i)
            decoded[i] = ((8 - i) * a + (i - 1) * b) * 5;
    } else {
        [unroll] for (int i = 2; i < 6; ++i)
            decoded[i] = ((6 - i) * a + (i - 1) * b) * 7;
        decoded[6] = 0;
        decoded[7] = 255 * 35;
    }
}

uint2 nearest(uint value, int decoded[8]) {
    uint error = 0xffffffff, selected = 0;
    [unroll] for (uint i = 0; i < 8; ++i) {
        int delta = int(value) * 35 - decoded[i];
        uint trial = uint(delta * delta);
        if (trial < error) { error = trial; selected = i; }
    }
    return uint2(selected, error);
}

Candidate score(uint channel, uint2 endpoints) {
    int decoded[8];
    palette(endpoints.x, endpoints.y, decoded);
    Candidate result;
    result.error = 0;
    result.first = endpoints.x;
    result.second = endpoints.y;
    for (uint i = 0; i < 16; ++i)
        result.error += nearest(samples[channel * 16 + i], decoded).y;
    return result;
}

Candidate fitted(uint channel, uint2 endpoints, out uint2 next) {
    bool six = endpoints.x <= endpoints.y;
    int denominator = six ? 5 : 7;
    int decoded[8];
    palette(endpoints.x, endpoints.y, decoded);
    int aa = 0, ab = 0, bb = 0, av = 0, bv = 0;
    Candidate result;
    result.error = 0;
    result.first = endpoints.x;
    result.second = endpoints.y;
    for (uint i = 0; i < 16; ++i) {
        uint value = samples[channel * 16 + i];
        uint2 selected = nearest(value, decoded);
        result.error += selected.y;
        if (six && selected.x >= 6) continue;
        int weightB = selected.x == 0 ? 0 : (selected.x == 1 ? denominator : int(selected.x) - 1);
        int weightA = denominator - weightB;
        aa += weightA * weightA;
        ab += weightA * weightB;
        bb += weightB * weightB;
        av += weightA * denominator * int(value);
        bv += weightB * denominator * int(value);
    }
    next = endpoints;
    int determinant = aa * bb - ab * ab;
    if (determinant > 0) {
        int a = max(av * bb - bv * ab, 0);
        int b = max(bv * aa - av * ab, 0);
        next = ordered((a + determinant / 2) / determinant,
                       (b + determinant / 2) / determinant, six);
    }
    return result;
}

uint2 seed(uint channel, uint seedIndex, bool six) {
    uint low = 255, high = 0, innerLow = 255, innerHigh = 0;
    for (uint i = 0; i < 16; ++i) {
        uint v = samples[channel * 16 + i];
        low = min(low, v); high = max(high, v);
        if (v > 0 && v < 255) { innerLow = min(innerLow, v); innerHigh = max(innerHigh, v); }
    }
    if (seedIndex == 1 && innerLow <= innerHigh)
        return ordered(int(innerLow), int(innerHigh), six);
    uint divisor = six ? 10 : 14;
    int step = int(max((high - low + divisor / 2) / divisor, 1));
    int x = 0, y = 0;
    if (seedIndex == 2) { x = 1; }
    else if (seedIndex == 3) { y = -1; }
    else if (seedIndex == 4) { x = 1; y = -1; }
    else if (seedIndex == 5) { x = -1; y = 1; }
    else if (seedIndex == 6) { x = -1; }
    else if (seedIndex == 7) { y = 1; }
    else if (seedIndex >= 8) {
        uint cell = seedIndex - 8;
        if (cell >= 12) ++cell;
        x = int(cell % 5) - 2;
        y = int(cell / 5) - 2;
    }
    return ordered(int(low) + x * step, int(high) + y * step, six);
}

Candidate search(uint channel, uint index, uint count, uint iterations, bool six) {
    Candidate best = emptyCandidate();
    if (index >= count) return best;
    uint2 endpoints = seed(channel, index, six);
    for (uint iteration = 0; iteration < iterations; ++iteration) {
        uint2 next;
        Candidate trial = fitted(channel, endpoints, next);
        if (better(trial, best)) best = trial;
        endpoints = next;
    }
    Candidate trial = score(channel, endpoints);
    return better(trial, best) ? trial : best;
}

// Every lane participates, including inactive search lanes. No wave operations.
Candidate reduce(Candidate value, uint lane) {
    candidates[lane] = value;
    GroupMemoryBarrierWithGroupSync();
    uint local = lane & 31;
    for (uint stride = 16; stride > 0; stride >>= 1) {
        if (local < stride && better(candidates[lane + stride], candidates[lane]))
            candidates[lane] = candidates[lane + stride];
        GroupMemoryBarrierWithGroupSync();
    }
    Candidate result = candidates[lane & 32];
    // Readers finish before a subsequent reduction overwrites shared candidates.
    GroupMemoryBarrierWithGroupSync();
    return result;
}

Candidate neighborhood(uint channel, Candidate center, int radius, uint local, bool six) {
    Candidate best = center;
    uint side = uint(radius * 2 + 1);
    for (uint cell = local; cell < side * side; cell += 32) {
        int a = int(center.first) + int(cell / side) - radius;
        int b = int(center.second) + int(cell % side) - radius;
        if (a < 0 || a > 255 || b < 0 || b > 255 || ((a <= b) != six)) continue;
        Candidate trial = score(channel, uint2(uint(a), uint(b)));
        if (better(trial, best)) best = trial;
    }
    return best;
}

uint2 encodeChannel(uint channel, uint lane) {
    bool six = lane >= 32;
    uint local = lane & 31;
    bool balanced = root.preset >= 1;
    Candidate best = search(channel, local, balanced ? 8 : 2, balanced ? 3 : 1, six);
    best = reduce(best, lane);
    if (balanced) best = reduce(neighborhood(channel, best, 1, local, six), lane);
    if (root.preset >= 2) {
        Candidate thorough = search(channel, local, 32, 6, six);
        if (better(thorough, best)) best = thorough;
        best = reduce(best, lane);
        for (uint iteration = 0; iteration < 2; ++iteration)
            best = reduce(neighborhood(channel, best, 3, local, six), lane);
    }
    // One final fixed pairwise comparison joins the two endpoint orders.
    Candidate eight = candidates[0], sixEntry = candidates[32];
    best = better(sixEntry, eight) ? sixEntry : eight;
    GroupMemoryBarrierWithGroupSync();
    uint2 packed = uint2(0, 0);
    if (lane == 0) {
        int decoded[8];
        palette(best.first, best.second, decoded);
        packed.x = best.first | (best.second << 8);
        for (uint i = 0; i < 16; ++i) {
            uint selected = nearest(samples[channel * 16 + i], decoded).x;
            uint bit = 16 + i * 3;
            if (bit < 32) {
                packed.x |= selected << bit;
                if (bit > 29) packed.y |= selected >> (32 - bit);
            } else packed.y |= selected << (bit - 32);
        }
    }
    return packed;
}

void loadBlock(uint block, uint lane) {
    if (lane < 16) {
        uint word = buffers[NonUniformResourceIndex(root.input)].Load((block * 16 + lane) * 4);
        samples[lane] = word & 255;
        samples[16 + lane] = (word >> 8) & 255;
    }
    GroupMemoryBarrierWithGroupSync();
}

[shader("compute")][numthreads(64,1,1)]
void encodeBc4(uint3 group : SV_GroupID, uint lane : SV_GroupIndex) {
    if (group.x >= root.blockCount) return;
    loadBlock(group.x, lane);
    uint2 red = encodeChannel(0, lane);
    if (lane == 0) buffers[NonUniformResourceIndex(root.output)].Store2(group.x * 8, red);
}

[shader("compute")][numthreads(64,1,1)]
void encodeBc5(uint3 group : SV_GroupID, uint lane : SV_GroupIndex) {
    if (group.x >= root.blockCount) return;
    loadBlock(group.x, lane);
    uint2 red = encodeChannel(0, lane);
    uint2 green = encodeChannel(1, lane);
    if (lane == 0) buffers[NonUniformResourceIndex(root.output)].Store4(group.x * 16, uint4(red, green));
}
