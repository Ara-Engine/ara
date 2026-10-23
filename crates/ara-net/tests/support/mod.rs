use ara_input::{Action, ActionKind, ActionMap, ActionValue, PlayerId};
use ara_net::{Expose, PolicyRegistry, Result, Server};
use ara_schema::{Schema, SchemaDb};
use ara_sim::{
    EditWorld, Identity, Session, SimContext, SimError, SimRegistry, SimulationFeature, TickConfig,
};
use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};

pub static SERIALIZATIONS: AtomicUsize = AtomicUsize::new(0);
/// Test-only observer proving ordinary Local ticks never request serialization.
#[derive(Component, Schema, Deserialize)]
#[ara(component)]
pub struct Probe {
    /// Probe marker, which never affects gameplay.
    pub value: u64,
}
impl Serialize for Probe {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        SERIALIZATIONS.fetch_add(1, Ordering::Relaxed);
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("Probe", 1)?;
        out.serialize_field("value", &self.value)?;
        out.end()
    }
}

/// Complete restorable movement state, in integer millimetres.
#[derive(Component, Schema, Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[ara(component)]
pub struct Motor {
    /// First horizontal position in millimetres.
    #[ara(replicate(audience=Relevant,interpolate=Linear,rollback))]
    pub x: i32,
    /// Second horizontal position in millimetres.
    #[ara(replicate(audience=Relevant,interpolate=Linear,rollback))]
    pub z: i32,
}
/// Authored input owner, distinct from authority and recipient policy.
#[derive(Component, Schema, Serialize, Deserialize)]
#[ara(component)]
pub struct Player {
    /// Explicit player identity.
    pub player: u64,
}
/// Mixed public, owner-only and private state in one logical component.
#[derive(Component, Schema, Serialize, Deserialize, Debug, PartialEq)]
#[ara(component)]
pub struct Inventory {
    /// Public health.
    #[ara(replicate(audience=Relevant))]
    pub hp: u16,
    /// Owner-only item bitset.
    #[ara(replicate(audience=Owner))]
    pub items: u64,
    /// Authority-only future-affecting timer participates in restore, never wire state.
    #[ara(replicate(rollback))]
    pub secret: u32,
}
/// Durable shared door.
#[derive(Component, Schema, Serialize, Deserialize)]
#[ara(component)]
pub struct Door {
    /// Traversal is permitted.
    #[ara(replicate(audience=Relevant))]
    pub open: bool,
}
/// Static pickup baseline with a quantized coordinate.
#[derive(Component, Schema, Serialize, Deserialize)]
#[ara(component)]
pub struct Item {
    /// Position in metres.
    #[ara(replicate(audience=Relevant,quantize=0.01,interpolate=Linear))]
    pub x: f32,
    /// Item can be collected.
    #[ara(replicate(audience=Relevant))]
    pub available: bool,
}
/// Public stable reference used to test out-of-order/interest-enter baselines.
#[derive(Component, Schema, Serialize, Deserialize)]
#[ara(component)]
pub struct Linkage {
    /// Optional referenced entity lifetime.
    #[ara(replicate(audience=Relevant))]
    pub target: Option<ara_schema::EntityRef>,
}
/// Typed heal intent.
#[derive(Schema, Serialize, Deserialize)]
#[ara(params)]
pub struct Heal {
    /// Requested number of items; the server checks schema bounds and gameplay preconditions.
    #[ara(range(min = 1, max = 1))]
    pub count: u8,
}
impl ara_sim::SimCommand for Heal {
    const NAME: &'static str = "heal";
    fn validate(&self, _: &ara_sim::CommandContext) -> std::result::Result<(), SimError> {
        if self.count != 1 {
            return Err(SimError("heal count must be one".into()));
        }
        Ok(())
    }
    fn execute(
        self,
        world: &mut World,
        command: &ara_sim::CommandContext,
    ) -> std::result::Result<(), SimError> {
        let mut q = world.query::<(&Identity, &mut Inventory)>();
        for (id, mut inv) in q.iter_mut(world) {
            if Some(&id.0) == command.target.as_ref() && inv.items >= u64::from(self.count) {
                inv.items -= u64::from(self.count);
                inv.hp = (inv.hp + 10).min(100);
            }
        }
        if command.player == PlayerId(0) {
            let mut q = world.query::<&mut Door>();
            for mut door in q.iter_mut(world) {
                door.open = true;
            }
        }
        Ok(())
    }
}
/// Network-independent movement feature.
pub struct Movement;
impl SimulationFeature for Movement {
    type State = Motor;
    type Input = [i8; 2];
    const ACTION: &'static str = "Move";
    fn input(action: Option<ActionValue>) -> std::result::Result<Self::Input, SimError> {
        match action {
            None => Ok([0; 2]),
            Some(ActionValue::Axis2([x, z]))
                if x.is_finite() && z.is_finite() && x.abs() <= 1.0 && z.abs() <= 1.0 =>
            {
                Ok([x as i8, z as i8])
            }
            _ => Err(SimError("movement axes must be within [-1,1]".into())),
        }
    }
    fn action(input: &Self::Input) -> std::result::Result<ActionValue, SimError> {
        if input.iter().any(|v| !(-1..=1).contains(v)) {
            return Err(SimError("invalid movement input".into()));
        }
        Ok(ActionValue::Axis2([
            f32::from(input[0]),
            f32::from(input[1]),
        ]))
    }
    fn step(state: &mut Motor, input: &Self::Input) {
        state.x += i32::from(input[0]) * 25;
        state.z += i32::from(input[1]) * 25;
    }
    fn interpolate(a: Motor, b: Motor, t: f32) -> Motor {
        Motor {
            x: (a.x as f32 + (b.x - a.x) as f32 * t).round() as i32,
            z: (a.z as f32 + (b.z - a.z) as f32 * t).round() as i32,
        }
    }
    fn correction(a: &Motor, b: &Motor) -> f64 {
        f64::from(a.x - b.x).hypot(f64::from(a.z - b.z))
    }
    fn position(state: &Motor) -> Option<[f32; 2]> {
        Some([state.x as f32 / 1000.0, state.z as f32 / 1000.0])
    }
}
pub fn schema() -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    db.register::<Motor>();
    db.register::<Player>();
    db.register::<Inventory>();
    db.register::<Door>();
    db.register::<Item>();
    db.register::<Heal>();
    db.register::<Probe>();
    db.register::<Linkage>();
    db
}
pub fn policies() -> Result<PolicyRegistry> {
    PolicyRegistry::new(SimRegistry::new(schema())?.schema().clone())
}
pub fn session() -> Result<Session> {
    ara_sim::initialize_compute_pool(
        std::num::NonZeroUsize::new(2).expect("positive worker count"),
    );
    let db = schema();
    let scene = ara_doc::read(
        r#"{"$schema":"ara://schema/scene/1","id":"sc_12345678","records":{}}"#,
        "scene.scene",
        &db,
        None,
    )
    .doc
    .expect("valid empty test scene");
    let mut registry = SimRegistry::new(db)?;
    registry.register_component::<Motor>()?;
    registry.register_component::<Player>()?;
    registry.register_component::<Inventory>()?;
    registry.register_component::<Door>()?;
    registry.register_component::<Item>()?;
    registry.register_component::<Probe>()?;
    registry.register_component::<Linkage>()?;
    registry.register_sim_command::<Heal>()?;
    let mut actions = ActionMap::default();
    actions
        .declare(Action {
            name: "Move".into(),
            kind: ActionKind::Axis2,
        })
        .map_err(SimError)?;
    let mut session = Session::local(
        EditWorld::new(registry, scene)?,
        TickConfig::default(),
        21668,
        actions,
    )?;
    for p in 0..4 {
        session.world_mut().spawn((
            Identity(format!("player{p}")),
            Player { player: p },
            Motor::default(),
            Inventory {
                hp: 50,
                items: 1_u64 << p,
                secret: 0xfeed0000 + p as u32,
            },
            Linkage {
                target: Some(ara_schema::EntityRef("e_0003".into())),
            },
        ));
    }
    session
        .world_mut()
        .spawn((Identity("door".into()), Door { open: false }));
    for n in 0..45 {
        session.world_mut().spawn((
            Identity(if n == 0 {
                "e_0003".into()
            } else {
                format!("item{n:02}")
            }),
            Item {
                x: n as f32 * 0.013,
                available: true,
            },
        ));
    }
    session.add_system("movement", movement)?;
    Ok(session)
}
fn movement(cx: Res<SimContext>, mut q: Query<(&Player, &mut Motor)>) {
    for (player, mut state) in &mut q {
        let input = Movement::input(cx.action(PlayerId(player.player), "Move"))
            .expect("validated typed test input");
        Movement::step(&mut state, &input);
    }
}
pub fn configure(server: &mut Server<Movement>) -> Result<()> {
    for p in 0..4 {
        server.spawn(
            &format!("player{p}"),
            "@players/avatar.prefab#e_0001",
            [1; 32],
            Some(PlayerId(p)),
            None,
        )?;
    }
    server.expose(Expose {
        command: "heal".into(),
        params: "Heal".into(),
        count: 2,
        window_ticks: 60,
    })?;
    Ok(())
}
pub fn script(tick: u64, player: PlayerId) -> Option<ActionValue> {
    // S4's late owner joins after the first movement interval; its first complete
    // out/back interval starts at tick 300, independent of baseline arrival time.
    if player == PlayerId(3) && tick < 300 {
        return Some(ActionValue::Axis2([0.0; 2]));
    }
    let axis = match tick {
        100..=159 | 300..=359 => 1.0,
        160..=219 | 360..=419 => -1.0,
        _ => 0.0,
    };
    Some(ActionValue::Axis2([axis, 0.0]))
}
pub fn build() -> ara_sim::StateHash {
    ara_sim::StateHash::of_bytes(b"ara-net-gates-v1")
}
