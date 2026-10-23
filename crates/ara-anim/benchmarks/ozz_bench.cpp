// Ara-owned benchmark adapter. ozz itself remains in the gitignored source clone.
#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <fstream>
#include <functional>
#include <iomanip>
#include <iostream>
#include <memory>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>
#include "json/json.h"
#include "ozz/animation/offline/animation_builder.h"
#include "ozz/animation/offline/raw_animation.h"
#include "ozz/animation/offline/raw_animation_utils.h"
#include "ozz/animation/offline/raw_skeleton.h"
#include "ozz/animation/offline/skeleton_builder.h"
#include "ozz/animation/runtime/animation.h"
#include "ozz/animation/runtime/blending_job.h"
#include "ozz/animation/runtime/local_to_model_job.h"
#include "ozz/animation/runtime/sampling_job.h"
#include "ozz/animation/runtime/skeleton.h"
#include "ozz/base/maths/soa_transform.h"
#include "ozz/base/io/archive.h"
#include "ozz/base/io/stream.h"

namespace oa = ozz::animation;
namespace om = ozz::math;
using Clock = std::chrono::steady_clock;
void check(bool value) { if (!value) throw std::runtime_error("invalid benchmark job/asset"); }
om::Float3 vector3(const Json::Value& v) { return {v[0].asFloat(),v[1].asFloat(),v[2].asFloat()}; }
om::Quaternion quat(const Json::Value& v) { return {v[0].asFloat(),v[1].asFloat(),v[2].asFloat(),v[3].asFloat()}; }
oa::offline::RawSkeleton::Joint joint(const Json::Value& joints, int index) {
    oa::offline::RawSkeleton::Joint out;
    const auto& j = joints[index];
    out.name = j["id"].asString().c_str();
    out.transform = {vector3(j["bind"]["translation"]),quat(j["bind"]["rotation"]),vector3(j["bind"]["scale"])};
    for (int i=0;i<int(joints.size());++i)
        if (!joints[i]["parent"].isNull() && joints[i]["parent"].asInt() == index) out.children.push_back(joint(joints,i));
    return out;
}
struct Actor {
    std::array<std::unique_ptr<oa::SamplingJob::Context>,4> contexts;
    std::array<ozz::vector<om::SoaTransform>,4> sampled;
    std::array<ozz::vector<om::SoaTransform>,2> lanes;
    ozz::vector<om::SoaTransform> output;
    ozz::vector<om::Float4x4> model, palette;
    int phase;
    float weight;
};
void sample(Actor& a, const std::array<ozz::unique_ptr<oa::Animation>,2>& clips, int frame) {
    const float ratio = float((frame+a.phase+1)%60)/60.f;
    for (int i=0;i<4;++i) {
        oa::SamplingJob job;
        job.animation=clips[i%2].get(); job.context=a.contexts[i].get(); job.ratio=ratio; job.output=ozz::make_span(a.sampled[i]);
        check(job.Run());
    }
}
void pair_blend(const oa::Skeleton& s, const ozz::vector<om::SoaTransform>& a,
    const ozz::vector<om::SoaTransform>& b, float weight, ozz::vector<om::SoaTransform>& out) {
    oa::BlendingJob::Layer layers[2];
    layers[0].weight=1.f-weight; layers[0].transform=ozz::make_span(a);
    layers[1].weight=weight; layers[1].transform=ozz::make_span(b);
    oa::BlendingJob job;
    job.layers=ozz::make_span(layers); job.rest_pose=s.joint_rest_poses(); job.output=ozz::make_span(out);
    check(job.Run());
}
void blend(Actor& a, const oa::Skeleton& s, int frame) {
    for (int i=0;i<2;++i) pair_blend(s,a.sampled[i*2],a.sampled[i*2+1],a.weight,a.lanes[i]);
    pair_blend(s,a.lanes[0],a.lanes[1],float((frame+a.phase)%60+1)/60.f,a.output);
}
void model(Actor& a, const oa::Skeleton& s) {
    oa::LocalToModelJob job; job.skeleton=&s; job.input=ozz::make_span(a.output); job.output=ozz::make_span(a.model); check(job.Run());
}

