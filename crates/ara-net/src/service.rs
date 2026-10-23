mod world;
use crate::{
    Client, ClientConfig, Conditions, Link, NetError, PolicyRegistry, Result, Server, ServerConfig,
};
use ara_host::{Cx, Host, Method, Service};
use ara_input::{ActionValue, Injection, PlayerId};
use ara_proto::Scope;
use ara_schema::{Schema, SchemaDb};
use ara_sim::{Session, SessionMode, SimulationFeature};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Instant;
pub use world::WorldControl;

/// Control plane implemented by an in-process runner or the editor's player-pool adapter.
pub trait NetControl: 'static {
    /// Starts Host+N or Dedicated+N without blocking for convergence.
    fn start(&mut self, mode: SessionMode, clients: u16, seed: u64) -> Result<Value>;
    /// Stops all owned servers/clients.
    fn stop(&mut self) -> Result<Value>;
    /// Read-only session/timeline diagnostics.
    fn status(&mut self) -> Result<Value>;
    /// Applies recorded-seed directional datagram impairment to one client.
    fn conditions(&mut self, player: PlayerId, up: Conditions, down: Conditions) -> Result<Value>;
    /// Per-field payload bandwidth accounting, with transport stats reported separately.
    fn profile(&mut self) -> Result<Value>;
    /// Injects one typed action into a headless owner.
    fn bot(
        &mut self,
        player: PlayerId,
        tick: u64,
        action: String,
        value: ActionValue,
        hold: u32,
    ) -> Result<Value>;
    /// Nonblocking host tick; implementations bound their work.
    fn poll(&mut self) -> Result<()>;
}

/// Schema-described net service registered by network-enabled host composition.
pub struct NetService<C: NetControl> {
    control: C,
}
impl<C: NetControl> NetService<C> {
    /// Wraps an idle control plane. No network resources are created until net.start.
    pub fn new(control: C) -> Self {
        Self { control }
    }
}
/// Registers all six net methods and their ara_net MCP action bindings on the host.
pub fn register<C: NetControl>(
    host: &mut Host,
    control: C,
) -> std::result::Result<(), ara_diag::Diagnostic> {
    host.register(Box::new(NetService::new(control)))
}

/// Parameters shared by net inspection and stop methods.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(deny_unknown_fields)]
struct NetSessionParams {
    /// Engine Protocol session handle.
    session: String,
}
/// Parameters for starting a multiplayer topology.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(deny_unknown_fields)]
struct NetStartParams {
    /// Engine Protocol session handle.
    session: String,
    /// Authority mode: listen or dedicated.
    mode: String,
    /// Number of additional remote clients.
    #[ara(range(min = 1, max = 16))]
    clients: u16,
    /// Recorded conditioner seed.
    seed: u64,
}
/// Parameters for directional per-client conditions.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(deny_unknown_fields)]
struct NetConditionsParams {
    /// Engine Protocol session handle.
    session: String,
    /// Input owner whose datagrams are impaired.
    player: u64,
    /// Upstream conditions.
    up: Conditions,
    /// Downstream conditions.
    down: Conditions,
}
/// Parameters for action-level bot input.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NetBotParams {
    /// Engine Protocol session handle.
    session: String,
    /// Explicit bot input owner.
    player: u64,
    /// Absolute game tick.
    tick: u64,
    /// Declared action name.
    action: String,
    /// ActionValue as typed serde JSON (Button, Axis or Axis2).
    value: Value,
    /// Positive hold duration in ticks.
    #[ara(range(min = 1, max = 36000))]
    hold_ticks: u32,
}
impl<C: NetControl> Service for NetService<C> {
    fn name(&self) -> &'static str {
        "net"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        db.register::<Conditions>();
        vec![
            Method::new::<NetStartParams>(
                db,
                "net.start",
                "Start Host+N or Dedicated+N multiplayer.",
            ),
            Method::new::<NetSessionParams>(db, "net.stop", "Stop all multiplayer players."),
            Method::new::<NetSessionParams>(
                db,
                "net.status",
                "Inspect multiplayer timelines and privacy.",
            )
            .read_only(),
            Method::new::<NetConditionsParams>(
                db,
                "net.conditions",
                "Set seeded per-client datagram conditions.",
            ),
            Method::new::<NetSessionParams>(
                db,
                "net.profile",
                "Inspect field payload and encrypted transport bytes.",
            )
            .read_only(),
            Method::new::<NetBotParams>(
                db,
                "net.bots",
                "Drive a headless owner through typed actions.",
            ),
        ]
        .into_iter()
        .map(|m| {
            let action = m.name.trim_start_matches("net.").to_owned();
            m.scope(Scope::Net).action("ara_net", &action)
        })
        .collect()
    }
    fn call(
        &mut self,
        _cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> std::result::Result<Value, Vec<ara_diag::Diagnostic>> {
        let result = (|| -> Result<Value> {
            match method {
                "net.start" => {
                    let p: NetStartParams = serde_json::from_value(params)?;
                    let _ = p.session;
                    let mode = match p.mode.as_str() {
                        "listen" => SessionMode::Listen,
                        "dedicated" => SessionMode::Dedicated,
                        _ => return Err(NetError("mode must be listen or dedicated".into())),
                    };
                    self.control.start(mode, p.clients, p.seed)
                }
                "net.conditions" => {
                    let p: NetConditionsParams = serde_json::from_value(params)?;
                    let _ = p.session;
                    self.control.conditions(PlayerId(p.player), p.up, p.down)
                }
                "net.bots" => {
                    let p: NetBotParams = serde_json::from_value(params)?;
                    let _ = p.session;
                    self.control.bot(
                        PlayerId(p.player),
                        p.tick,
                        p.action,
                        serde_json::from_value(p.value)?,
                        p.hold_ticks,
                    )
                }
                "net.stop" | "net.status" | "net.profile" => {
                    let p: NetSessionParams = serde_json::from_value(params)?;
                    let _ = p.session;
                    match method {
                        "net.stop" => self.control.stop(),
                        "net.status" => self.control.status(),
                        _ => self.control.profile(),
                    }
                }
                _ => Err(NetError("unknown net method".into())),
            }
        })();
        result.map_err(|e| {
            vec![ara_diag::Diagnostic::error(
                ara_diag::codes::INVALID_PARAMS,
                e.to_string(),
            )]
        })
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.control.poll() {
            cx.log(ara_diag::Diagnostic::error(
                ara_diag::codes::INVALID_PARAMS,
                e.to_string(),
            ));
        }
    }
}
