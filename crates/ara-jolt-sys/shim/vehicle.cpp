#include "internal.h"
#include <Jolt/Physics/Collision/Shape/BoxShape.h>
#include <Jolt/Physics/Collision/Shape/OffsetCenterOfMassShape.h>

extern "C" {
uint32_t ara_vehicle_create(AraWorld* w, const AraVehicleDesc* d) {
    BodyID chassis_id;
    try {
        require(w && d && finite(d->position) && finite(d->half_extents) && unit(d->rotation),"invalid vehicle pose");
        require(v3(d->half_extents).ReduceMin() > 0 && d->mass > 0 && finite(d->mass)
            && d->wheel_radius > 0 && d->wheel_width > 0 && d->suspension_min >= 0
            && d->suspension_max > d->suspension_min && d->suspension_frequency > 0
            && d->suspension_damping >= 0 && d->max_steer_deg >= 0 && d->max_steer_deg < 90
            && d->engine_max_torque > 0 && d->layer > 0 && d->layer < 16,"invalid vehicle settings");
        for (auto p : d->wheel_offsets) require(finite(p),"invalid wheel offset");
        auto& bi=w->system.GetBodyInterfaceNoLock(); auto half=v3(d->half_extents);
        auto result=OffsetCenterOfMassShapeSettings(Vec3(0,-half.GetY(),0),new BoxShape(half,std::min(cDefaultConvexRadius,0.5f*half.ReduceMin()))).Create();
        require(!result.HasError(),"vehicle shape failed");
        BodyCreationSettings cs(result.Get(),RVec3(v3(d->position)),q4(d->rotation),EMotionType::Dynamic,ObjectLayer(d->layer));
        cs.mOverrideMassProperties=EOverrideMassProperties::CalculateInertia; cs.mMassPropertiesOverride.mMass=d->mass; cs.mUserData=d->user_data;
        Body* chassis=bi.CreateBody(cs); require(chassis,"vehicle body capacity exhausted"); chassis_id=chassis->GetID();
        VehicleConstraintSettings vs;
        for (int i=0;i<4;++i) {
            auto* wheel=new WheelSettingsWV;
            wheel->mPosition=v3(d->wheel_offsets[i]); wheel->mRadius=d->wheel_radius; wheel->mWidth=d->wheel_width;
            wheel->mSuspensionMinLength=d->suspension_min; wheel->mSuspensionMaxLength=d->suspension_max;
            wheel->mSuspensionSpring.mFrequency=d->suspension_frequency; wheel->mSuspensionSpring.mDamping=d->suspension_damping;
            wheel->mMaxSteerAngle=i<2 ? DegreesToRadians(d->max_steer_deg) : 0;
            if (i<2) wheel->mMaxHandBrakeTorque=0;
            vs.mWheels.push_back(wheel);
        }
        vs.mAntiRollBars.resize(2);
        vs.mAntiRollBars[0].mLeftWheel=0; vs.mAntiRollBars[0].mRightWheel=1;
        vs.mAntiRollBars[1].mLeftWheel=2; vs.mAntiRollBars[1].mRightWheel=3;
        auto* controller=new WheeledVehicleControllerSettings;
        controller->mEngine.mMaxTorque=d->engine_max_torque; controller->mDifferentials.resize(1);
        controller->mDifferentials[0].mLeftWheel=d->rear_wheel_drive ? 2 : 0;
        controller->mDifferentials[0].mRightWheel=d->rear_wheel_drive ? 3 : 1;
        vs.mController=controller;
        Ref<VehicleConstraint> vc=new VehicleConstraint(*chassis,vs);
        vc->SetVehicleCollisionTester(new VehicleCollisionTesterRay(ObjectLayer(d->layer)));
        w->vehicles.push_back({vc,chassis_id});
        bi.AddBody(chassis_id,EActivation::Activate); w->system.AddConstraint(vc); w->system.AddStepListener(vc);
        ++w->structure; return uint32_t(w->vehicles.size()-1);
    } catch (const std::exception& e) {
        if (w && !chassis_id.IsInvalid()) w->system.GetBodyInterfaceNoLock().DestroyBody(chassis_id);
        set_error(e.what()); return kInvalid;
    } catch (...) { set_error("vehicle creation exception"); return kInvalid; }
}
uint32_t ara_vehicle_destroy(AraWorld* w, uint32_t id) {
    try {
        require(w,"null world"); auto& v=w->vehicle(id);
        require(!w->has_joint(v.chassis.GetIndexAndSequenceNumber()),"remove chassis joints first");
        w->system.RemoveStepListener(v.constraint); w->system.RemoveConstraint(v.constraint); v.constraint=nullptr;
        auto& bi=w->system.GetBodyInterfaceNoLock(); bi.RemoveBody(v.chassis); bi.DestroyBody(v.chassis);
        ++w->structure; return 1;
    } ARA_CATCH(0)
}
uint32_t ara_vehicle_input(AraWorld* w,const AraVehicleInput* d) {
    try {
        require(w && d && finite(d->throttle) && finite(d->steer) && finite(d->brake) && finite(d->hand_brake)
            && std::abs(d->throttle)<=1 && std::abs(d->steer)<=1 && d->brake>=0 && d->brake<=1 && d->hand_brake>=0 && d->hand_brake<=1,"invalid driver input");
        auto& v=w->vehicle(d->vehicle);
        if (d->throttle || d->steer || d->brake || d->hand_brake) w->system.GetBodyInterfaceNoLock().ActivateBody(v.chassis);
        static_cast<WheeledVehicleController*>(v.constraint->GetController())->SetDriverInput(d->throttle,d->steer,d->brake,d->hand_brake);
        return 1;
    } ARA_CATCH(0)
}
uint32_t ara_vehicle_read(const AraWorld* w,uint32_t id,AraVehicleState* out) {
    try {
        require(w && out,"null vehicle output"); const auto& v=w->vehicle(id); const auto& vc=*v.constraint;
        out->chassis=v.chassis.GetIndexAndSequenceNumber(); out->wheels_in_contact=0;
        for (uint32_t i=0;i<4;++i) {
            out->wheels_in_contact+=vc.GetWheel(i)->HasContact() ? 1 : 0;
            out->wheel_positions[i]=a3(Vec3(vc.GetWheelWorldTransform(i,Vec3::sAxisX(),Vec3::sAxisY()).GetTranslation()));
        }
        auto* ctl=static_cast<const WheeledVehicleController*>(vc.GetController());
        out->engine_rpm=ctl->GetEngine().GetCurrentRPM(); out->gear=ctl->GetTransmission().GetCurrentGear(); return 1;
    } ARA_CATCH(0)
}
}
