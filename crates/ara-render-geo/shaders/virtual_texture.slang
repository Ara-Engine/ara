// Virtual texture Root32/bindless v1 adapter. Physical pages are RGBA8 buffers;
// decode sRGB per texel before bilinear interpolation. Mip-major page tables.
uint vtPage(uint4 desc,uint mip,float2 uv,out float2 local) {
    uint2 size=max(uint2(1),desc.xy>>mip);float2 pixel=frac(uv)*float2(size);
    uint2 tile=min(uint2(pixel)/128,(size-1)/128);local=pixel-float2(tile*128)+.5;
    uint base=desc.z;for(uint m=0;m<mip;m++)base+=((max(1,desc.x>>m)+127)/128)*((max(1,desc.y>>m)+127)/128);
    return base+tile.y*((size.x+127)/128)+tile.x;
}
float3 vtDecode(float3 c) {return select(c<=.04045,c/12.92,pow((c+.055)/1.055,float3(2.4)));}
float4 vtTexel(uint pool,uint slot,uint2 p,bool srgb) {
    uint v=buffers[NonUniformResourceIndex(pool)].Load((slot*130*130+p.y*130+p.x)*4);
    float4 c=float4(v&255,(v>>8)&255,(v>>16)&255,v>>24)/255.;
    if(srgb)c.rgb=vtDecode(c.rgb);return c;
}
float4 vtBilinear(uint pool,uint slot,float2 p,bool srgb) {
    uint2 lo=uint2(floor(p));float2 t=frac(p);
    return lerp(lerp(vtTexel(pool,slot,lo,srgb),vtTexel(pool,slot,lo+uint2(1,0),srgb),t.x),
        lerp(vtTexel(pool,slot,lo+uint2(0,1),srgb),vtTexel(pool,slot,lo+1,srgb),t.x),t.y);
}
void vtRequest(uint feedback,uint page,uint epoch) {
    bool shared=WaveActiveAllEqual(page);uint ignored;
    if(!shared || WaveIsFirstLane())buffers[NonUniformResourceIndex(feedback)].InterlockedMax((128+page)*4,epoch,ignored);
}
// slots = descriptors, table, pool, feedback. tracking = epoch, diagnostics
// buffer, viewport width, viewport height. Consumer owns UAV declarations.
float4 vtSample(uint4 slots,uint4 tracking,uint textureId,float2 uv,float2 dx,float2 dy,uint2 pixel,out float desired,out float actual) {
    uint4 d=buffers[NonUniformResourceIndex(slots.x)].Load4(textureId*32);
    uint flags=buffers[NonUniformResourceIndex(slots.x)].Load(textureId*32+16);
    float rho=max(length(dx*float2(d.xy)),length(dy*float2(d.xy)));
    desired=clamp(log2(max(rho,1.)),0.,float(d.w-1));uint target=uint(floor(desired));
    uint ignored;uint minimum=WaveActiveMin(uint(desired*256.));bool sameTexture=WaveActiveAllEqual(textureId);
    if(!sameTexture || WaveIsFirstLane())buffers[NonUniformResourceIndex(slots.w)].InterlockedMin((16+textureId)*4,sameTexture?minimum:uint(desired*256.),ignored);
    float4 result=0;actual=d.w-1;bool found=false;
    // Request one finer mip as a velocity-independent prefetch margin, plus all
    // parents. An allocation never replaces a missing parent with a fine page.
    uint tail=0;while(max(d.x>>tail,d.y>>tail)>128)tail++;
    // Uniform mip iteration keeps neighboring lanes at the same tree level.
    // Pinned tails never need timestamps or requests.
    for(uint mip=0;mip<=max(target,tail);mip++) {
        if(mip+1<target)continue;
        float2 local;uint page=vtPage(d,mip,uv,local);
        if(mip<tail)vtRequest(slots.w,page,tracking.x);
        uint slot=buffers[NonUniformResourceIndex(slots.y)].Load(page*4);
        if(!found && mip>=target && slot!=0) {actual=float(mip);result=vtBilinear(slots.z,slot-1,local,(flags&1)!=0);found=true;}
    }
    uint samples=WaveActiveCountBits(true),late=WaveActiveCountBits(actual>desired+1.);
    uint lag=WaveActiveMax(uint(max(0.,actual-desired)*256.));
    if(WaveIsFirstLane()){
        buffers[NonUniformResourceIndex(slots.w)].InterlockedAdd(0,samples,ignored);
        buffers[NonUniformResourceIndex(slots.w)].InterlockedMax(8,lag,ignored);
        if(late>0)buffers[NonUniformResourceIndex(slots.w)].InterlockedAdd(4,late,ignored);
    }
    if(all(pixel<tracking.zw))buffers[NonUniformResourceIndex(tracking.y)].Store4((pixel.y*tracking.z+pixel.x)*16,asuint(float4(uv,desired,actual)));
    return result;
}
