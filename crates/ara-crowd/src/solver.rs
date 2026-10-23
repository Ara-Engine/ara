//! One fixed step of reciprocal avoidance for every agent, in parallel.
use crate::{
    CrowdError, Walls,
    grid::Grid,
    orca::{Lines, V2, agent_line, solve, wall_line},
    walls::{MAX_CANDIDATES, closest, crossing},
};
use rayon::prelude::*;

/// Solver tuning. Defaults suit humanoids at walking and running speeds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Neighbour search radius in metres; also the hash-grid cell size.
    pub neighbor_distance: f32,
    /// Neighbours considered per agent, nearest first (1..=24).
    pub max_neighbors: usize,
    /// Agent-agent look-ahead in seconds; longer is smoother but more conservative.
    pub time_horizon: f32,
    /// Wall look-ahead in seconds.
    pub wall_time_horizon: f32,
    /// Clearance kept from walls in metres (navmeshes are already radius-eroded).
    pub wall_margin: f32,
    /// Walls considered per agent, nearest first (0..=16).
    pub max_walls: usize,
}
impl Default for Params {
    fn default() -> Self {
        Self {
            neighbor_distance: 3.0,
            max_neighbors: 10,
            time_horizon: 1.5,
            wall_time_horizon: 0.4,
            wall_margin: 0.02,
            max_walls: 8,
        }
    }
}
impl Params {
    fn validate(&self) -> Result<(), CrowdError> {
        let positive = |v: f32| v.is_finite() && v > 0.0;
        if !positive(self.neighbor_distance)
            || !positive(self.time_horizon)
            || !positive(self.wall_time_horizon)
            || !(self.wall_margin.is_finite() && self.wall_margin >= 0.0)
            || !(1..=24).contains(&self.max_neighbors)
            || self.max_walls > 16
        {
            return Err(CrowdError::new(
                "crowd params must be finite and positive; neighbors 1..=24, walls 0..=16",
            ));
        }
        Ok(())
    }
}

/// Sentinel for [`Agent::walls`]: the agent ignores walls.
pub const NO_WALLS: u16 = u16::MAX;

/// Solver input and output for one agent. Results depend only on the set of
/// agents (ties break by `id`), never on slice order or worker count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Agent {
    /// Stable simulation identity; breaks exact distance ties.
    pub id: u64,
    /// Feet position; overwritten with the integrated result.
    pub position: [f32; 3],
    /// Current velocity; overwritten with the chosen velocity.
    pub velocity: [f32; 3],
    /// Desired path-following velocity (Y follows the horizontal speed ratio).
    pub preferred: [f32; 3],
    /// Collision radius in metres.
    pub radius: f32,
    /// Collision height in metres; agents only avoid vertically overlapping others.
    pub height: f32,
    /// Maximum speed in metres per second.
    pub max_speed: f32,
    /// Maximum velocity change per second.
    pub max_acceleration: f32,
    /// Avoidance layers; two agents avoid each other when their masks intersect.
    pub layers: u32,
    /// Index into the wall sets passed to `step` (per agent profile), or [`NO_WALLS`].
    pub walls: u16,
    /// A fixed disc obstacle: never moves, and others take full responsibility
    /// for avoiding it (avoidance-only dynamic obstacles, parked vehicles).
    pub obstacle: bool,
}
impl Default for Agent {
    fn default() -> Self {
        Self {
            id: 0,
            position: [0.0; 3],
            velocity: [0.0; 3],
            preferred: [0.0; 3],
            radius: 0.3,
            height: 1.8,
            max_speed: 3.0,
            max_acceleration: 8.0,
            layers: 1,
            walls: NO_WALLS,
            obstacle: false,
        }
    }
}

/// Work counts from one step, summed in a fixed order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Agents solved.
    pub agents: usize,
    /// Agents whose neighbour constraints were infeasible (minimal violation used).
    pub fallbacks: usize,
    /// Agents whose integrated move was clipped at a wall crossing.
    pub clipped: usize,
}

