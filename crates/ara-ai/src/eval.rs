//! Expression evaluation and EQS queries for one brain. Everything an expression
//! can read is passed in explicitly, so evaluation is deterministic.
use crate::ast::BinOp;
use crate::program::{Builtin, ExprId, Generator, Node, Program, Query, Value};
use ara_nav::{NavAgentState, NavMesh, NavQuery, NavStatus, QueryFilter};
use ara_perception::{LineOfSight, Perception, Sense};

/// Value returned for unknown distances/durations.
pub const FAR: f32 = 1.0e9;
/// Eye height used by `visible(a, b)`.
pub const EYE: f32 = 1.5;

/// Inputs to one brain's evaluation this tick.
pub(crate) struct Env<'a, 'm> {
    pub program: &'a Program,
    pub dt: f32,
    pub me: u64,
    pub position: [f32; 3],
    pub perception: Option<&'a Perception>,
    pub nav: Option<&'a NavAgentState>,
    pub poses: &'a [(u64, [f32; 3])],
    pub los: &'a dyn LineOfSight,
    pub mesh: Option<&'m NavMesh>,
    pub query: &'a mut Option<NavQuery<'m>>,
    pub rng: &'a mut u64,
    /// Seconds in the state whose expression is being evaluated.
    pub time: f32,
    /// Current EQS candidate.
    pub item: Option<[f32; 3]>,
}

/// One scored EQS candidate (debugger stream and overlays).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EqsItem {
    /// Candidate position (snapped when a `navigable` filter projected it).
    pub point: [f32; 3],
    /// Weighted score (0 when filtered out).
    pub score: f32,
    /// Whether every filter passed.
    pub passed: bool,
}

