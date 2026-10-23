use super::*;

#[derive(Clone)]
pub(super) struct Binding {
    pub body: Option<BodyHandle>,
    pub character: Option<CharacterHandle>,
    pub vehicle: Option<VehicleHandle>,
    pub joint: Option<JointHandle>,
    pub joint_settings: Option<Joint>,
    rigid: Option<RigidBody>,
    collider: Option<Collider>,
    controller: Option<CharacterController>,
    car: Option<Vehicle>,
    cooked: Option<CookedCollider>,
    pub last_pose: PhysicsPose,
    pub last_velocity: Velocity,
}

pub(super) fn validate(world: &World, e: Entity) -> Result<()> {
    let rigid = world.get::<RigidBody>(e);
    let character = world.get::<CharacterController>(e);
    let vehicle = world.get::<Vehicle>(e);
    if usize::from(rigid.is_some())
        + usize::from(character.is_some())
        + usize::from(vehicle.is_some())
        > 1
    {
        return Err(PhysicsError(
            "an entity may have one of RigidBody, CharacterController or Vehicle".into(),
        ));
    }
    if rigid.is_some() && world.get::<Collider>(e).is_none() {
        return Err(PhysicsError("RigidBody requires Collider".into()));
    }
    if let Some(c) = world.get::<Collider>(e)
        && c.kind == ColliderKind::Cooked
        && world.get::<CookedCollider>(e).is_none()
    {
        return Err(PhysicsError(
            "cook and attach the collider asset before the tick boundary".into(),
        ));
    }
    if world.get::<Joint>(e).is_some() && rigid.is_none() && vehicle.is_none() {
        return Err(PhysicsError("Joint requires RigidBody or Vehicle".into()));
    }
    Ok(())
}

impl Binding {
    pub fn body(&self) -> Option<BodyHandle> {
        self.body
    }
    pub fn motion(&self) -> MotionType {
        self.rigid.map_or(MotionType::Dynamic, |r| r.motion)
    }
    pub fn matches(&self, world: &World, e: Entity) -> bool {
        self.rigid.as_ref() == world.get::<RigidBody>(e)
            && self.collider.as_ref() == world.get::<Collider>(e)
            && self.controller.as_ref() == world.get::<CharacterController>(e)
            && self.car.as_ref() == world.get::<Vehicle>(e)
            && self.cooked.as_ref() == world.get::<CookedCollider>(e)
    }
    pub fn create<B: PhysicsBackend>(
        world: &World,
        e: Entity,
        id: u64,
        backend: &mut B,
    ) -> Result<Self> {
        let pose = world.get::<PhysicsPose>(e).copied().unwrap_or_default();
        let velocity = world.get::<Velocity>(e).copied().unwrap_or_default();
        let rigid = world.get::<RigidBody>(e).copied();
        let controller = world.get::<CharacterController>(e).copied();
        let car = world.get::<Vehicle>(e).copied();
        let cooked = world.get::<CookedCollider>(e).copied();
        // Owned authoring data is copied only on creation/configuration replacement.
        let collider = world.get::<Collider>(e).cloned();
        let mut b = Self {
            body: None,
            character: None,
            vehicle: None,
            joint: None,
            joint_settings: None,
            rigid,
            collider,
            controller,
            car,
            cooked,
            last_pose: pose,
            last_velocity: velocity,
        };
        if let (Some(body), Some(c)) = (rigid, b.collider.as_ref()) {
            let shape = match c.kind {
                ColliderKind::Box => backend.create_shape(Shape::Box {
                    half_extents: c.half_extents,
                })?,
                ColliderKind::Sphere => backend.create_shape(Shape::Sphere { radius: c.radius })?,
                ColliderKind::Capsule => backend.create_shape(Shape::Capsule {
                    radius: c.radius,
                    half_height: c.half_height,
                })?,
                ColliderKind::Cylinder => backend.create_shape(Shape::Cylinder {
                    radius: c.radius,
                    half_height: c.half_height,
                })?,
                ColliderKind::Cooked => {
                    cooked
                        .ok_or_else(|| PhysicsError("missing cooked shape".into()))?
                        .0
                }
            };
            let mut ids = Vec::with_capacity(1);
            backend.create_bodies(
                &[BodyDesc {
                    shape,
                    user_data: id,
                    pose,
                    body,
                    collider: c.clone(),
                    velocity,
                }],
                &mut ids,
            )?;
            b.body = ids.first().copied();
        }
        if let Some(controller) = controller {
            b.character = Some(backend.create_character(CharacterDesc {
                user_data: id,
                position: pose.position,
                controller,
            })?);
        }
        if let Some(vehicle) = car {
            let handle = backend.create_vehicle(VehicleDesc::new(vehicle, pose, id))?;
            b.body = Some(backend.vehicle_state(handle)?.chassis);
            b.vehicle = Some(handle);
        }
        Ok(b)
    }
    pub fn destroy<B: PhysicsBackend>(self, backend: &mut B) -> Result<()> {
        if let Some(v) = self.vehicle {
            backend.destroy_vehicle(v)?;
        } else if let Some(body) = self.body {
            backend.destroy_body(body)?;
        }
        if let Some(c) = self.character {
            backend.destroy_character(c)?;
        }
        Ok(())
    }
}