// Error checks are outside the timer, over the same keys + 16 subdivisions.
void accuracy(const oa::offline::RawAnimation& raw, const oa::Animation& clip, const oa::Skeleton& s) {
    oa::SamplingJob::Context context(s.num_joints());
    ozz::vector<om::SoaTransform> sampled(s.num_soa_joints());
    ozz::vector<om::Transform> expected(s.num_joints());
    double translation=0,rotation=0,scale=0;
    for(int sub=0;sub<=960;++sub) {
        const float ratio=float(sub)/960.f;
        oa::SamplingJob job; job.animation=&clip;job.context=&context;job.ratio=ratio;job.output=ozz::make_span(sampled);check(job.Run());
        check(oa::offline::SampleAnimation(raw,ratio,ozz::make_span(expected)));
        for(int j=0;j<s.num_joints();++j) {
            const auto& actual=sampled[j/4]; const auto& e=expected[j]; const int lane=j%4;
            auto value=[&](om::SimdFloat4 simd) {float lanes[4];om::StorePtrU(simd,lanes);return lanes[lane];};
            double t[3]={value(actual.translation.x)-e.translation.x,value(actual.translation.y)-e.translation.y,value(actual.translation.z)-e.translation.z};
            double sc[3]={value(actual.scale.x)-e.scale.x,value(actual.scale.y)-e.scale.y,value(actual.scale.z)-e.scale.z};
            translation=std::max(translation,std::sqrt(t[0]*t[0]+t[1]*t[1]+t[2]*t[2]));
            scale=std::max(scale,std::sqrt(sc[0]*sc[0]+sc[1]*sc[1]+sc[2]*sc[2]));
            auto q=om::Normalize(om::Quaternion(value(actual.rotation.x),value(actual.rotation.y),value(actual.rotation.z),value(actual.rotation.w)));
            const auto eq=om::Normalize(e.rotation);
            double a[4]={q.x,q.y,q.z,q.w},b[4]={eq.x,eq.y,eq.z,eq.w},minus=0,plus=0;
            for(int c=0;c<4;++c) {minus+=(a[c]-b[c])*(a[c]-b[c]);plus+=(a[c]+b[c])*(a[c]+b[c]);}
            rotation=std::max(rotation,4.0*std::asin(std::min(1.0,0.5*std::sqrt(std::min(minus,plus)))));
        }
    }
    std::cerr<<"ozz measured translation_m "<<translation<<" rotation_rad "<<rotation<<" scale "<<scale<<"\n";
}

