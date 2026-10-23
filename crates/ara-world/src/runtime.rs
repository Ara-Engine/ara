use crate::{CellKey, HlodProxy, Manifest, Result, WorkerPool, invalid, worker::LoadedCell};
use ara_sim::StreamingBoundary;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// A player/camera source. Velocity biases loading toward the direction of travel.
#[derive(Debug, Clone, Copy)]
pub struct StreamingSource {
    /// World-space position in metres.
    pub position: [f32; 3],
    /// Velocity in metres per second; never inferred from wall-clock simulation time.
    pub velocity: [f32; 3],
    /// Range multiplier; spectator cameras can use a smaller footprint.
    pub range_scale: f32,
}

/// Main-thread structural budget. One entity is the smallest indivisible operation.
#[derive(Debug, Clone, Copy)]
pub struct ActivationBudget {
    /// Stop starting operations after this much wall time on the host frame.
    pub time: Duration,
    /// Additional deterministic ceiling, useful for replay and tiny frame budgets.
    pub max_entities: usize,
}

/// Admission and prefetch controls; budgets cover loading, ready, active and retiring.
#[derive(Debug, Clone, Copy)]
pub struct StreamConfig {
    /// Maximum retained cell reservations, including workers and deferred destruction.
    pub memory_bytes: usize,
    /// Conservative per-cell reservation; oversize cells fail with a split diagnostic.
    pub cell_reservation: usize,
    /// Seconds of source velocity used for predictive range and priority.
    pub lookahead_seconds: f32,
}

/// Cell readiness/visibility seam shared by renderer extraction and net relevancy.
/// Consumers must keep callbacks short; transport encoding is deferred by adapters.
pub trait Interest {
    /// A fully activated cell is ready for a renderer/net baseline.
    fn ready(&mut self, cell: &str, key: CellKey, visible: bool, proxies: &[HlodProxy]);
    /// All entities have been removed; discard cell-scoped presentation/net mappings.
    fn removed(&mut self, cell: &str, key: CellKey);
    /// Layer visibility changed while the cell remained resident.
    fn visibility(&mut self, cell: &str, visible: bool);
}

impl Interest for () {
    fn ready(&mut self, _: &str, _: CellKey, _: bool, _: &[HlodProxy]) {}
    fn removed(&mut self, _: &str, _: CellKey) {}
    fn visibility(&mut self, _: &str, _: bool) {}
}

/// Measured activation work for one host frame, excluding workers and game systems.
#[derive(Debug, Default, Clone, Copy)]
pub struct FrameStats {
    /// Main-thread time inside activation/destruction scheduling.
    pub elapsed: Duration,
    /// Spawned ECS rows.
    pub spawned: usize,
    /// Despawned ECS rows.
    pub despawned: usize,
    /// Admitted cell reservations, including in-flight and retiring cells.
    pub reserved_bytes: usize,
    /// Estimated bytes retained by decoded/active cells, excluding worker temporaries.
    pub retained_bytes: usize,
    /// Count of active ECS rows across partially and fully activated cells.
    pub entities: usize,
}

struct State {
    key: CellKey,
    loaded: Option<LoadedCell>,
    count: usize,
    in_flight: bool,
    retiring: bool,
    wanted: bool,
    visible: bool,
    unload: bool,
    last_wanted: u64,
    grace: u32,
    priority: f64,
    notified: bool,
    failure: Option<String>,
    retiring_charge: usize,
}

/// Host-side range planner and incremental cell lifecycle. No world I/O, decoding,
/// waiting or whole-cell ECS mutations occur in `activate`.
pub struct Streaming {
    workers: WorkerPool,
    config: StreamConfig,
    cells: BTreeMap<String, State>,
    reserved: usize,
    retained: usize,
    entities: usize,
    last_tick: u64,
    adopt: bool,
}