/// Compact copy of the fields neighbours read, stored in grid-bucket order.
#[derive(Clone, Copy, Default)]
struct Body {
    x: f32,
    z: f32,
    y: f32,
    vx: f32,
    vz: f32,
    radius: f32,
    height: f32,
    layers: u32,
    obstacle: bool,
}

#[derive(Clone, Copy, Default)]
struct Out {
    position: [f32; 3],
    velocity: [f32; 3],
    fallback: bool,
    clipped: bool,
}

/// Reusable solver scratch. All gameplay state stays with the caller's agents;
/// a `Crowd` can be dropped and recreated between steps without changing results.
pub struct Crowd {
    params: Params,
    grid: Grid,
    bodies: Vec<Body>,
    ids: Vec<u64>,
    out: Vec<Out>,
}

const CHUNK: usize = 128;

impl Crowd {
    /// Creates scratch for validated parameters.
    pub fn new(params: Params) -> Result<Self, CrowdError> {
        params.validate()?;
        Ok(Self {
            params,
            grid: Grid::default(),
            bodies: Vec::new(),
            ids: Vec::new(),
            out: Vec::new(),
        })
    }

    /// Current parameters.
    pub fn params(&self) -> Params {
        self.params
    }

    /// Advances every agent by `dt` on the caller's Rayon pool. `walls[i]` is the
    /// boundary set for agents whose `walls` index is `i` (one per navmesh profile).
    /// Each agent reads only the previous step's state: 1, 4 or 8 workers and any
    /// input order give identical bits per agent ID.
    pub fn step(
        &mut self,
        agents: &mut [Agent],
        walls: &[&Walls],
        dt: f32,
    ) -> Result<Stats, CrowdError> {
        if !(dt.is_finite() && dt > 0.0 && dt <= 0.25) {
            return Err(CrowdError::new("dt must be in (0, 0.25]"));
        }
        if agents.len() > u32::MAX as usize / 2 {
            return Err(CrowdError::new("too many agents"));
        }
        for (i, a) in agents.iter().enumerate() {
            let finite = a
                .position
                .iter()
                .chain(&a.velocity)
                .chain(&a.preferred)
                .all(|v| v.is_finite() && v.abs() < 1.0e6);
            if !finite
                || !(a.radius.is_finite() && a.radius > 0.0 && a.radius <= 10.0)
                || !(a.height.is_finite() && a.height > 0.0)
                || !(a.max_speed.is_finite() && a.max_speed >= 0.0)
                || !(a.max_acceleration.is_finite() && a.max_acceleration >= 0.0)
                || (a.walls != NO_WALLS && usize::from(a.walls) >= walls.len())
            {
                return Err(CrowdError::new(format!("agent {i} has invalid state")));
            }
        }
        self.grid
            .build(self.params.neighbor_distance, agents.len(), |i| {
                (agents[i].position[0], agents[i].position[2])
            });
        let order = self.grid.order();
        self.bodies.clear();
        self.ids.clear();
        for &i in order {
            let a = &agents[i as usize];
            self.bodies.push(Body {
                x: a.position[0],
                z: a.position[2],
                y: a.position[1],
                vx: a.velocity[0],
                vz: a.velocity[2],
                radius: a.radius,
                height: a.height,
                layers: a.layers,
                obstacle: a.obstacle,
            });
            self.ids.push(a.id);
        }
        self.out.clear();
        self.out.resize(agents.len(), Out::default());
        let ctx = Context {
            params: &self.params,
            grid: &self.grid,
            bodies: &self.bodies,
            ids: &self.ids,
            agents,
            walls,
            dt,
        };
        self.out
            .par_chunks_mut(CHUNK)
            .enumerate()
            .for_each(|(chunk, outputs)| {
                for (k, out) in outputs.iter_mut().enumerate() {
                    *out = ctx.agent(chunk * CHUNK + k);
                }
            });
        let mut stats = Stats {
            agents: agents.len(),
            ..Stats::default()
        };
        for (&i, out) in self.grid.order().iter().zip(&self.out) {
            let agent = &mut agents[i as usize];
            agent.position = out.position;
            agent.velocity = out.velocity;
            stats.fallbacks += usize::from(out.fallback);
            stats.clipped += usize::from(out.clipped);
        }
        Ok(stats)
    }
}

