use crate::{NetError, Result};
use ara_input::PlayerId;
use ara_schema::{SchemaDb, Validator, ValueType};
use ara_sim::{BoundaryCommand, PredictionKey, Session};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Authority-only, owner-callable intent exposure. This cannot expose document edits.
#[derive(Debug, Clone)]
pub struct Expose {
    /// Registered simulation command name.
    pub command: String,
    /// Schema params type used for bounds/unknown-field validation.
    pub params: String,
    /// Maximum accepted intents per receive-time window.
    pub count: u32,
    /// Window length in authoritative ticks.
    pub window_ticks: u64,
}

/// Client request. Caller identity is deliberately absent: it comes from encryption/authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    /// Original sampled tick; the authority may assign the next tick to a late intent.
    pub tick: u64,
    /// Monotonic per-player sequence, retained across retries/reconnects.
    pub sequence: u64,
    /// Exposed simulation command.
    #[serde(deserialize_with = "crate::bounds::text::<_,128>")]
    pub command: String,
    /// Owned stable simulation target.
    #[serde(deserialize_with = "crate::bounds::text::<_,256>")]
    pub target: String,
    /// Bounded typed JSON params, validated before allocation of boundary work.
    #[serde(deserialize_with = "crate::bounds::bytes::<_,4096>")]
    pub payload: Vec<u8>,
}

/// Authentication/ownership/tick/bounds/rate admission before Session::submit.
pub struct IntentGate {
    schema: SchemaDb,
    exposed: BTreeMap<String, Expose>,
    authenticated: BTreeSet<PlayerId>,
    sequence: BTreeMap<PlayerId, u64>,
    rates: BTreeMap<(PlayerId, String), VecDeque<u64>>,
    /// Number admitted, useful to protocol tooling.
    pub accepted: u64,
    /// Number rejected, including replayed and unauthorized requests.
    pub rejected: u64,
}
impl IntentGate {
    /// Creates a closed gate: commands and callers must be explicitly admitted.
    pub fn new(schema: SchemaDb) -> Self {
        Self {
            schema,
            exposed: BTreeMap::new(),
            authenticated: BTreeSet::new(),
            sequence: BTreeMap::new(),
            rates: BTreeMap::new(),
            accepted: 0,
            rejected: 0,
        }
    }
    /// Exposes an existing typed simulation command with a bounded rate policy.
    pub fn expose(&mut self, policy: Expose) -> Result<()> {
        if policy.count == 0
            || policy.count > 64
            || policy.window_ticks == 0
            || self.schema.get(&policy.params).is_none()
            || policy.command.is_empty()
            || self.exposed.contains_key(&policy.command)
        {
            return Err(NetError("invalid or duplicate expose policy".into()));
        }
        self.exposed.insert(policy.command.clone(), policy);
        Ok(())
    }
    /// Admits an authenticated transport peer; never called from a message body.
    pub fn authenticate(&mut self, player: PlayerId) {
        self.authenticated.insert(player);
    }
    /// Revokes a peer while retaining sequence/rate state to prevent reconnect replay.
    pub fn disconnect(&mut self, player: PlayerId) {
        self.authenticated.remove(&player);
    }
    /// Validates and queues an intent; target ownership comes from authoritative spawn metadata.
    pub fn submit(
        &mut self,
        session: &mut Session,
        caller: PlayerId,
        owner: Option<PlayerId>,
        intent: Intent,
    ) -> Result<u64> {
        let result = self.admit(session, caller, owner, intent);
        if result.is_ok() {
            self.accepted += 1;
        } else {
            self.rejected += 1;
        }
        result
    }
    fn admit(
        &mut self,
        session: &mut Session,
        caller: PlayerId,
        owner: Option<PlayerId>,
        intent: Intent,
    ) -> Result<u64> {
        let now = session.tick();
        if !self.authenticated.contains(&caller) || owner != Some(caller) {
            return Err(NetError(
                "intent caller is not the authenticated owner".into(),
            ));
        }
        if intent.tick < now.saturating_sub(60)
            || intent.tick > now.saturating_add(120)
            || intent.payload.len() > 4096
            || intent.target.len() > 256
            || intent.command.len() > 128
        {
            return Err(NetError("intent tick/size window exceeded".into()));
        }
        if intent.sequence == 0
            || self
                .sequence
                .get(&caller)
                .is_some_and(|seq| *seq >= intent.sequence)
        {
            return Err(NetError("replayed intent sequence".into()));
        }
        // Consume identities even on a subsequent validation failure.
        self.sequence.insert(caller, intent.sequence);
        let policy = self
            .exposed
            .get(&intent.command)
            .ok_or_else(|| NetError("command is not exposed".into()))?;
        let payload: serde_json::Value = serde_json::from_slice(&intent.payload)?;
        let mut diagnostics = Vec::new();
        Validator::new(&self.schema, &mut diagnostics).value(
            &ValueType::Named(policy.params.clone()),
            None,
            &payload,
            &intent.command,
        );
        if let Some(d) = diagnostics.into_iter().find(|d| d.is_error()) {
            return Err(NetError(d.message));
        }
        let rates = self
            .rates
            .entry((caller, intent.command.clone()))
            .or_default();
        while rates
            .front()
            .is_some_and(|tick| now.saturating_sub(*tick) >= policy.window_ticks)
        {
            rates.pop_front();
        }
        if rates.len() >= policy.count as usize {
            return Err(NetError("intent rate limit exceeded".into()));
        }
        let assigned = intent.tick.max(
            now.checked_add(1)
                .ok_or_else(|| NetError("tick exhausted".into()))?,
        );
        session.submit(
            assigned,
            BoundaryCommand {
                kind: intent.command,
                player: caller,
                sequence: intent.sequence,
                target: Some(intent.target),
                payload,
            },
        )?;
        rates.push_back(now);
        Ok(assigned)
    }
}