// Persistent workers, dynamic contiguous actor chunks, one barrier per frame.
class Pool {
    std::vector<std::thread> workers;
    std::atomic<int> epoch{0}, done{0}, next{0};
    bool stop=false;
    std::function<void(int,int)> job;
    int chunk;
public:
    Pool(int count,int size):chunk(size) {
        for(int i=0;i<count;++i) workers.emplace_back([this]{
            int seen=0;
            for (;;) {
                epoch.wait(seen); seen=epoch.load();
                if(stop) break;
                for(int start=next.fetch_add(chunk);start<1000;start=next.fetch_add(chunk)) job(start,std::min(1000,start+chunk));
                done.fetch_add(1); done.notify_one();
            }
        });
    }
    template<class F> void run(F&& f) {
        job=std::forward<F>(f); next=0; done=0;
        epoch.fetch_add(1); epoch.notify_all();
        int completed=done.load();
        while(completed<int(workers.size())) { done.wait(completed); completed=done.load(); }
    }
    ~Pool() { stop=true; epoch.fetch_add(1); epoch.notify_all(); for(auto& t:workers)t.join(); }
};
int main(int argc,char** argv) {
    try {
        if(argc<6) throw std::runtime_error("assets.json frames threads stage chunk");
        Json::Value data; std::ifstream input(argv[1]); input>>data;
        const int frames=std::stoi(argv[2]), threads=std::stoi(argv[3]), chunk=std::stoi(argv[5]);
        const std::string stage=argv[4];
        check(frames>0 && threads>0 && chunk>0 && (stage=="sample" || stage=="blend" || stage=="model" || stage=="palette" || stage=="all"));
        const auto& joints=data["skeleton"]["joints"];
        oa::offline::RawSkeleton raw_s;
        for(int i=0;i<int(joints.size());++i) if(joints[i]["parent"].isNull()) raw_s.roots.push_back(joint(joints,i));
        auto s=oa::offline::SkeletonBuilder()(raw_s); check(bool(s));
        std::array<ozz::unique_ptr<oa::Animation>,2> clips;
        for(int c=0;c<2;++c) {
            check(data["clips"][c]["duration"].asInt()==48000 && data["clips"][c]["ticksPerSecond"].asInt()==48000);
            oa::offline::RawAnimation raw; raw.duration=1.f; raw.tracks.resize(s->num_joints());
            for(int j=0;j<s->num_joints();++j) {
                int original=-1;
                for(int k=0;k<int(joints.size());++k) if(joints[k]["id"].asString()==s->joint_names()[j]) original=k;
                check(original>=0);
                const auto& track=data["clips"][c]["tracks"][original]; auto& out=raw.tracks[j];
                for(const auto& key:track["translations"]) out.translations.push_back({key["tick"].asFloat()/48000.f,vector3(key["value"])});
                for(const auto& key:track["rotations"]) out.rotations.push_back({key["tick"].asFloat()/48000.f,quat(key["value"])});
                for(const auto& key:track["scales"]) out.scales.push_back({key["tick"].asFloat()/48000.f,vector3(key["value"])});
            }
            clips[c]=oa::offline::AnimationBuilder()(raw); check(bool(clips[c]));
            ozz::io::MemoryStream stream; {ozz::io::OArchive archive(&stream);archive<<*clips[c];}
            std::cerr<<"ozz clip "<<c<<" runtime_bytes "<<clips[c]->size()<<" archive_bytes "<<stream.Size()<<"\n";
            accuracy(raw,*clips[c],*s);
        }
        Actor bind; bind.output.assign(s->joint_rest_poses().begin(),s->joint_rest_poses().end()); bind.model.resize(s->num_joints()); model(bind,*s);
        ozz::vector<om::Float4x4> inverse; for(const auto& m:bind.model) inverse.push_back(om::Invert(m));
        std::vector<std::unique_ptr<Actor>> actors;
        for(int i=0;i<1000;++i) {
            auto a=std::make_unique<Actor>(); a->phase=i%60; a->weight=float(i%101)/100.f;
            for(int j=0;j<4;++j) { a->contexts[j]=std::make_unique<oa::SamplingJob::Context>(s->num_joints()); a->sampled[j].resize(s->num_soa_joints()); }
            for(auto& lane:a->lanes) lane.resize(s->num_soa_joints());
            a->output.resize(s->num_soa_joints()); a->model.resize(s->num_joints()); a->palette.resize(s->num_joints());
            sample(*a,clips,0); blend(*a,*s,0); model(*a,*s);
            for(int j=0;j<s->num_joints();++j)a->palette[j]=a->model[j]*inverse[j];
            actors.push_back(std::move(a));
        }
        Pool pool(threads>1?threads:0,chunk);
        std::vector<double> times; times.reserve(frames);
        for(int frame=0;frame<frames+30;++frame) {
            auto run=[&](int first,int last) { for(int i=first;i<last;++i) {
                auto& a=*actors[i];
                if(stage=="sample"||stage=="all")sample(a,clips,frame);
                if(stage=="blend"||stage=="all")blend(a,*s,frame);
                if(stage=="model"||stage=="all")model(a,*s);
                if(stage=="palette"||stage=="all")for(int j=0;j<s->num_joints();++j)a.palette[j]=a.model[j]*inverse[j];
            }};
            auto start=Clock::now(); if(threads==1)run(0,1000);else pool.run(run);
            if(frame>=30)times.push_back(std::chrono::duration<double,std::milli>(Clock::now()-start).count());
        }
        // Observable checksum outside the timer prevents dead-store elimination.
        float checksum=0.f; for(const auto& a:actors) checksum+=om::GetX(a->palette[0].cols[0])+om::GetX(a->model[0].cols[0])+om::GetX(a->output[0].translation.x)+om::GetX(a->sampled[0][0].translation.x);
        std::cerr<<"checksum "<<checksum<<"\n";
        std::sort(times.begin(),times.end());
        std::cout<<stage<<","<<threads<<","<<chunk<<","<<std::fixed<<std::setprecision(6)<<times[frames/2]<<","<<times[frames*95/100]<<"\n";
    } catch(const std::exception& e) { std::cerr<<e.what()<<"\n"; return 1; }
}