struct Context<'a> {
    params: &'a Params,
    grid: &'a Grid,
    bodies: &'a [Body],
    ids: &'a [u64],
    agents: &'a [Agent],
    walls: &'a [&'a Walls],
    dt: f32,
}

/// Ranking key: squared distance, then stable ID (a deterministic total order).
type Key = (f32, u64, u32);

impl Context<'_> {
    /// Solves the agent at bucket-order slot `k`.
    fn agent(&self, k: usize) -> Out {
        let me = self.bodies[k];
        let agent = &self.agents[self.grid.order()[k] as usize];
        if me.obstacle {
            return Out {
                position: agent.position,
                ..Out::default()
            };
        }
        let p = V2::new(me.x, me.z);
        let v = V2::new(me.vx, me.vz);
        let dt = self.dt;
        // Preferred velocity limited by speed, then by reachable acceleration.
        let mut preferred = V2::new(agent.preferred[0], agent.preferred[2]);
        let preferred_len = preferred.len_sq().sqrt();
        if preferred_len > agent.max_speed {
            preferred = preferred.scale(agent.max_speed / preferred_len);
        }
        let dv = preferred - v;
        let dv_len = dv.len_sq().sqrt();
        let max_dv = agent.max_acceleration * dt;
        if dv_len > max_dv {
            preferred = v + dv.scale(max_dv / dv_len);
        }

        let mut lines = Lines::new();
        // Walls first: they are hard constraints in the fallback program.
        let walls = self.walls.get(usize::from(agent.walls)).copied();
        let mut wall_ids = [0u32; MAX_CANDIDATES];
        let mut wall_count = 0;
        if let Some(walls) = walls
            && self.params.max_walls > 0
        {
            let mut near: [Key; 16] = [(0.0, 0, 0); 16];
            let mut near_count = 0;
            wall_count = walls.candidates(me.x, me.z, &mut wall_ids);
            let reach = (self.params.wall_time_horizon * agent.max_speed
                + self.params.wall_margin
                + agent.radius)
                .min(walls.cell);
            for &index in &wall_ids[..wall_count] {
                let s = &walls.segments[index as usize];
                if me.y < s.y_min - me.height || me.y > s.y_max + me.height {
                    continue;
                }
                let d = (p - closest(s.a, s.b, p)).len_sq();
                if d <= reach * reach {
                    let key = (d, u64::from(index), index);
                    insert(&mut near, &mut near_count, self.params.max_walls, key);
                }
            }
            let inv = 1.0 / self.params.wall_time_horizon;
            for &(d, _, index) in &near[..near_count] {
                let s = &walls.segments[index as usize];
                let mut away = p - closest(s.a, s.b, p);
                if d <= 1e-12 {
                    // Exactly on the wall: push toward the side it came from.
                    let edge = s.b - s.a;
                    away = V2::new(-edge.z, edge.x);
                    if away.dot(-v) < 0.0 {
                        away = -away;
                    }
                }
                lines.push(wall_line(away, d.sqrt() - self.params.wall_margin, inv));
            }
        }
        let hard = lines.len;

        let mut neighbors: [Key; 24] = [(0.0, 0, 0); 24];
        let mut count = 0;
        let range_sq = self.params.neighbor_distance * self.params.neighbor_distance;
        self.grid.around(me.x, me.z, |range| {
            for j in range {
                if j == k {
                    continue;
                }
                let other = &self.bodies[j];
                if other.layers & me.layers == 0
                    || (other.y - me.y).abs() * 2.0 >= other.height + me.height
                {
                    continue;
                }
                let dx = other.x - me.x;
                let dz = other.z - me.z;
                let d = dx * dx + dz * dz;
                if d < range_sq {
                    let key = (d, self.ids[j], j as u32);
                    insert(&mut neighbors, &mut count, self.params.max_neighbors, key);
                }
            }
            // Once full, buckets wholly beyond the farthest kept neighbour cannot
            // contribute (strictly farther), so skipping them is exact.
            if count == self.params.max_neighbors {
                neighbors[count - 1].0
            } else {
                range_sq
            }
        });
        // Fixed discs are local minima for pure ORCA when the goal lies behind
        // them: sidestep along the tangent nearer the preferred direction.
        for &(d, _, j) in &neighbors[..count] {
            let other = &self.bodies[j as usize];
            if !other.obstacle {
                continue;
            }
            let to = V2::new(other.x - me.x, other.z - me.z);
            let r = me.radius + other.radius + self.params.wall_margin;
            let speed = preferred.len_sq().sqrt();
            let dist = d.sqrt();
            if speed <= 1e-6 || dist <= r {
                continue;
            }
            let dir = preferred.scale(1.0 / speed);
            let ahead = dir.dot(to);
            let lateral = crate::orca::det(dir, to);
            let reach = speed * self.params.time_horizon * 2.0 + r;
            if ahead <= 0.0 || ahead > reach || lateral.abs() >= r {
                continue; // Not on a collision course within the look-ahead.
            }
            let half = (r / dist).asin();
            let base = to.scale(1.0 / dist);
            // Rotate toward the side the preferred direction already leans to;
            // exact head-on picks the left (counter-clockwise) side.
            let sign = if lateral > 0.0 { -1.0 } else { 1.0 };
            let (s, c) = (half * sign).sin_cos();
            let tangent = V2::new(base.x * c - base.z * s, base.x * s + base.z * c);
            preferred = tangent.scale(speed);
            break;
        }
        let inv_horizon = 1.0 / self.params.time_horizon;
        let inv_dt = 1.0 / dt;
        for &(_, _, j) in &neighbors[..count] {
            let other = &self.bodies[j as usize];
            lines.push(agent_line(
                v,
                V2::new(other.x - me.x, other.z - me.z),
                v - V2::new(other.vx, other.vz),
                me.radius + other.radius,
                inv_horizon,
                inv_dt,
                if other.obstacle { 1.0 } else { 0.5 },
            ));
        }

        let (mut chosen, fallback) = solve(&lines, hard, agent.max_speed, preferred);
        let mut out = Out {
            fallback,
            ..Out::default()
        };
        // Integrate, clipping at any wall the move would cross.
        let mut q = p + chosen.scale(dt);
        if let Some(walls) = walls {
            for &index in &wall_ids[..wall_count] {
                let s = &walls.segments[index as usize];
                if me.y < s.y_min - me.height || me.y > s.y_max + me.height {
                    continue;
                }
                if let Some(t) = crossing(p, q, s.a, s.b) {
                    q = p + (q - p).scale((t - 1e-3).max(0.0));
                    out.clipped = true;
                }
            }
            if out.clipped {
                chosen = (q - p).scale(inv_dt);
            }
        }
        let horizontal = (agent.preferred[0] * agent.preferred[0]
            + agent.preferred[2] * agent.preferred[2])
            .sqrt();
        let vy = if horizontal > 1e-6 {
            agent.preferred[1] * chosen.len_sq().sqrt() / horizontal
        } else {
            0.0
        };
        out.position = [q.x, me.y + vy * dt, q.z];
        out.velocity = [chosen.x, vy, chosen.z];
        out
    }
}

/// Keeps the `limit` smallest keys in ascending order.
fn insert<const N: usize>(list: &mut [Key; N], len: &mut usize, limit: usize, item: Key) {
    let limit = limit.min(N);
    let less = |a: &Key, b: &Key| a.0 < b.0 || (a.0 == b.0 && a.1 < b.1);
    if *len == limit {
        if limit == 0 || !less(&item, &list[limit - 1]) {
            return;
        }
        *len -= 1;
    }
    let mut k = *len;
    while k > 0 && less(&item, &list[k - 1]) {
        list[k] = list[k - 1];
        k -= 1;
    }
    list[k] = item;
    *len += 1;
}