/// SplitMix64 step: deterministic, state lives in `BrainState`.
pub(crate) fn next_random(state: &mut u64) -> f32 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((z ^ (z >> 31)) >> 40) as f32 / 16_777_216.0
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl<'m> Env<'_, 'm> {
    /// Position of an entity (true pose) or a vec3; None for unset values.
    pub(crate) fn position_of(&self, v: &Value) -> Option<[f32; 3]> {
        match v {
            Value::Vec3(p) => Some(*p),
            Value::Entity(id) if *id == self.me => Some(self.position),
            Value::Entity(id) => self
                .poses
                .binary_search_by_key(id, |(i, _)| *i)
                .ok()
                .map(|i| self.poses[i].1),
            _ => None,
        }
    }

    fn query(&mut self) -> Option<&mut NavQuery<'m>> {
        if self.query.is_none() {
            *self.query = self.mesh.and_then(|m| m.query().ok());
        }
        self.query.as_mut()
    }

    /// Nearest walkable point within 1 m horizontally.
    pub(crate) fn project(&mut self, p: [f32; 3]) -> Option<[f32; 3]> {
        let near = self.query()?.nearest(p, &QueryFilter::default()).ok()??;
        let h = ((near.position[0] - p[0]).powi(2) + (near.position[2] - p[2]).powi(2)).sqrt();
        (h <= 1.0).then_some(near.position)
    }

    fn path_distance(&mut self, a: [f32; 3], b: [f32; 3]) -> f32 {
        let Some(q) = self.query() else {
            return FAR;
        };
        match q.path(a, b, &QueryFilter::default()) {
            Ok(path) if path.complete => path
                .corners
                .windows(2)
                .map(|w| dist(w[0].position, w[1].position))
                .sum(),
            _ => FAR,
        }
    }

    /// Evaluates an expression against the blackboard.
    pub(crate) fn eval(&mut self, id: ExprId, vars: &[Value]) -> Value {
        let program = self.program;
        match &program.exprs[id as usize] {
            Node::Const(v) => v.clone(),
            Node::Str(hash, _) => Value::Float(*hash as f32),
            Node::Var(slot) => vars.get(*slot as usize).cloned().unwrap_or(Value::None),
            Node::SelfRef => Value::Entity(self.me),
            Node::Time => Value::Float(self.time),
            Node::Item => self.item.map_or(Value::None, Value::Vec3),
            Node::Vec3(parts) => Value::Vec3(parts.map(|p| self.eval(p, vars).float())),
            Node::List(items) => Value::Points(
                items
                    .iter()
                    .filter_map(|i| match self.eval(*i, vars) {
                        Value::Vec3(p) => Some(p),
                        _ => None,
                    })
                    .collect(),
            ),
            Node::Not(x) => Value::Bool(!self.eval(*x, vars).truthy()),
            Node::Neg(x) => match self.eval(*x, vars) {
                Value::Float(f) => Value::Float(-f),
                Value::Vec3(p) => Value::Vec3(p.map(|v| -v)),
                _ => Value::None,
            },
            Node::Bin(op, a, b) => {
                // Short-circuit logic keeps expensive checks (rays) off when possible.
                match op {
                    BinOp::And => {
                        return Value::Bool(
                            self.eval(*a, vars).truthy() && self.eval(*b, vars).truthy(),
                        );
                    }
                    BinOp::Or => {
                        return Value::Bool(
                            self.eval(*a, vars).truthy() || self.eval(*b, vars).truthy(),
                        );
                    }
                    _ => {}
                }
                let (x, y) = (self.eval(*a, vars), self.eval(*b, vars));
                binary(*op, x, y)
            }
            Node::Call(builtin, args) => self.call(*builtin, args, vars),
        }
    }

    fn call(&mut self, builtin: Builtin, args: &[ExprId], vars: &[Value]) -> Value {
        let arg = |env: &mut Self, i: usize| env.eval(args[i], vars);
        let entity = |v: &Value| match v {
            Value::Entity(id) => Some(*id),
            _ => None,
        };
        match builtin {
            Builtin::Sees => {
                let e = arg(self, 0);
                Value::Bool(
                    entity(&e)
                        .and_then(|id| self.perception?.get(id))
                        .is_some_and(|k| k.visible),
                )
            }
            Builtin::Heard => Value::Bool(
                self.perception
                    .is_some_and(|p| p.known.iter().any(|k| k.heard)),
            ),
            Builtin::Noise => self
                .perception
                .and_then(|p| {
                    p.known
                        .iter()
                        .filter(|k| k.sense == Sense::Hearing)
                        .min_by_key(|k| (k.since, k.id))
                })
                .map_or(Value::None, |k| Value::Vec3(k.position)),
            Builtin::Nearest => {
                let kind = match &self.program.exprs[args[0] as usize] {
                    Node::Str(hash, _) => *hash,
                    _ => return Value::None,
                };
                let me = self.position;
                self.perception
                    .and_then(|p| {
                        p.known
                            .iter()
                            .filter(|k| k.kind == kind && k.id != 0)
                            .min_by(|a, b| {
                                dist(a.position, me)
                                    .total_cmp(&dist(b.position, me))
                                    .then(a.id.cmp(&b.id))
                            })
                    })
                    .map_or(Value::None, |k| Value::Entity(k.id))
            }
            Builtin::Known => {
                let e = arg(self, 0);
                Value::Bool(entity(&e).and_then(|id| self.perception?.get(id)).is_some())
            }
            Builtin::Lost => {
                let e = arg(self, 0);
                Value::Float(
                    entity(&e)
                        .and_then(|id| self.perception?.get(id))
                        .map_or(FAR, |k| k.since as f32 * self.dt),
                )
            }
            Builtin::LastSeen => {
                let e = arg(self, 0);
                entity(&e)
                    .and_then(|id| self.perception?.get(id))
                    .map_or(Value::None, |k| Value::Vec3(k.position))
            }
            Builtin::Position => {
                let v = arg(self, 0);
                self.position_of(&v).map_or(Value::None, Value::Vec3)
            }
            Builtin::Distance => {
                let (a, b) = (arg(self, 0), arg(self, 1));
                match (self.position_of(&a), self.position_of(&b)) {
                    (Some(a), Some(b)) => Value::Float(dist(a, b)),
                    _ => Value::Float(FAR),
                }
            }
            Builtin::Visible => {
                let (a, b) = (arg(self, 0), arg(self, 1));
                let ids = (entity(&a).unwrap_or(0), entity(&b).unwrap_or(0));
                match (self.position_of(&a), self.position_of(&b)) {
                    (Some(a), Some(b)) => Value::Bool(self.los.clear(
                        [a[0], a[1] + EYE, a[2]],
                        [b[0], b[1] + EYE, b[2]],
                        ids.0,
                        ids.1,
                    )),
                    _ => Value::Bool(false),
                }
            }
            Builtin::Navigable => {
                let snap_item = matches!(self.program.exprs[args[0] as usize], Node::Item);
                let v = arg(self, 0);
                match self.position_of(&v).and_then(|p| self.project(p)) {
                    Some(p) => {
                        if snap_item {
                            self.item = Some(p);
                        }
                        Value::Bool(true)
                    }
                    None => Value::Bool(false),
                }
            }
            Builtin::PathDistance => {
                let (a, b) = (arg(self, 0), arg(self, 1));
                match (self.position_of(&a), self.position_of(&b)) {
                    (Some(a), Some(b)) => Value::Float(self.path_distance(a, b)),
                    _ => Value::Float(FAR),
                }
            }
            Builtin::Arrived => {
                Value::Bool(self.nav.is_some_and(|n| n.status == NavStatus::Arrived))
            }
            Builtin::Random => Value::Float(next_random(self.rng)),
            Builtin::IsSet => Value::Bool(arg(self, 0).is_set()),
            Builtin::Min => Value::Float(arg(self, 0).float().min(arg(self, 1).float())),
            Builtin::Max => Value::Float(arg(self, 0).float().max(arg(self, 1).float())),
            Builtin::Clamp => {
                let (x, lo, hi) = (
                    arg(self, 0).float(),
                    arg(self, 1).float(),
                    arg(self, 2).float(),
                );
                Value::Float(x.max(lo).min(hi))
            }
            Builtin::Abs => Value::Float(arg(self, 0).float().abs()),
            Builtin::Remap => {
                let (x, a, b) = (
                    arg(self, 0).float(),
                    arg(self, 1).float(),
                    arg(self, 2).float(),
                );
                Value::Float(if b == a {
                    f32::from(u8::from(x >= b))
                } else {
                    ((x - a) / (b - a)).clamp(0.0, 1.0)
                })
            }
            Builtin::Inverse => {
                let (x, m) = (arg(self, 0).float(), arg(self, 1).float());
                Value::Float(if m > 0.0 {
                    1.0 - (x / m).clamp(0.0, 1.0)
                } else {
                    0.0
                })
            }
        }
    }

    /// Runs a query: generate, filter (in order), score; best item and the trace.
    pub(crate) fn run_query(
        &mut self,
        q: &Query,
        vars: &[Value],
    ) -> (Option<[f32; 3]>, Vec<EqsItem>) {
        let mut items = Vec::new();
        for g in &q.generators {
            self.generate(g, vars, &mut items);
        }
        items.truncate(256);
        let mut trace = Vec::with_capacity(items.len());
        let mut best: Option<([f32; 3], f32)> = None;
        for p in items {
            self.item = Some(p);
            let passed = q.filters.iter().all(|f| self.eval(*f, vars).truthy());
            let point = self.item.unwrap_or(p);
            let score = if passed {
                q.scores
                    .iter()
                    .map(|(s, w)| self.eval(*s, vars).float().clamp(0.0, 1.0) * w)
                    .sum()
            } else {
                0.0
            };
            if passed && best.is_none_or(|(_, b)| score > b) {
                best = Some((point, score));
            }
            trace.push(EqsItem {
                point,
                score,
                passed,
            });
        }
        self.item = None;
        (best.map(|(p, _)| p), trace)
    }

    fn generate(&mut self, g: &Generator, vars: &[Value], out: &mut Vec<[f32; 3]>) {
        match g {
            Generator::Ring(center, radius, count) => {
                let c = self.eval(*center, vars);
                let Some(c) = self.position_of(&c) else {
                    return;
                };
                let r = self.eval(*radius, vars).float();
                let n = (self.eval(*count, vars).float() as usize).clamp(1, 64);
                for k in 0..n {
                    let a = std::f32::consts::TAU * k as f32 / n as f32;
                    out.push([c[0] + r * a.cos(), c[1], c[2] + r * a.sin()]);
                }
            }
            Generator::Grid(center, half, spacing) => {
                let c = self.eval(*center, vars);
                let Some(c) = self.position_of(&c) else {
                    return;
                };
                let h = self.eval(*half, vars).float().max(0.0);
                let s = self.eval(*spacing, vars).float().max(0.25);
                let n = ((2.0 * h / s).floor() as i32).clamp(0, 31);
                for z in 0..=n {
                    for x in 0..=n {
                        out.push([c[0] - h + x as f32 * s, c[1], c[2] - h + z as f32 * s]);
                    }
                }
            }
            Generator::Cover(center, radius) => {
                let c = self.eval(*center, vars);
                let Some(c) = self.position_of(&c) else {
                    return;
                };
                let r = self.eval(*radius, vars).float().max(0.0);
                let Some(mesh) = self.mesh else { return };
                let mut walls = Vec::new();
                mesh.walls().within(c, r, &mut walls);
                for w in walls {
                    let (dx, dz) = (w.b[0] - w.a[0], w.b[2] - w.a[2]);
                    let len = (dx * dx + dz * dz).sqrt();
                    if len < 0.25 {
                        continue;
                    }
                    let (nx, nz) = (-dz / len * 0.6, dx / len * 0.6);
                    let samples = ((len / 2.0).floor() as usize).clamp(1, 8);
                    for k in 0..samples {
                        let t = (k as f32 + 0.5) / samples as f32;
                        let p = [w.a[0] + dx * t, (w.a[1] + w.b[1]) * 0.5, w.a[2] + dz * t];
                        out.push([p[0] + nx, p[1], p[2] + nz]);
                        out.push([p[0] - nx, p[1], p[2] - nz]);
                    }
                }
            }
        }
    }
}

