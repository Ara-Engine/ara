#include "internal.h"
#include <Jolt/Physics/Collision/Shape/CapsuleShape.h>
#include <Jolt/Physics/Collision/Shape/RotatedTranslatedShape.h>

namespace {
RefConst<Shape> capsule(float radius, float height) {
    auto r = RotatedTranslatedShapeSettings(Vec3(0,height*0.5f,0),Quat::sIdentity(),new CapsuleShape(height*0.5f-radius,radius)).Create();
    require(!r.HasError(), "character capsule creation failed"); return r.Get();
}
bool set_crouch(AraWorld* w, CharacterSlot& c, bool crouch, float max_penetration) {
    auto layer = ObjectLayer(c.desc.layer);
    if (!c.character->SetShape(crouch ? c.crouching : c.standing, max_penetration,
        w->system.GetDefaultBroadPhaseLayerFilter(layer),w->system.GetDefaultLayerFilter(layer),{},{},*w->temp)) return false;
    c.crouched = crouch; return true;
}
}
extern "C" {
uint32_t ara_character_create(AraWorld* w, const AraCharacterDesc* d) {
    try {
        require(w && d && finite(d->position), "invalid character descriptor");
        require(d->radius > 0 && finite(d->radius) && finite(d->height) && finite(d->crouch_height)
            && d->height > 2*d->radius && d->crouch_height > 2*d->radius && d->crouch_height <= d->height, "invalid character capsule sizes");
        require(d->layer > 0 && d->layer < 16 && d->max_slope_deg > 0 && d->max_slope_deg < 90
            && d->padding >= 0 && d->mass > 0 && d->max_strength >= 0 && d->step_up >= 0 && d->stick_down >= 0, "invalid character movement settings");
        uint32_t id = uint32_t(w->characters.size());
        CharacterSlot c; c.desc = *d; c.standing = capsule(d->radius,d->height); c.crouching = capsule(d->radius,d->crouch_height);
        Ref<CharacterVirtualSettings> s = new CharacterVirtualSettings();
        s->mID = CharacterID(id+1); // Never use Jolt's process-global automatic character IDs.
        s->mMaxSlopeAngle = DegreesToRadians(d->max_slope_deg); s->mMaxStrength = d->max_strength;
        s->mMass = d->mass; s->mShape = c.standing; s->mCharacterPadding = d->padding;
        s->mPenetrationRecoverySpeed = 1.0f; s->mPredictiveContactDistance = 0.1f;
        s->mSupportingVolume = Plane(Vec3::sAxisY(),-d->radius);
        c.character = new CharacterVirtual(s,RVec3(v3(d->position)),Quat::sIdentity(),id,&w->system);
        c.character->SetListener(&w->character_listener);
        c.character->SetCharacterVsCharacterCollision(&w->character_collision);
        w->characters.push_back(c); w->character_listener.sliding.push_back(1);
        w->character_collision.Add(c.character); ++w->structure; return id;
    } ARA_CATCH(kInvalid)
}
uint32_t ara_character_destroy(AraWorld* w, uint32_t id) {
    try { require(w,"null world"); auto& c=w->character(id); w->character_collision.Remove(c.character); c.character=nullptr; ++w->structure; return 1; } ARA_CATCH(0)
}
uint32_t ara_character_step(AraWorld* w, const AraCharacterInput* input, float dt) {
    try {
        require(w && input && finite(dt) && dt > 0 && finite(input->desired) && input->desired.y == 0
            && finite(input->jump_speed) && input->jump_speed >= 0, "invalid character input");
        auto& slot = w->character(input->character); auto& c = *slot.character; const auto& d = slot.desc;
        bool crouch = input->crouch != 0;
        if (crouch != slot.crouched) set_crouch(w,slot,crouch,0.01f);
        c.UpdateGroundVelocity();
        bool grounded = c.GetGroundState() == CharacterBase::EGroundState::OnGround;
        Vec3 velocity = grounded ? c.GetGroundVelocity() : Vec3(0,c.GetLinearVelocity().GetY(),0);
        bool jumping = grounded && input->jump_speed > 0;
        if (jumping) velocity += Vec3(0,input->jump_speed,0);
        velocity += w->system.GetGravity()*dt + v3(input->desired);
        c.SetLinearVelocity(velocity);
        w->character_listener.sliding[input->character] = !v3(input->desired).IsNearZero() || jumping;
        CharacterVirtual::ExtendedUpdateSettings us;
        us.mStickToFloorStepDown = jumping ? Vec3::sZero() : Vec3(0,-d.stick_down,0);
        us.mWalkStairsStepUp = Vec3(0,d.step_up,0);
        auto layer = ObjectLayer(d.layer);
        c.ExtendedUpdate(dt,w->system.GetGravity(),us,w->system.GetDefaultBroadPhaseLayerFilter(layer),
            w->system.GetDefaultLayerFilter(layer),{},{},*w->temp);
        return 1;
    } ARA_CATCH(0)
}
uint32_t ara_character_read(const AraWorld* w, uint32_t id, AraCharacterState* out) {
    try {
        require(w && out,"null character state"); const auto& s=w->character(id); const auto& c=*s.character;
        *out={a3(Vec3(c.GetPosition())),a3(c.GetLinearVelocity()),a3(c.GetGroundVelocity()),a3(c.GetGroundNormal()),
              c.GetGroundBodyID().GetIndexAndSequenceNumber(),uint32_t(c.GetGroundState()),uint32_t(s.crouched)};
        return 1;
    } ARA_CATCH(0)
}
}
