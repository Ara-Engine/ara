// All lists are stable by slot. No unordered atomic append enters replay state.
[shader("compute")][numthreads(1,1,1)]
void prepareMain(){uint e=root.emitter;uint requested=frame().demands[e].x+state(e,12);uint emitted=min(state(e,4),requested);storeState(e,48,state(e,48)+requested-emitted);}
[shader("compute")][numthreads(256,1,1)]
void spawnMain(uint3 id:SV_DispatchThreadID){
    uint e=root.emitter;Config c=config(e);uint j=id.x;uint regular=frame().demands[e].x;
    uint count=min(state(e,4),regular+state(e,12));if(j>=count)return;
    uint slot=list(c.lists.y+j*4);Particle p;p.position=0;p.velocity=0;p.age=0;p.life=1;p.color=1;p.size=.1;p.birth=root.tick;
    p.seed=hash(frame().demands[e].y^hash(e)^hash(root.tick)^hash(j));initializeParticle(e,p);
    if(j>=regular)p.position+=buffers[NonUniformResourceIndex(root.lists)].Load<float4>(c.scratch.z+(j-regular)*16).xyz;
    storeParticle(c.partition.x+slot,p,0);
}
[shader("compute")][numthreads(256,1,1)]
void updateMain(uint3 id:SV_DispatchThreadID){
    uint e=root.emitter;Config c=config(e);if(id.x>=c.partition.y)return;
    uint slot=c.partition.x+id.x;Particle p=loadParticle(slot);uint events=0;
    if(p.age>=0){float3 previous=p.position;p.age+=frame().clock.x;p.position+=p.velocity*frame().clock.x;
        updateParticle(e,p,previous,frame().clock.x,events);if(p.age>=p.life){events|=2u;p.age=-1;}}
    storeParticle(slot,p,events);
}
groupshared uint aliveScan[256];
groupshared uint eventScan[256];
groupshared float areaScan[256];
[shader("compute")][numthreads(256,1,1)]
void scanMain(uint3 id:SV_DispatchThreadID,uint3 local:SV_GroupThreadID,uint3 group:SV_GroupID){
    Config c=config(root.emitter);uint l=local.x;uint i=id.x;uint a=0;uint events=0;float area=0;
    if(i<c.partition.y){Particle p=loadParticle(c.partition.x+i);a=p.age>=0?1:0;uint flags=buffers[NonUniformResourceIndex(root.particles)].Load((c.partition.x+i+3*frame().extent.z)*16+12);events=countbits(flags&3u);
        if(a!=0){Frame f=frame();float diameter=p.size;if(c.render.x==0){if(c.appearance.x==1)diameter*=2.;if(c.appearance.z==1)diameter=max(diameter,max(p.size*c.motion.x,p.size+length(p.velocity)*f.clock.x*c.motion.y));}if(c.render.x==1)diameter=max(c.surface.z,length(c.boundsMax.xyz-c.boundsMin.xyz));if(c.render.x==2)diameter*=asfloat(f.demands[root.emitter].z);
            float distance=max(f.clock.z,dot(p.position-f.camera.xyz,f.forward.xyz)-diameter*.5);float projected=diameter*f.clock.w/(2*distance);area=max(1./float(f.extent.x*f.extent.y),min(1.,projected*projected*float(f.extent.y)/f.extent.x));}}
    aliveScan[l]=a;eventScan[l]=events;areaScan[l]=area;GroupMemoryBarrierWithGroupSync();
    for(uint stride=1;stride<256;stride*=2){uint pa=l>=stride?aliveScan[l-stride]:0;uint pe=l>=stride?eventScan[l-stride]:0;float ps=l>=stride?areaScan[l-stride]:0;GroupMemoryBarrierWithGroupSync();aliveScan[l]+=pa;eventScan[l]+=pe;areaScan[l]+=ps;GroupMemoryBarrierWithGroupSync();}
    buffers[NonUniformResourceIndex(root.lists)].Store<uint4>(c.lists.z+i*16,uint4(aliveScan[l],eventScan[l],asuint(areaScan[l]),0));
    if(l==255)buffers[NonUniformResourceIndex(root.lists)].Store<uint4>(c.lists.w+group.x*16,uint4(aliveScan[l],eventScan[l],asuint(areaScan[l]),0));
}
[shader("compute")][numthreads(256,1,1)]
void prefixMain(uint3 local:SV_GroupThreadID){
    uint e=root.emitter;Config c=config(e);uint lane=local.x;
    // Contiguous chunks preserve slot order. At 1M particles each lane scans 16 groups.
    uint span=(c.partition.w+255)/256;uint start=lane*span;uint end=min(start+span,c.partition.w);
    uint2 sum=0;float area=0;
    for(uint g=start;g<end;g++){uint4 count=buffers[NonUniformResourceIndex(root.lists)].Load<uint4>(c.lists.w+g*16);sum+=count.xy;area+=asfloat(count.z);}
    aliveScan[lane]=sum.x;eventScan[lane]=sum.y;areaScan[lane]=area;GroupMemoryBarrierWithGroupSync();
    for(uint stride=1;stride<256;stride*=2){uint pa=lane>=stride?aliveScan[lane-stride]:0;uint pe=lane>=stride?eventScan[lane-stride]:0;float ps=lane>=stride?areaScan[lane-stride]:0;GroupMemoryBarrierWithGroupSync();aliveScan[lane]+=pa;eventScan[lane]+=pe;areaScan[lane]+=ps;GroupMemoryBarrierWithGroupSync();}
    uint2 total=uint2(aliveScan[255],eventScan[255]);float totalArea=areaScan[255];
    uint2 prefix=lane>0?uint2(aliveScan[lane-1],eventScan[lane-1]):uint2(0);
    area=lane>0?areaScan[lane-1]:0;uint selected=0;
    GroupMemoryBarrierWithGroupSync();
    for(uint g=start;g<end;g++){
        uint4 count=buffers[NonUniformResourceIndex(root.lists)].Load<uint4>(c.lists.w+g*16);
        buffers[NonUniformResourceIndex(root.lists)].Store<uint2>(c.scratch.x+g*16,prefix);
        float groupArea=asfloat(count.z);
        if(area+groupArea<=c.surface.w)selected=prefix.x+count.x;
        else if(area<c.surface.w){for(uint slot=0;slot<256;slot++){uint4 value=buffers[NonUniformResourceIndex(root.lists)].Load<uint4>(c.lists.z+(g*256+slot)*16);if(area+asfloat(value.z)>c.surface.w)break;selected=prefix.x+value.x;}}
        prefix+=count.xy;area+=groupArea;
    }
    aliveScan[lane]=selected;GroupMemoryBarrierWithGroupSync();
    for(uint stride=128;stride>0;stride/=2){if(lane<stride)aliveScan[lane]=max(aliveScan[lane],aliveScan[lane+stride]);GroupMemoryBarrierWithGroupSync();}
    if(lane==0){storeState(e,0,total.x);storeState(e,4,c.partition.y-total.x);storeState(e,8,min(total.y,c.atlas.z));storeState(e,52,state(e,52)+(total.y>c.atlas.z?total.y-c.atlas.z:0));storeState(e,56,aliveScan[0]);storeState(e,60,asuint(totalArea));}
}
[shader("compute")][numthreads(256,1,1)]
void scatterMain(uint3 id:SV_DispatchThreadID,uint3 group:SV_GroupID){
    uint e=root.emitter;Config c=config(e);uint i=id.x;if(i>=c.partition.y)return;
    uint slot=c.partition.x+i;Particle p=loadParticle(slot);
    uint2 local=buffers[NonUniformResourceIndex(root.lists)].Load<uint2>(c.lists.z+i*16);
    uint2 prefix=buffers[NonUniformResourceIndex(root.lists)].Load<uint2>(c.scratch.x+group.x*16);
    if(p.age>=0)storeList(c.lists.x+(prefix.x+local.x-1)*4,i);
    else storeList(c.lists.y+(i-prefix.x-local.x)*4,i);
    uint flags=buffers[NonUniformResourceIndex(root.particles)].Load((slot+3*frame().extent.z)*16+12)&3u;
    uint cursor=prefix.y+local.y-countbits(flags);
    for(uint trigger=1;trigger<=2;trigger*=2){if((flags&trigger)!=0){if(cursor<c.atlas.z){uint offset=c.scratch.y+cursor*32;buffers[NonUniformResourceIndex(root.lists)].Store<float4>(offset,float4(p.position,0));buffers[NonUniformResourceIndex(root.lists)].Store<uint4>(offset+16,uint4(trigger,e,i,root.tick));}cursor++;}}
}
[shader("compute")][numthreads(1,1,1)]
void routeMain(){
    uint dest=root.emitter;Config target=config(dest);uint routed=0;uint demand=0;
    for(uint source=0;source<frame().extent.w;source++){Config c=config(source);uint count=state(source,8);
        for(uint event=0;event<count;event++){uint offset=c.scratch.y+event*32;uint trigger=list(offset+16);uint2 route=eventTarget(source,trigger);
            if(route.x==dest){float4 position=buffers[NonUniformResourceIndex(root.lists)].Load<float4>(offset);demand+=route.y;
                for(uint child=0;child<route.y&&routed<target.atlas.z;child++){buffers[NonUniformResourceIndex(root.lists)].Store<float4>(target.scratch.z+routed*16,position);routed++;}}}}
    storeState(dest,12,routed);storeState(dest,52,state(dest,52)+demand-routed);
}
float sortKey(uint slot,Config c){if(slot==0xffffffffu)return -3.402823e38;Particle p=loadParticle(c.partition.x+slot);return c.render.w==1?dot(p.position-frame().camera.xyz,frame().forward.xyz):p.age;}
[shader("compute")][numthreads(256,1,1)]
void sortPrepareMain(uint3 id:SV_DispatchThreadID){Config c=config(root.emitter);if(id.x<c.scratch.w&&id.x>=state(root.emitter,56))storeList(c.lists.x+id.x*4,0xffffffffu);}
[shader("compute")][numthreads(256,1,1)]
void sortMain(uint3 id:SV_DispatchThreadID){
    Config c=config(root.emitter);uint i=id.x;uint j=i^root.sortJ;if(j<=i||j>=c.scratch.w)return;
    uint a=list(c.lists.x+i*4);uint b=list(c.lists.x+j*4);float ka=sortKey(a,c);float kb=sortKey(b,c);
    bool greater=ka>kb||(ka==kb&&a<b);bool descending=(i&root.sortK)==0;
    if(greater!=descending){storeList(c.lists.x+i*4,b);storeList(c.lists.x+j*4,a);}
}
[shader("compute")][numthreads(1,1,1)]
void argsMain(){
    uint e=root.emitter;Config c=config(e);uint alive=state(e,0);uint visible=frame().demands[e].w;
    uint rendered=visible!=0?min(alive,state(e,56)):0;
    if(c.render.x==1)rendered=rendered>1?rendered-1:0;
    buffers[NonUniformResourceIndex(root.control)].Store<uint4>(e*64+16,uint4(c.render.y,rendered,0,0));
    buffers[NonUniformResourceIndex(root.control)].Store<uint3>(e*64+32,uint3((state(e,4)+255)/256,1,1));
    storeState(e,56,rendered);
}