fn binary(op: BinOp, x: Value, y: Value) -> Value {
    use Value::*;
    match op {
        BinOp::Eq => Bool(x == y),
        BinOp::Ne => Bool(x != y),
        BinOp::Lt => Bool(x.float() < y.float()),
        BinOp::Le => Bool(x.float() <= y.float()),
        BinOp::Gt => Bool(x.float() > y.float()),
        BinOp::Ge => Bool(x.float() >= y.float()),
        BinOp::Add | BinOp::Sub => {
            let s = if op == BinOp::Add { 1.0 } else { -1.0 };
            match (x, y) {
                (Float(a), Float(b)) => Float(a + s * b),
                (Vec3(a), Vec3(b)) => Vec3([a[0] + s * b[0], a[1] + s * b[1], a[2] + s * b[2]]),
                _ => None,
            }
        }
        BinOp::Mul => match (x, y) {
            (Float(a), Float(b)) => Float(a * b),
            (Vec3(a), Float(b)) | (Float(b), Vec3(a)) => Vec3(a.map(|v| v * b)),
            _ => None,
        },
        BinOp::Div => match (x, y) {
            (Float(a), Float(b)) => Float(if b == 0.0 { 0.0 } else { a / b }),
            (Vec3(a), Float(b)) => Vec3(a.map(|v| if b == 0.0 { 0.0 } else { v / b })),
            _ => None,
        },
        BinOp::And | BinOp::Or => Bool(false), // Handled with short-circuit.
    }
}