impl Streaming {
    /// Starts empty. The immutable source and manifest must describe one generation.
    pub fn new(manifest: &Manifest, workers: WorkerPool, config: StreamConfig) -> Result<Self> {
        if config.cell_reservation == 0
            || workers.cell_limit != config.cell_reservation
            || config.memory_bytes < config.cell_reservation
            || !config.lookahead_seconds.is_finite()
            || config.lookahead_seconds < 0.0
        {
            return Err(invalid("invalid streaming memory/lookahead configuration"));
        }
        let cells = manifest
            .cells
            .iter()
            .map(|(id, cell)| {
                (
                    id.to_owned(),
                    State {
                        key: cell.key(),
                        loaded: None,
                        count: 0,
                        in_flight: false,
                        retiring: false,
                        wanted: false,
                        visible: manifest.layers[&cell.layer].visible,
                        unload: false,
                        last_wanted: 0,
                        grace: manifest.layers[&cell.layer].grace_ticks,
                        priority: f64::INFINITY,
                        notified: false,
                        failure: None,
                        retiring_charge: 0,
                    },
                )
            })
            .collect();
        Ok(Self {
            workers,
            config,
            cells,
            reserved: 0,
            retained: 0,
            entities: 0,
            last_tick: 0,
            adopt: false,
        })
    }

    /// Updates interest from explicit host ticks. Call once per frame before the
    /// boundary. Runtime layer changes are read from the newly committed manifest.
    pub fn update(
        &mut self,
        manifest: &Manifest,
        sources: &[StreamingSource],
        tick: u64,
        interest: &mut impl Interest,
    ) -> Result<()> {
        if tick < self.last_tick || manifest.cells.len() != self.cells.len() {
            return Err(invalid(
                "stream tick went backwards or cell generation changed",
            ));
        }
        for source in sources {
            if source
                .position
                .iter()
                .chain(&source.velocity)
                .any(|v| !v.is_finite())
                || !source.range_scale.is_finite()
                || source.range_scale < 0.0
            {
                return Err(invalid("invalid streaming source"));
            }
        }
        self.last_tick = tick;
        while let Some(completion) = self.workers.poll() {
            let state = self
                .cells
                .get_mut(&completion.cell)
                .ok_or_else(|| invalid("worker returned unknown cell"))?;
            match completion.result {
                Ok(Some(cell)) => {
                    state.in_flight = false;
                    if cell.charge > self.config.cell_reservation || cell.blob.key != state.key {
                        cell_failed(
                            &completion.cell,
                            "it exceeds the runtime reservation or its key differs",
                        );
                        state.failure =
                            Some("cell exceeds runtime reservation or key differs".into());
                        state.retiring = true;
                        self.workers.retire(cell);
                    } else {
                        self.retained += cell.charge;
                        state.loaded = Some(cell);
                    }
                }
                Ok(None) => {
                    state.retiring = false;
                    self.reserved -= self.config.cell_reservation;
                    self.retained -= state.retiring_charge;
                    state.retiring_charge = 0;
                }
                Err(error) => {
                    cell_failed(&completion.cell, &error.to_string());
                    state.in_flight = false;
                    state.failure = Some(error.to_string());
                    self.reserved -= self.config.cell_reservation;
                }
            }
        }
        for (id, state) in &mut self.cells {
            let spec = manifest
                .cells
                .get(id)
                .ok_or_else(|| invalid("cell generation changed"))?;
            if spec.key() != state.key {
                return Err(invalid("cell key changed; create a new generation"));
            }
            let layer = manifest
                .layers
                .get(&spec.layer)
                .ok_or_else(|| invalid("missing data layer"))?;
            state.grace = layer.grace_ticks;
            if state.visible != layer.visible {
                state.visible = layer.visible;
                if state.notified {
                    interest.visibility(id, state.visible);
                }
            }
            state.priority = f64::INFINITY;
            state.wanted = false;
            if layer.loaded {
                for source in sources {
                    let current = distance(state.key, manifest.settings.cell_size, source.position);
                    let predicted = std::array::from_fn(|i| {
                        source.position[i] + source.velocity[i] * self.config.lookahead_seconds
                    });
                    let ahead = distance(state.key, manifest.settings.cell_size, predicted);
                    let range = f64::from(layer.range) * f64::from(source.range_scale);
                    if current <= range || ahead <= range {
                        state.wanted = true;
                        state.priority = state.priority.min(current * 0.25 + ahead * 0.75);
                    }
                }
            }
            if state.wanted {
                state.last_wanted = tick;
            }
            if !state.wanted && tick.saturating_sub(state.last_wanted) >= u64::from(state.grace) {
                state.unload = true;
            }
        }
        self.workers.collect();
        if self.reserved > self.config.memory_bytes - self.config.cell_reservation {
            let waiting = self
                .cells
                .values()
                .filter(|s| {
                    s.wanted
                        && s.loaded.is_none()
                        && !s.in_flight
                        && !s.retiring
                        && s.failure.is_none()
                })
                .map(|s| s.priority)
                .min_by(f64::total_cmp);
            if let Some(priority) = waiting {
                // Under pressure a nearer cell replaces the least useful admitted
                // cell, even within its grace period. Reservations release only
                // after the worker confirms destruction, preventing overcommit.
                if let Some((_, state)) = self
                    .cells
                    .iter_mut()
                    .filter(|(_, s)| {
                        (s.loaded.is_some() || s.in_flight) && !s.unload && s.priority > priority
                    })
                    .max_by(|(a, sa), (b, sb)| sa.priority.total_cmp(&sb.priority).then(a.cmp(b)))
                {
                    state.unload = true;
                }
            }
        }
        // Nearest predictive cells enter first; stable ID breaks ties across machines.
        loop {
            if self.reserved > self.config.memory_bytes - self.config.cell_reservation {
                break;
            }
            let candidate = self
                .cells
                .iter_mut()
                .filter(|(_, s)| {
                    s.wanted
                        && s.loaded.is_none()
                        && !s.in_flight
                        && !s.retiring
                        && s.failure.is_none()
                })
                .min_by(|(a, sa), (b, sb)| sa.priority.total_cmp(&sb.priority).then(a.cmp(b)));
            let Some((id, state)) = candidate else {
                break;
            };
            if !self.workers.request(id) {
                break;
            }
            state.in_flight = true;
            state.unload = false;
            self.reserved += self.config.cell_reservation;
        }
        Ok(())
    }

