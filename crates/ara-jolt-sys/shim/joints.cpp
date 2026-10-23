#include "internal.h"
#include <Jolt/Physics/Constraints/FixedConstraint.h>
#include <Jolt/Physics/Constraints/HingeConstraint.h>
#include <Jolt/Physics/Constraints/SliderConstraint.h>
#include <Jolt/Physics/Constraints/DistanceConstraint.h>
#include <Jolt/Physics/Constraints/PointConstraint.h>
#include <Jolt/Physics/Constraints/SixDOFConstraint.h>

extern "C" uint32_t ara_joint_create(AraWorld* w,const AraJointDesc* d) {
    try {
        require(w && d && finite(d->anchor_a) && finite(d->anchor_b),"invalid joint anchors");
        require(d->body_a != d->body_b && d->body_b != kInvalid,"joint needs distinct bodies");
        Body& a=d->body_a==kInvalid ? Body::sFixedToWorld : w->body(d->body_a); Body& b=w->body(d->body_b);
        require(a.IsDynamic() || b.IsDynamic(),"joint needs a dynamic body");
        auto axis=v3(d->axis), normal=v3(d->normal);
        require(axis.IsNormalized() && normal.IsNormalized() && std::abs(axis.Dot(normal))<1.0e-4f,"joint axes must be orthonormal");
        for (uint32_t i=0;i<6;++i) require(finite(d->lower[i]) && finite(d->upper[i]),"nonfinite joint limits");
        Ref<TwoBodyConstraint> c;
        auto p1=RVec3(v3(d->anchor_a)),p2=RVec3(v3(d->anchor_b));
        switch(d->kind) {
        case 0: { FixedConstraintSettings s; s.mPoint1=p1;s.mPoint2=p2;s.mAxisX1=s.mAxisX2=axis;s.mAxisY1=s.mAxisY2=normal; c=s.Create(a,b);break; }
        case 1: {
            require(d->lower[3]>=-JPH_PI && d->lower[3]<=0 && d->upper[3]>=0 && d->upper[3]<=JPH_PI,"invalid hinge limits");
            HingeConstraintSettings s;s.mPoint1=p1;s.mPoint2=p2;s.mHingeAxis1=s.mHingeAxis2=axis;s.mNormalAxis1=s.mNormalAxis2=normal;
            s.mLimitsMin=d->lower[3];s.mLimitsMax=d->upper[3];c=s.Create(a,b);break;
        }
        case 2: {
            require(d->lower[0]<=0 && d->upper[0]>=0,"invalid slider limits");
            SliderConstraintSettings s;s.mPoint1=p1;s.mPoint2=p2;s.mSliderAxis1=s.mSliderAxis2=axis;s.mNormalAxis1=s.mNormalAxis2=normal;
            s.mLimitsMin=d->lower[0];s.mLimitsMax=d->upper[0];c=s.Create(a,b);break;
        }
        case 3: {
            require(d->lower[0]>=0 && d->upper[0]>=d->lower[0],"invalid distance limits");
            DistanceConstraintSettings s;s.mPoint1=p1;s.mPoint2=p2;s.mMinDistance=d->lower[0];s.mMaxDistance=d->upper[0];c=s.Create(a,b);break;
        }
        case 4: { PointConstraintSettings s;s.mPoint1=p1;s.mPoint2=p2;c=s.Create(a,b);break; }
        case 5: {
            SixDOFConstraintSettings s;s.mPosition1=p1;s.mPosition2=p2;s.mAxisX1=s.mAxisX2=axis;s.mAxisY1=s.mAxisY2=normal;
            s.mSwingType=ESwingType::Pyramid;
            for (uint32_t i=0;i<6;++i) s.SetLimitedAxis(SixDOFConstraintSettings::EAxis(i),d->lower[i],d->upper[i]);
            c=s.Create(a,b);break;
        }
        default: throw std::runtime_error("unknown joint kind");
        }
        w->joints.push_back({c,d->body_a,d->body_b});w->system.AddConstraint(c);++w->structure;return uint32_t(w->joints.size()-1);
    } ARA_CATCH(kInvalid)
}
extern "C" uint32_t ara_joint_destroy(AraWorld* w,uint32_t id) {
    try {
        require(w && id<w->joints.size() && w->joints[id].constraint!=nullptr,"stale joint handle");
        auto& c=w->joints[id].constraint;w->system.RemoveConstraint(c);c=nullptr;++w->structure;return 1;
    } ARA_CATCH(0)
}
