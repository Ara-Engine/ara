#include "internal.h"

extern "C" {
uint32_t ara_bodies_create(AraWorld* w, const AraBodyDesc* descs, uint32_t count, uint32_t* out) {
    std::vector<BodyID> batch;
    try {
        require(w && (!count || (descs && out)), "null body batch");
        auto& bi = w->system.GetBodyInterfaceNoLock();
        require(count<=w->system.GetMaxBodies()-w->system.GetNumBodies(),"body capacity exhausted");
        // Validate the complete batch before allocating any native object.
        for (uint32_t i=0; i<count; ++i) {
            const auto& d = descs[i]; auto* shape = w->shape(d.shape);
            require(d.motion <= 2 && d.layer < 16 && (d.motion == 0 || d.layer != 0), "invalid body motion/layer");
            require(d.motion == 0 || !shape->MustBeStatic(), "mesh/heightfield requires a static body");
            require(finite(d.position) && unit(d.rotation) && finite(d.linear) && finite(d.angular), "invalid body pose/velocity");
            require(finite(d.mass) && d.mass >= 0 && finite(d.friction) && d.friction >= 0 && finite(d.restitution) && d.restitution >= 0 && d.restitution <= 1, "invalid mass/material");
        }
        batch.reserve(count);
        for (uint32_t i=0; i<count; ++i) {
            const auto& d = descs[i];
            BodyCreationSettings s(w->shape(d.shape), RVec3(v3(d.position)), q4(d.rotation), EMotionType(d.motion), ObjectLayer(d.layer));
            s.mFriction = d.friction; s.mRestitution = d.restitution;
            s.mIsSensor = (d.flags & 1) != 0; s.mAllowSleeping = (d.flags & 2) != 0;
            s.mCollideKinematicVsNonDynamic = s.mIsSensor;
            s.mMotionQuality = (d.flags & 4) ? EMotionQuality::LinearCast : EMotionQuality::Discrete;
            s.mLinearVelocity = v3(d.linear); s.mAngularVelocity = v3(d.angular); s.mUserData = d.user_data;
            if (d.mass > 0) { s.mOverrideMassProperties = EOverrideMassProperties::CalculateInertia; s.mMassPropertiesOverride.mMass = d.mass; }
            Body* body = bi.CreateBody(s);
            require(body, "body capacity exhausted");
            batch.push_back(body->GetID());
            out[i] = body->GetID().GetIndexAndSequenceNumber();
        }
        if (count) {
            auto state = bi.AddBodiesPrepare(batch.data(), int(count));
            bi.AddBodiesFinalize(batch.data(), int(count), state, EActivation::Activate);
            w->structure+=count;
        }
        return 1;
    } catch (const std::exception& e) {
        if (w) for (auto id : batch) w->system.GetBodyInterfaceNoLock().DestroyBody(id);
        set_error(e.what()); return 0;
    } catch (...) {
        if (w) for (auto id : batch) w->system.GetBodyInterfaceNoLock().DestroyBody(id);
        set_error("body creation exception"); return 0;
    }
}
uint32_t ara_body_destroy(AraWorld* w, uint32_t id) {
    try {
        require(w, "null world"); w->body(id);
        require(!w->has_joint(id), "remove attached joints before destroying a body");
        for (const auto& v : w->vehicles) require(v.constraint == nullptr || v.chassis.GetIndexAndSequenceNumber() != id, "destroy vehicle through vehicle API");
        auto& bi = w->system.GetBodyInterfaceNoLock();
        bi.RemoveBody(BodyID(id)); bi.DestroyBody(BodyID(id)); ++w->structure; return 1;
    } ARA_CATCH(0)
}
uint32_t ara_bodies_read(const AraWorld* w, const uint32_t* ids, uint32_t count, AraBodyState* out) {
    try {
        require(w && (!count || (ids && out)), "null extraction buffer");
        for (uint32_t i=0; i<count; ++i) require(w->find(ids[i]), "stale body handle");
        for (uint32_t i=0; i<count; ++i) {
            auto* b = w->find(ids[i]);
            out[i] = {ids[i], uint32_t(b->IsActive()), a3(Vec3(b->GetPosition())), aq(b->GetRotation()),
                      a3(b->GetLinearVelocity()), a3(b->GetAngularVelocity())};
        }
        return 1;
    } ARA_CATCH(0)
}
uint32_t ara_bodies_apply(AraWorld* w, const AraBodyCommand* commands, uint32_t count, float dt) {
    try {
        require(w && (!count || commands) && finite(dt) && dt > 0, "invalid body commands");
        for (uint32_t i=0; i<count; ++i) {
            const auto& c = commands[i]; auto& b = w->body(c.body);
            require(c.kind <= 5 && finite(c.position) && finite(c.linear) && finite(c.angular), "invalid body command");
            if (c.kind <= 1) require(unit(c.rotation), "invalid target rotation");
            if (c.kind == 1) require(b.IsKinematic(), "kinematic target requires a kinematic body");
            if (c.kind == 3 || c.kind == 4) require(b.IsDynamic(), "forces and impulses require a dynamic body");
        }
        auto& bi = w->system.GetBodyInterfaceNoLock();
        for (uint32_t i=0; i<count; ++i) {
            const auto& c = commands[i]; BodyID id(c.body);
            switch (c.kind) {
            case 0: bi.SetPositionAndRotation(id,RVec3(v3(c.position)),q4(c.rotation),EActivation::Activate);bi.InvalidateContactCache(id);break;
            case 1: bi.MoveKinematic(id,RVec3(v3(c.position)),q4(c.rotation),dt); break;
            case 2: bi.SetLinearAndAngularVelocity(id,v3(c.linear),v3(c.angular)); break;
            case 3: bi.AddImpulse(id,v3(c.linear),RVec3(v3(c.position))); bi.AddAngularImpulse(id,v3(c.angular)); break;
            case 4: bi.AddForce(id,v3(c.linear)); bi.AddTorque(id,v3(c.angular)); break;
            case 5: bi.ActivateBody(id); break;
            }
        }
        return 1;
    } ARA_CATCH(0)
}
}