    /// Adoption after a checkpoint or snapshot restore: the restored world already
    /// holds rows of the cells resident at the checkpoint, which this (fresh) state
    /// never spawned. While on, activation counts a row whose identity is resident as
    /// spawned (no spawn, no budget, nothing recorded) instead of failing on the
    /// duplicate. Turn it off once those cells are resident again: duplicate authored
    /// identities must keep failing.
    pub fn adopt_resident(&mut self, adopt: bool) {
        self.adopt = adopt;
    }

    /// `update`, then waits for every cook and retirement it started (repeating until
    /// nothing is in flight, at most `timeout`), so the next `activate` sees every
    /// wanted cell loaded. Hosts that need reproducible arrival ticks (agent
    /// playtests, replays, load screens) call it instead of `update`: arrivals then
    /// depend only on sources and ticks, never on worker speed. False on timeout
    /// (whatever finished is admitted; the rest stays in flight).
    pub fn settle(
        &mut self,
        manifest: &Manifest,
        sources: &[StreamingSource],
        tick: u64,
        interest: &mut impl Interest,
        timeout: Duration,
    ) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            self.update(manifest, sources, tick, interest)?;
            if !self.workers.pending() {
                return Ok(true);
            }
            if !self.workers.wait_idle(deadline) {
                return Ok(false);
            }
        }
    }

    /// Spreads activation and unloading across frames. Call from the closure passed
    /// to `Session::step_with_streaming`. Clock decisions stay in this host crate;
    /// ara-sim records the resulting structural arrivals for deterministic replay.
    pub fn activate(
        &mut self,
        boundary: &mut StreamingBoundary<'_>,
        budget: ActivationBudget,
        interest: &mut impl Interest,
    ) -> Result<FrameStats> {
        let start = Instant::now();
        let mut stats = FrameStats::default();
        let adopt = self.adopt;
        while stats.spawned + stats.despawned < budget.max_entities && start.elapsed() < budget.time
        {
            let candidate = self
                .cells
                .iter_mut()
                .filter(|(_, s)| s.loaded.is_some() && (s.unload || !s.notified))
                .min_by(|(a, sa), (b, sb)| {
                    sb.unload
                        .cmp(&sa.unload)
                        .then(sa.priority.total_cmp(&sb.priority))
                        .then(a.cmp(b))
                });
            let Some((id, state)) = candidate else {
                break;
            };
            let cell = state.loaded.as_mut().expect("selected cell is loaded");
            if state.unload {
                if state.count > 0 {
                    boundary.despawn(&cell.identities[state.count - 1])?;
                    state.count -= 1;
                    self.entities -= 1;
                    stats.despawned += 1;
                }
                if state.count == 0 {
                    if state.notified {
                        interest.removed(id, state.key);
                        state.notified = false;
                    }
                    let cell = state.loaded.take().expect("cell was just selected");
                    state.retiring_charge = cell.charge;
                    state.retiring = true;
                    self.workers.retire(cell);
                }
            } else {
                if let Some(entity) = cell.prepared.pop_front() {
                    if adopt && boundary.contains(entity.identity()) {
                        // Restored with the world: already resident.
                    } else {
                        boundary.spawn(entity)?;
                        stats.spawned += 1;
                    }
                    state.count += 1;
                    self.entities += 1;
                }
                if cell.prepared.is_empty() {
                    interest.ready(id, state.key, state.visible, &cell.blob.proxies);
                    state.notified = true;
                }
            }
        }
        stats.elapsed = start.elapsed();
        stats.reserved_bytes = self.reserved;
        stats.retained_bytes = self.retained;
        stats.entities = self.entities;
        Ok(stats)
    }

    /// Whether a cell's entire entity set is activated and available for net baselines.
    pub fn is_resident(&self, cell: &str) -> bool {
        self.cells
            .get(cell)
            .is_some_and(|s| s.notified && !s.unload)
    }

    /// Admission reservations including background destruction; bounded by config.
    pub fn reserved_bytes(&self) -> usize {
        self.reserved
    }

    /// Number of currently active streamed ECS rows.
    pub fn entity_count(&self) -> usize {
        self.entities
    }

    /// Read-only per-connection interest hook. Returns fully resident cells in one
    /// source's current/predictive range. Net adapters additionally enforce owner
    /// and private-field policy; presentation visibility is not authorization.
    pub fn relevant_cells<'a>(
        &'a self,
        manifest: &'a Manifest,
        source: StreamingSource,
    ) -> Result<impl Iterator<Item = (&'a str, CellKey)> + 'a> {
        if source
            .position
            .iter()
            .chain(&source.velocity)
            .any(|v| !v.is_finite())
            || !source.range_scale.is_finite()
            || source.range_scale < 0.0
        {
            return Err(invalid("invalid interest source"));
        }
        let predicted = std::array::from_fn(|i| {
            source.position[i] + source.velocity[i] * self.config.lookahead_seconds
        });
        Ok(self.cells.iter().filter_map(move |(id, state)| {
            let cell = manifest.cells.get(id)?;
            let layer = manifest.layers.get(&cell.layer)?;
            let range = f64::from(layer.range) * f64::from(source.range_scale);
            (state.notified
                && !state.unload
                && layer.loaded
                && distance(state.key, manifest.settings.cell_size, source.position).min(distance(
                    state.key,
                    manifest.settings.cell_size,
                    predicted,
                )) <= range)
                .then_some((id.as_str(), state.key))
        }))
    }

    /// Last worker error. A failed cell is not retried every frame.
    pub fn failure(&self, cell: &str) -> Option<&str> {
        self.cells.get(cell)?.failure.as_deref()
    }

    /// Explicit retry after the user fixes a missing/corrupt/oversized cell.
    pub fn retry(&mut self, cell: &str) -> Result<()> {
        self.cells
            .get_mut(cell)
            .ok_or_else(|| invalid("unknown cell"))?
            .failure = None;
        Ok(())
    }
}

fn distance(key: CellKey, size: f32, position: [f32; 3]) -> f64 {
    let size = f64::from(size);
    let min = [f64::from(key.x) * size, f64::from(key.z) * size];
    let point = [f64::from(position[0]), f64::from(position[2])];
    let delta: [f64; 2] =
        std::array::from_fn(|i| (min[i] - point[i]).max(point[i] - (min[i] + size)).max(0.0));
    delta[0].hypot(delta[1])
}

/// Reports a failed cell on the diagnostics bus (`ARA2202`, about the cell).
fn cell_failed(cell: &str, why: &str) {
    ara_diag::Diagnostic::error(
        ara_diag::codes::WORLD_CELL_FAILED,
        format!("world cell `{cell}` failed to load: {why}; it stays unloaded until retried"),
    )
    .subject(ara_diag::Subject::document(cell))
    .emit();
}
