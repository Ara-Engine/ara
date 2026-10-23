//! Private Lightyear/Replicon implementation, based on the released S4 adapter.
use crate::{
    FieldBytes, Intent, IntentGate, NetError, NetId, PolicyRegistry, Projection, Result, Spawn,
    error,
};
use ara_input::{ActionValue, Injection, PlayerId};
use ara_schema::{Audience, Schema};
use ara_sim::{Session, SessionAdapter, SessionMode, SimulationFeature, TickRecord};
use bevy_app::{App, FixedPreUpdate, FixedUpdate, PreUpdate};
use bevy_ecs::{
    entity::{EntityMapper, MapEntities},
    prelude::*,
};
use bevy_reflect::Reflect;
use bevy_replicon::prelude::{AppVisibilityExt, ScopeLifetime, SingleComponent, VisibilityFilter};
use lightyear::{prelude::input::native::*, prelude::*};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    time::Duration,
};
mod client;
mod server;
pub use client::{Client, ClientConfig, ClientStatus};
pub use server::{Server, ServerConfig};

const PROTOCOL: u64 = 0xa4a0_0002_0001;

/// Encrypted short-lived join credential; never log or persist it with replay artifacts.
pub struct JoinToken(pub(crate) Vec<u8>);
impl JoinToken {
    /// Exports the token for a trusted player-process launch channel.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
    /// Imports a token received through a trusted launch/authentication channel.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        lightyear::netcode::ConnectToken::try_from_bytes(&bytes).map_err(error)?;
        Ok(Self(bytes))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
struct Input {
    #[serde(deserialize_with = "crate::bounds::bytes::<_,256>")]
    data: Vec<u8>,
}
impl MapEntities for Input {
    fn map_entities<M: EntityMapper>(&mut self, _: &mut M) {}
}
impl Input {
    fn typed<F: SimulationFeature>(&self) -> Result<F::Input> {
        if self.data.len() > 256 {
            return Err(NetError("feature input exceeds 256 bytes".into()));
        }
        let input = if self.data.is_empty() {
            F::Input::default()
        } else {
            postcard::from_bytes(&self.data)?
        };
        F::input(Some(F::action(&input)?))?;
        Ok(input)
    }
}
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct ReplicaTick(u64);
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct PublicFields(Projection);
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct OwnerFields(Projection);

#[derive(Component)]
#[component(immutable)]
struct PrivateTo(Option<PlayerId>);
impl VisibilityFilter for PrivateTo {
    type ClientComponent = RemoteId;
    type Scope = SingleComponent<OwnerFields>;
    const LIFETIME: ScopeLifetime = ScopeLifetime::WhileVisible;
    fn is_visible(&self, _: Entity, remote: Option<&RemoteId>) -> bool {
        self.0
            .is_some_and(|p| remote.is_some_and(|r| r.0 == PeerId::Netcode(p.0)))
    }
}
#[derive(Component)]
#[component(immutable)]
struct RelevantTo(BTreeSet<u64>);
impl VisibilityFilter for RelevantTo {
    type ClientComponent = RemoteId;
    type Scope = Entity;
    const LIFETIME: ScopeLifetime = ScopeLifetime::WhileVisible;
    fn is_visible(&self, _: Entity, remote: Option<&RemoteId>) -> bool {
        remote.is_some_and(|r| matches!(r.0, PeerId::Netcode(p) if self.0.contains(&p)))
    }
}
struct Control;
#[derive(Serialize, Deserialize)]
enum ToServer {
    Ready { schema: [u8; 32], feature: [u8; 32] },
    Intent(Intent),
}
#[derive(Serialize, Deserialize)]
enum ToClient {
    Begin { origin: u32 },
    Retired { id: NetId },
    Rejected,
}

fn protocol<F: SimulationFeature>(
    app: &mut App,
    server: bool,
    policies: PolicyRegistry,
    step: Duration,
) -> Result<()> {
    let desc = F::State::describe();
    // Native history restores the complete feature unit. Partial visibility needs
    // schema projections instead; never silently serialize a hidden field here.
    policies.validate_feature(&desc.name)?;
    app.component::<Spawn>().replicate();
    app.component::<ReplicaTick>().replicate();
    app.component::<PublicFields>().replicate();
    app.component::<OwnerFields>().replicate();
    app.component::<F::State>().replicate().predict();
    app.interpolate_with::<F::State>(InterpolationFns::interpolate(F::interpolate));
    if server {
        app.add_visibility_filter::<PrivateTo>();
        app.add_visibility_filter::<RelevantTo>();
    }
    app.register_message::<ToServer>()
        .add_direction(NetworkDirection::ClientToServer);
    app.register_message::<ToClient>()
        .add_direction(NetworkDirection::ServerToClient);
    app.add_channel::<Control>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        priority: 10.0,
        ..Default::default()
    })
    .add_direction(NetworkDirection::Bidirectional);
    app.add_plugins(InputPlugin::<Input> {
        config: lightyear::input::config::InputConfig {
            send_interval: step,
            packet_redundancy: 8,
            ..Default::default()
        },
    });
    app.insert_resource(Policies(policies));
    Ok(())
}
#[derive(Resource)]
struct Policies(PolicyRegistry);
#[derive(Resource)]
struct Compatibility([u8; 32]);
fn base<F: SimulationFeature>(server: bool, policies: PolicyRegistry, hz: u32) -> Result<App> {
    if hz == 0 || hz > 1000 {
        return Err(NetError("network tick rate must be 1..1000 Hz".into()));
    }
    let step = Duration::from_secs_f64(1.0 / f64::from(hz));
    let mut app = App::new();
    app.add_plugins(bevy_state::app::StatesPlugin);
    if server {
        app.add_plugins(lightyear::prelude::server::ServerPlugins {
            tick_duration: step,
        });
    } else {
        app.add_plugins(lightyear::prelude::client::ClientPlugins {
            tick_duration: step,
        });
    }
    protocol::<F>(&mut app, server, policies, step)?;
    app.insert_resource(Compatibility(
        ara_sim::StateHash::of_bytes(
            format!(
                "{}:{}:{}:{hz}",
                F::State::describe().name,
                F::ACTION,
                F::VERSION
            )
            .as_bytes(),
        )
        .0,
    ));
    Ok(app)
}
fn finish(app: &mut App) {
    app.finish();
    app.cleanup();
}
fn send(world: &mut World, link: Entity, message: ToClient) {
    if let Some(mut sender) = world.get_mut::<MessageSender<ToClient>>(link) {
        sender.send::<Control>(message);
    }
}
