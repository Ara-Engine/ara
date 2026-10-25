// Integer BC6H UF16; reference and numeric bounds: ../src/bc6h_reference.rs.
// One 64-lane group per block. Lanes 0..31 search partitions with the box seed;
// lanes 32..63 add the PCA seed in HQ. No wave intrinsics or floating arithmetic.
struct Root { uint input; uint output; uint blockCount; uint preset; uint a; uint b; uint c; uint d; };
#ifdef VULKAN
#define BIND(n) [[vk::binding(n,0)]]
[[vk::push_constant]] ConstantBuffer<Root> root;
#else
#define BIND(n)
ConstantBuffer<Root> root : register(b0,space0);
#endif
BIND(1) RWByteAddressBuffer buffers[64] : register(u0,space2);

#include "bc6h_tables.slangh"

groupshared uint bc6Half[16][3];
groupshared uint bc6Light[16][3];
groupshared uint bc6Shift;
groupshared uint64_t bc6Rank[32];
groupshared uint64_t bc6Errors[64];
groupshared uint4 bc6Words[64];

struct Bc6Endpoints { uint v[12]; };
struct Bc6Indices { uint v[16]; };
struct Bc6Candidate { uint64_t error; uint4 words; };

static const uint BC6_WEIGHT3[8] = {0,9,18,27,37,46,55,64};
static const uint BC6_WEIGHT4[16] = {0,4,9,13,17,21,26,30,34,38,43,47,51,55,60,64};

Bc6Candidate bc6Worst() {
    Bc6Candidate result;
    result.error = ~uint64_t(0); result.words = uint4(~0u);
    return result;
}

bool bc6Less(Bc6Candidate a, Bc6Candidate b) {
    if (a.error != b.error) return a.error < b.error;
    for (uint i=0;i<4;++i) { if (a.words[i] != b.words[i]) return a.words[i] < b.words[i]; }
    return false;
}

void bc6Keep(inout Bc6Candidate best, Bc6Candidate candidate) {
    if (bc6Less(candidate,best)) best = candidate;
}

uint bc6Linear(uint half, uint shift) {
    uint exponent=half>>10, mantissa=half&1023;
    uint left=exponent>0 ? exponent-1 : 0;
    uint64_t value=uint64_t(mantissa+(exponent>0 ? 1024u : 0u))<<left;
    return uint(value>>shift);
}

uint bc6Unquantize(uint q, uint bits) {
    if (bits==16) return q;
    if (q==0) return 0;
    if (q==((1u<<bits)-1)) return 65535;
    return ((q<<16)+32768)>>bits;
}

uint bc6Quantize(uint half, uint bits) {
    uint target=(half*64+15)/31, maximum=(1u<<bits)-1;
    uint center=uint(min(((uint64_t(target)<<bits)+32768)>>16,uint64_t(maximum)));
    uint bestError=~0u, best=0;
    uint first=center>2 ? center-2 : 0;
    for (uint q=first;q<=min(center+2,maximum);++q) {
        uint decoded=bc6Unquantize(q,bits)*31>>6;
        uint error=uint(abs(int(decoded)-int(half)));
        if (error<bestError) { bestError=error; best=q; }
    }
    return best;
}

uint bc6Subset(uint partition, uint pixel) { return (BC6_MASKS[partition]>>pixel)&1; }
uint bc6Weight(uint mode, uint index) { return mode<10 ? BC6_WEIGHT3[index] : BC6_WEIGHT4[index]; }

uint64_t bc6Distance(uint pixel, uint3 half, uint3 light) {
    uint64_t error=0;
    for (uint c=0;c<3;++c) {
        uint64_t a=uint64_t(abs(int(bc6Half[pixel][c])-int(half[c])));
        uint64_t b=uint64_t(abs(int(bc6Light[pixel][c])-int(light[c])));
        error+=a*a*4+b*b;
    }
    return error;
}

#include "bc6h_fit.slangh"
#include "bc6h_encode.slangh"

[shader("compute")]
[numthreads(64,1,1)]
void encodeBc6h(uint3 group : SV_GroupID, uint lane : SV_GroupIndex) {
    if (group.x>=root.blockCount) return;
    if (lane<16) {
        uint rg=buffers[NonUniformResourceIndex(root.input)].Load(group.x*128+lane*8);
        uint b=buffers[NonUniformResourceIndex(root.input)].Load(group.x*128+lane*8+4);
        bc6Half[lane][0]=rg&65535; bc6Half[lane][1]=rg>>16; bc6Half[lane][2]=b&65535;
    }
    GroupMemoryBarrierWithGroupSync();
    if (lane==0) {
        uint maximum=0;
        for (uint i=0;i<16;++i) for (uint c=0;c<3;++c) maximum=max(maximum,bc6Half[i][c]);
        uint exponent=maximum>>10;
        bc6Shift=exponent>5 ? exponent-5 : 0;
    }
    GroupMemoryBarrierWithGroupSync();
    if (lane<16) for (uint c=0;c<3;++c) bc6Light[lane][c]=bc6Linear(bc6Half[lane][c],bc6Shift);
    GroupMemoryBarrierWithGroupSync();

    uint partition=lane&31, kind=lane>>5;
    uint rounds=root.preset==0 ? 1 : root.preset==1 ? 3 : 5;
    Bc6Candidate best=bc6Worst();
    Bc6Endpoints initial=(Bc6Endpoints)0;
    if (root.preset!=0 && (kind==0 || root.preset>=2)) initial=bc6Seed(partition,true,kind);
    if (root.preset==1 && kind==0) bc6Rank[partition]=bc6Search(initial,9,partition,1).error;
    GroupMemoryBarrierWithGroupSync();
    bool selected=root.preset>=2;
    if (root.preset==1 && kind==0) {
        uint rank=0;
        for (uint p=0;p<32;++p) {
            if (bc6Rank[p]<bc6Rank[partition] || (bc6Rank[p]==bc6Rank[partition] && p<partition)) ++rank;
        }
        selected=rank<8;
    }
    if (selected) for (uint mode=0;mode<10;++mode) {
        if (root.preset==1 && mode!=0 && mode!=1 && mode!=5 && mode!=9) continue;
        bc6Keep(best,bc6Search(initial,mode,partition,rounds));
    }
    // Four independent lanes cover the single-subset modes with each HQ seed.
    if (partition<4 && (kind==0 || root.preset>=2) && (root.preset!=0 || lane==0)) {
        Bc6Endpoints one=bc6Seed(0,false,kind);
        bc6Keep(best,bc6Search(one,10+partition,0,rounds));
    }
    bc6Errors[lane]=best.error; bc6Words[lane]=best.words;
    GroupMemoryBarrierWithGroupSync();
    // Fixed binary tree, with a total order over (error, words[0..3]).
    for (uint stride=32;stride>0;stride>>=1) {
        if (lane<stride) {
            Bc6Candidate a,b;
            a.error=bc6Errors[lane]; a.words=bc6Words[lane];
            b.error=bc6Errors[lane+stride]; b.words=bc6Words[lane+stride];
            if (bc6Less(b,a)) { bc6Errors[lane]=b.error; bc6Words[lane]=b.words; }
        }
        GroupMemoryBarrierWithGroupSync();
    }
    if (lane==0) buffers[NonUniformResourceIndex(root.output)].Store4(group.x*16,bc6Words[0]);
}

#include "bc6h_split.slangh"