/// Bounded effect ledger kept outside simulation. Replaying a key never emits twice.
pub struct CueLedger {
    predicted: BTreeSet<PredictionKey>,
    confirmed: BTreeSet<PredictionKey>,
    retracted: BTreeSet<PredictionKey>,
    capacity: usize,
    retired_through: u64,
}
impl CueLedger {
    /// Creates a bounded speculative/confirmed effect ledger.
    pub fn new(capacity: usize) -> Self {
        Self {
            predicted: BTreeSet::new(),
            confirmed: BTreeSet::new(),
            retracted: BTreeSet::new(),
            capacity,
            retired_through: 0,
        }
    }
    /// Returns true once when a speculative presentation cue should be emitted.
    pub fn predict(&mut self, key: PredictionKey) -> Result<bool> {
        if key.tick <= self.retired_through
            || self.confirmed.contains(&key)
            || self.predicted.contains(&key)
            || self.retracted.contains(&key)
        {
            return Ok(false);
        }
        self.reserve()?;
        Ok(self.predicted.insert(key))
    }
    /// Returns true once for an authority-confirmed irreversible effect.
    pub fn confirm(&mut self, key: PredictionKey) -> Result<bool> {
        if key.tick <= self.retired_through || self.confirmed.contains(&key) {
            return Ok(false);
        }
        if !self.predicted.remove(&key) && !self.retracted.remove(&key) {
            self.reserve()?;
        }
        Ok(self.confirmed.insert(key))
    }
    /// Retracts rejected speculative presentation. Confirmation is never retracted.
    pub fn retract(&mut self, key: PredictionKey) -> bool {
        if self.predicted.remove(&key) {
            self.retracted.insert(key);
            true
        } else {
            false
        }
    }
    /// Retires acknowledged ticks; old replay identities remain suppressed by a watermark.
    pub fn retire(&mut self, through: u64) {
        self.retired_through = self.retired_through.max(through);
        self.predicted.retain(|k| k.tick > self.retired_through);
        self.confirmed.retain(|k| k.tick > self.retired_through);
        self.retracted.retain(|k| k.tick > self.retired_through);
    }
    fn reserve(&self) -> Result<()> {
        if self.predicted.len() + self.confirmed.len() + self.retracted.len() >= self.capacity {
            return Err(NetError("cue history capacity exceeded".into()));
        }
        Ok(())
    }
}
