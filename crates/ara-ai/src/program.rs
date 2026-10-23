//! Checking and compilation: a [`Behavior`] becomes a flat, immutable [`Program`]
//! (state table, expression arena, typed tasks). Errors are diagnostics with
//! source-level names, never IDs.
use crate::ast::*;
use crate::codes;
use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Runtime value of a blackboard slot or expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Value {
    /// Unset entity/vec3/points.
    None,
    /// Boolean.
    Bool(bool),
    /// Float.
    Float(f32),
    /// Position or direction.
    Vec3([f32; 3]),
    /// Stable entity ID.
    Entity(u64),
    /// Ordered positions.
    Points(Vec<[f32; 3]>),
}
impl Value {
    /// The empty value of a type (`clear`).
    pub fn empty(ty: Type) -> Self {
        match ty {
            Type::Bool => Self::Bool(false),
            Type::Float => Self::Float(0.0),
            Type::Vec3 | Type::Entity => Self::None,
            Type::Points => Self::Points(Vec::new()),
        }
    }
    /// Truthiness for conditions (only booleans are true).
    pub fn truthy(&self) -> bool {
        matches!(self, Self::Bool(true))
    }
    /// Float value, or 0.
    pub fn float(&self) -> f32 {
        match self {
            Self::Float(f) => *f,
            Self::Bool(b) => f32::from(u8::from(*b)),
            _ => 0.0,
        }
    }
    /// Whether the value is set (non-none, non-empty).
    pub fn is_set(&self) -> bool {
        match self {
            Self::None => false,
            Self::Points(p) => !p.is_empty(),
            _ => true,
        }
    }
}

/// Static type of an expression during checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    T(Type),
    Str,
    /// `none`: assignable to vec3, entity and points.
    Nothing,
}
impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::T(t) => t.name(),
            Ty::Str => "string",
            Ty::Nothing => "none",
        }
    }
    fn fits(self, want: Type) -> bool {
        match self {
            Ty::T(t) => t == want,
            Ty::Nothing => matches!(want, Type::Vec3 | Type::Entity | Type::Points),
            Ty::Str => false,
        }
    }
    fn positional(self) -> bool {
        matches!(self, Ty::T(Type::Vec3 | Type::Entity) | Ty::Nothing)
    }
}

/// Built-in functions available in expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Builtin {
    /// `sees(e)`: e is visible this tick.
    Sees,
    /// `heard()`: a noise was heard within the last second.
    Heard,
    /// `noise()`: position of the most recently heard noise.
    Noise,
    /// `nearest("kind")`: nearest remembered stimulus of a kind.
    Nearest,
    /// `known(e)`: e is in memory.
    Known,
    /// `lost(e)`: seconds since e was last sensed (1e9 when unknown).
    Lost,
    /// `lastSeen(e)`: remembered position of e.
    LastSeen,
    /// `position(e)`: current position of e (vec3 passes through).
    Position,
    /// `distance(a, b)`.
    Distance,
    /// `visible(a, b)`: line of sight between two positions (eye height added).
    Visible,
    /// `navigable(p)`: p projects onto the agent's navmesh.
    Navigable,
    /// `pathDistance(a, b)`: navmesh path length (1e9 when unreachable).
    PathDistance,
    /// `arrived()`: navigation reports arrival.
    Arrived,
    /// `random()`: deterministic per-brain uniform [0, 1).
    Random,
    /// `isSet(x)`: x is not none/empty.
    IsSet,
    /// `min(a, b)`.
    Min,
    /// `max(a, b)`.
    Max,
    /// `clamp(x, lo, hi)`.
    Clamp,
    /// `abs(x)`.
    Abs,
    /// `remap(x, a, b)`: maps a..b to 0..1, clamped.
    Remap,
    /// `inverse(x, max)`: 1 - clamp(x / max).
    Inverse,
    /// `direction(a, b)`: unit XZ vector from a to b (zero when coincident).
    Direction,
    /// `dot(a, b)`: dot product of two vec3.
    Dot,
}

struct Signature {
    name: &'static str,
    builtin: Builtin,
    args: &'static [Arg],
    ret: Type,
}
#[derive(Clone, Copy, PartialEq)]
enum Arg {
    Float,
    Vec3,
    Pos,
    Entity,
    Str,
    Any,
}

const BUILTINS: &[Signature] = &[
    Signature {
        name: "sees",
        builtin: Builtin::Sees,
        args: &[Arg::Entity],
        ret: Type::Bool,
    },
    Signature {
        name: "heard",
        builtin: Builtin::Heard,
        args: &[],
        ret: Type::Bool,
    },
    Signature {
        name: "noise",
        builtin: Builtin::Noise,
        args: &[],
        ret: Type::Vec3,
    },
    Signature {
        name: "nearest",
        builtin: Builtin::Nearest,
        args: &[Arg::Str],
        ret: Type::Entity,
    },
    Signature {
        name: "known",
        builtin: Builtin::Known,
        args: &[Arg::Entity],
        ret: Type::Bool,
    },
    Signature {
        name: "lost",
        builtin: Builtin::Lost,
        args: &[Arg::Entity],
        ret: Type::Float,
    },
    Signature {
        name: "lastSeen",
        builtin: Builtin::LastSeen,
        args: &[Arg::Entity],
        ret: Type::Vec3,
    },
    Signature {
        name: "position",
        builtin: Builtin::Position,
        args: &[Arg::Pos],
        ret: Type::Vec3,
    },
    Signature {
        name: "distance",
        builtin: Builtin::Distance,
        args: &[Arg::Pos, Arg::Pos],
        ret: Type::Float,
    },
    Signature {
        name: "visible",
        builtin: Builtin::Visible,
        args: &[Arg::Pos, Arg::Pos],
        ret: Type::Bool,
    },
    Signature {
        name: "navigable",
        builtin: Builtin::Navigable,
        args: &[Arg::Pos],
        ret: Type::Bool,
    },
    Signature {
        name: "pathDistance",
        builtin: Builtin::PathDistance,
        args: &[Arg::Pos, Arg::Pos],
        ret: Type::Float,
    },
    Signature {
        name: "arrived",
        builtin: Builtin::Arrived,
        args: &[],
        ret: Type::Bool,
    },
    Signature {
        name: "random",
        builtin: Builtin::Random,
        args: &[],
        ret: Type::Float,
    },
    Signature {
        name: "isSet",
        builtin: Builtin::IsSet,
        args: &[Arg::Any],
        ret: Type::Bool,
    },
    Signature {
        name: "min",
        builtin: Builtin::Min,
        args: &[Arg::Float, Arg::Float],
        ret: Type::Float,
    },
    Signature {
        name: "max",
        builtin: Builtin::Max,
        args: &[Arg::Float, Arg::Float],
        ret: Type::Float,
    },
    Signature {
        name: "clamp",
        builtin: Builtin::Clamp,
        args: &[Arg::Float, Arg::Float, Arg::Float],
        ret: Type::Float,
    },
    Signature {
        name: "abs",
        builtin: Builtin::Abs,
        args: &[Arg::Float],
        ret: Type::Float,
    },
    Signature {
        name: "remap",
        builtin: Builtin::Remap,
        args: &[Arg::Float, Arg::Float, Arg::Float],
        ret: Type::Float,
    },
    Signature {
        name: "inverse",
        builtin: Builtin::Inverse,
        args: &[Arg::Float, Arg::Float],
        ret: Type::Float,
    },
    Signature {
        name: "direction",
        builtin: Builtin::Direction,
        args: &[Arg::Pos, Arg::Pos],
        ret: Type::Vec3,
    },
    Signature {
        name: "dot",
        builtin: Builtin::Dot,
        args: &[Arg::Vec3, Arg::Vec3],
        ret: Type::Float,
    },
];

/// Names of every built-in function (for docs and did-you-mean).
pub fn builtin_names() -> Vec<&'static str> {
    BUILTINS.iter().map(|s| s.name).collect()
}

/// Index into [`Program::exprs`].
pub type ExprId = u32;

/// Compiled expression node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    /// Constant value.
    Const(Value),
    /// String literal, hashed (kinds) and kept for action names.
    Str(u32, String),
    /// Blackboard slot.
    Var(u16),
    /// This agent.
    SelfRef,
    /// Seconds in the current state.
    Time,
    /// EQS candidate.
    Item,
    /// `vec3(x, y, z)`.
    Vec3([ExprId; 3]),
    /// `[p, ...]`.
    List(Vec<ExprId>),
    /// Built-in call.
    Call(Builtin, Vec<ExprId>),
    /// `not x`.
    Not(ExprId),
    /// `-x`.
    Neg(ExprId),
    /// Binary operator.
    Bin(BinOp, ExprId, ExprId),
}

/// Compiled entry/exit action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CAction {
    /// Assign a slot.
    Set(u16, ExprId),
    /// Reset a slot to its type's empty value.
    Clear(u16),
    /// Emit a named event.
    Emit(String),
}

/// Compiled task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Task {
    /// Walk to a position or entity; finishes on arrival, fails when unreachable.
    MoveTo {
        /// Destination (vec3 or entity).
        target: ExprId,
        /// Speed limit (m/s), if given.
        speed: Option<ExprId>,
        /// Arrival tolerance in metres.
        tolerance: Option<ExprId>,
    },
    /// Loop over a points route; never finishes.
    Patrol {
        /// Route (points).
        route: ExprId,
        /// Speed limit, if given.
        speed: Option<ExprId>,
    },
    /// Finish after a number of seconds.
    Wait {
        /// Seconds.
        seconds: ExprId,
    },
    /// Keep facing a position or entity; never finishes.
    Face {
        /// Look target.
        target: ExprId,
    },
    /// Gameplay action intent (`fire`, `reload`); repeats with `every`, else
    /// finishes after `duration` seconds.
    Act {
        /// Action name.
        name: String,
        /// Optional target.
        target: Option<ExprId>,
        /// Repeat period in seconds.
        every: Option<ExprId>,
        /// Duration in seconds when not repeating.
        duration: Option<ExprId>,
    },
    /// Run a query once; finishes when a point was found (written to `into`).
    Find {
        /// Query index.
        query: u16,
        /// Destination slot (vec3).
        into: u16,
    },
    /// Wander to random navigable points around a centre; never finishes.
    Wander {
        /// Radius in metres.
        radius: ExprId,
        /// Centre (defaults to the position when the task started).
        center: Option<ExprId>,
        /// Speed limit, if given.
        speed: Option<ExprId>,
    },
}
impl Task {
    /// Whether the task can finish (only finishing tasks count for `on done`).
    pub fn finishes(&self) -> bool {
        match self {
            Task::MoveTo { .. } | Task::Wait { .. } | Task::Find { .. } => true,
            Task::Act { every, .. } => every.is_none(),
            Task::Patrol { .. } | Task::Face { .. } | Task::Wander { .. } => false,
        }
    }
    /// DSL name.
    pub fn name(&self) -> &'static str {
        match self {
            Task::MoveTo { .. } => "moveTo",
            Task::Patrol { .. } => "patrol",
            Task::Wait { .. } => "wait",
            Task::Face { .. } => "face",
            Task::Act { .. } => "act",
            Task::Find { .. } => "find",
            Task::Wander { .. } => "wander",
        }
    }
}

/// Compiled transition trigger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CTrigger {
    /// All finishing tasks succeeded.
    Done,
    /// A task failed.
    Failed,
    /// Condition.
    When(ExprId),
}

/// One state in the flat table (parents precede children).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateNode {
    /// Authored name.
    pub name: String,
    /// Stable graph ID.
    pub id: String,
    /// Parent state.
    pub parent: Option<u16>,
    /// Children in order.
    pub children: Vec<u16>,
    /// Entry condition.
    pub when: Option<ExprId>,
    /// Child selection.
    pub select: Select,
    /// Entry actions.
    pub enter: Vec<CAction>,
    /// Exit actions.
    pub exit: Vec<CAction>,
    /// Tasks.
    pub tasks: Vec<Task>,
    /// Utility considerations.
    pub scores: Vec<ExprId>,
    /// Transitions (trigger, target state index).
    pub transitions: Vec<(CTrigger, u16)>,
    /// Authored text of each transition (`on lost(target) > 3 -> Search`), for diagnostics.
    pub transition_text: Vec<String>,
}

/// Compiled generator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Generator {
    /// `ring(center, radius, count)`.
    Ring(ExprId, ExprId, ExprId),
    /// `grid(center, halfSize, spacing)`.
    Grid(ExprId, ExprId, ExprId),
    /// `cover(center, radius)`: points just off nearby navmesh walls.
    Cover(ExprId, ExprId),
}

/// Compiled query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    /// Authored name.
    pub name: String,
    /// Generators.
    pub generators: Vec<Generator>,
    /// Filters.
    pub filters: Vec<ExprId>,
    /// Weighted scores.
    pub scores: Vec<(ExprId, f32)>,
}

/// Blackboard slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    /// Authored name.
    pub name: String,
    /// Type.
    pub ty: Type,
    /// Default expression (evaluated when a brain starts).
    pub default: Option<ExprId>,
}

/// An immutable compiled behaviour, shared by every brain that runs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    /// Behaviour name.
    pub name: String,
    /// Semantic hash of the source AST (snapshots record it).
    pub hash: String,
    /// Blackboard slots.
    pub slots: Vec<Slot>,
    /// States, parents before children.
    pub states: Vec<StateNode>,
    /// Top-level states.
    pub roots: Vec<u16>,
    /// Queries.
    pub queries: Vec<Query>,
    /// Expression arena.
    pub exprs: Vec<Node>,
}
impl Program {
    /// State index by name.
    pub fn state(&self, name: &str) -> Option<u16> {
        self.states
            .iter()
            .position(|s| s.name == name)
            .map(|i| i as u16)
    }
    /// Slot index by name.
    pub fn slot(&self, name: &str) -> Option<u16> {
        self.slots
            .iter()
            .position(|s| s.name == name)
            .map(|i| i as u16)
    }
    /// Root-to-state chain of indices.
    pub fn path(&self, state: u16) -> Vec<u16> {
        let mut out = vec![state];
        let mut at = state;
        while let Some(p) = self.states[at as usize].parent {
            out.push(p);
            at = p;
        }
        out.reverse();
        out
    }
}

struct Compiler<'a> {
    b: &'a Behavior,
    slots: BTreeMap<&'a str, (u16, Type)>,
    states: BTreeMap<&'a str, u16>,
    queries: BTreeMap<&'a str, u16>,
    exprs: Vec<Node>,
    errors: Vec<Diagnostic>,
    in_query: bool,
}

/// Checks and compiles a behaviour. All errors are returned together.
pub fn compile(b: &Behavior) -> Result<Program, Vec<Diagnostic>> {
    let mut c = Compiler {
        b,
        slots: BTreeMap::new(),
        states: BTreeMap::new(),
        queries: BTreeMap::new(),
        exprs: Vec::new(),
        errors: Vec::new(),
        in_query: false,
    };
    c.run().ok_or(c.errors)
}

impl<'a> Compiler<'a> {
    fn err(&mut self, code: ara_diag::Code, msg: String, record: &str) {
        self.errors
            .push(Diagnostic::error(code, msg).record(record));
    }

    fn run(&mut self) -> Option<Program> {
        let b = self.b;
        let mut names = BTreeMap::new();
        for (i, v) in b.vars.iter().enumerate() {
            if self.slots.insert(&v.name, (i as u16, v.ty)).is_some() {
                self.err(
                    codes::NAME,
                    format!("variable `{}` is declared twice", v.name),
                    &v.id,
                );
            }
        }
        let flat = b.all_states();
        for (i, s) in flat.iter().enumerate() {
            if self.states.insert(&s.name, i as u16).is_some() {
                self.err(
                    codes::NAME,
                    format!("state `{}` is declared twice", s.name),
                    &s.id,
                );
            }
            names.insert(s.name.as_str(), s.id.as_str());
        }
        for (i, q) in b.queries.iter().enumerate() {
            if self.queries.insert(&q.name, i as u16).is_some() {
                self.err(
                    codes::NAME,
                    format!("query `{}` is declared twice", q.name),
                    &q.id,
                );
            }
        }
        if b.states.is_empty() {
            self.err(
                codes::NAME,
                "a behaviour needs at least one state".into(),
                &b.id,
            );
        }
        let mut slots = Vec::new();
        for v in &b.vars {
            let default = v.default.as_ref().map(|d| {
                let (id, ty) = self.expr(d, &v.id);
                if !ty.fits(v.ty) {
                    self.err(
                        codes::TYPE,
                        format!(
                            "`{}` is {} but its default is {}",
                            v.name,
                            v.ty.name(),
                            ty.name()
                        ),
                        &v.id,
                    );
                }
                id
            });
            slots.push(Slot {
                name: v.name.clone(),
                ty: v.ty,
                default,
            });
        }
        let mut queries = Vec::new();
        self.in_query = true;
        for q in &b.queries {
            let mut generators = Vec::new();
            for g in &q.generators {
                if let Some(generator) = self.generator(g, &q.id) {
                    generators.push(generator);
                }
            }
            let filters = q
                .filters
                .iter()
                .map(|f| self.condition(f, &q.id, "filter"))
                .collect();
            let scores = q
                .scores
                .iter()
                .map(|(s, w)| (self.number(s, &q.id, "score"), *w))
                .collect();
            queries.push(Query {
                name: q.name.clone(),
                generators,
                filters,
                scores,
            });
        }
        self.in_query = false;
        let mut states = Vec::new();
        let mut roots = Vec::new();
        fn add<'s>(
            c: &mut Compiler<'s>,
            s: &'s State,
            parent: Option<u16>,
            out: &mut Vec<StateNode>,
        ) -> u16 {
            let index = out.len() as u16;
            out.push(StateNode {
                name: s.name.clone(),
                id: s.id.clone(),
                parent,
                children: Vec::new(),
                when: None,
                select: s.select,
                enter: Vec::new(),
                exit: Vec::new(),
                tasks: Vec::new(),
                scores: Vec::new(),
                transitions: Vec::new(),
                transition_text: Vec::new(),
            });
            let when = s.when.as_ref().map(|w| c.condition(w, &s.id, "when"));
            let enter = s.enter.iter().filter_map(|a| c.action(a, &s.id)).collect();
            let exit = s.exit.iter().filter_map(|a| c.action(a, &s.id)).collect();
            let tasks = s.tasks.iter().filter_map(|t| c.task(t, &s.id)).collect();
            let scores = s
                .scores
                .iter()
                .map(|e| c.number(e, &s.id, "score"))
                .collect();
            let mut transitions = Vec::new();
            let mut transition_text = Vec::new();
            for t in &s.transitions {
                let trigger = match &t.trigger {
                    Trigger::Done => CTrigger::Done,
                    Trigger::Failed => CTrigger::Failed,
                    Trigger::When(e) => CTrigger::When(c.condition(e, &s.id, "transition")),
                };
                let Some(&target) = c.states.get(t.target.as_str()) else {
                    let names: Vec<&str> = c.states.keys().copied().collect();
                    c.err(
                        codes::TARGET,
                        format!(
                            "transition from `{}` targets unknown state `{}`{}",
                            s.name,
                            t.target,
                            ara_diag::did_you_mean(&t.target, names.iter().copied())
                                .map(|n| format!("; did you mean `{n}`?"))
                                .unwrap_or_default()
                        ),
                        &s.id,
                    );
                    continue;
                };
                transitions.push((trigger, target));
                transition_text.push(format!(
                    "on {} -> {}",
                    match &t.trigger {
                        Trigger::Done => "done".to_owned(),
                        Trigger::Failed => "failed".to_owned(),
                        Trigger::When(e) => crate::dsl::print_expr(e),
                    },
                    t.target
                ));
            }
            let node = &mut out[index as usize];
            node.when = when;
            node.enter = enter;
            node.exit = exit;
            node.tasks = tasks;
            node.scores = scores;
            node.transitions = transitions;
            node.transition_text = transition_text;
            for child in &s.children {
                let ci = add(c, child, Some(index), out);
                out[index as usize].children.push(ci);
            }
            index
        }
        for s in &b.states {
            let i = add(self, s, None, &mut states);
            roots.push(i);
        }
        if !self.errors.is_empty() {
            return None;
        }
        let hash = blake3::hash(
            serde_json::to_string(&(b.name.as_str(), &b.vars, &b.queries, &b.states))
                .unwrap_or_default()
                .as_bytes(),
        )
        .to_hex()
        .to_string();
        Some(Program {
            name: b.name.clone(),
            hash,
            slots,
            states,
            roots,
            queries,
            exprs: std::mem::take(&mut self.exprs),
        })
    }

    fn push(&mut self, node: Node) -> ExprId {
        self.exprs.push(node);
        (self.exprs.len() - 1) as ExprId
    }

    fn condition(&mut self, e: &Expr, record: &str, what: &str) -> ExprId {
        let (id, ty) = self.expr(e, record);
        if ty != Ty::T(Type::Bool) {
            self.err(
                codes::TYPE,
                format!(
                    "{what} `{}` must be bool, found {}",
                    crate::dsl::print_expr(e),
                    ty.name()
                ),
                record,
            );
        }
        id
    }

    fn number(&mut self, e: &Expr, record: &str, what: &str) -> ExprId {
        let (id, ty) = self.expr(e, record);
        if ty != Ty::T(Type::Float) {
            self.err(
                codes::TYPE,
                format!(
                    "{what} `{}` must be float, found {}",
                    crate::dsl::print_expr(e),
                    ty.name()
                ),
                record,
            );
        }
        id
    }

    fn position(&mut self, e: &Expr, record: &str, what: &str) -> ExprId {
        let (id, ty) = self.expr(e, record);
        if !ty.positional() {
            self.err(
                codes::TYPE,
                format!(
                    "{what} `{}` must be vec3 or entity, found {}",
                    crate::dsl::print_expr(e),
                    ty.name()
                ),
                record,
            );
        }
        id
    }

    fn slot(&mut self, name: &str, record: &str) -> Option<(u16, Type)> {
        let found = self.slots.get(name).copied();
        if found.is_none() {
            let names: Vec<&str> = self.slots.keys().copied().collect();
            self.err(
                codes::NAME,
                format!(
                    "unknown variable `{name}`{}",
                    ara_diag::did_you_mean(name, names.iter().copied())
                        .map(|n| format!("; did you mean `{n}`?"))
                        .unwrap_or_default()
                ),
                record,
            );
        }
        found
    }

    fn action(&mut self, a: &Action, record: &str) -> Option<CAction> {
        match a {
            Action::Set(name, e) => {
                let (slot, ty) = self.slot(name, record)?;
                let (id, got) = self.expr(e, record);
                if !got.fits(ty) {
                    self.err(
                        codes::TYPE,
                        format!("cannot set {} `{name}` to {}", ty.name(), got.name()),
                        record,
                    );
                }
                Some(CAction::Set(slot, id))
            }
            Action::Clear(name) => Some(CAction::Clear(self.slot(name, record)?.0)),
            Action::Emit(name) => Some(CAction::Emit(name.clone())),
        }
    }

    fn args<'c>(
        &mut self,
        call: &'c Call,
        names: &[&str],
        required: usize,
        record: &str,
    ) -> Option<Vec<Option<&'c Expr>>> {
        let mut out: Vec<Option<&Expr>> = vec![None; names.len()];
        let mut positional = 0;
        for arg in &call.args {
            let index = match &arg.name {
                Some(n) => names.iter().position(|x| x == n),
                None => {
                    positional += 1;
                    Some(positional - 1).filter(|i| *i < names.len())
                }
            };
            match index {
                Some(i) if out[i].is_none() => out[i] = Some(&arg.value),
                _ => {
                    self.err(
                        codes::TARGET,
                        format!(
                            "`{}` takes ({}); unexpected argument {}",
                            call.name,
                            names.join(", "),
                            arg.name.as_deref().unwrap_or("(positional)")
                        ),
                        record,
                    );
                    return None;
                }
            }
        }
        if out[..required].iter().any(Option::is_none) {
            self.err(
                codes::TARGET,
                format!(
                    "`{}` requires ({})",
                    call.name,
                    names[..required].join(", ")
                ),
                record,
            );
            return None;
        }
        Some(out)
    }

    fn task(&mut self, call: &Call, record: &str) -> Option<Task> {
        Some(match call.name.as_str() {
            "moveTo" => {
                let a = self.args(call, &["target", "speed", "tolerance"], 1, record)?;
                Task::MoveTo {
                    target: self.position(a[0]?, record, "moveTo target"),
                    speed: a[1].map(|e| self.number(e, record, "speed")),
                    tolerance: a[2].map(|e| self.number(e, record, "tolerance")),
                }
            }
            "patrol" => {
                let a = self.args(call, &["route", "speed"], 1, record)?;
                let (route, ty) = self.expr(a[0]?, record);
                if ty != Ty::T(Type::Points) {
                    self.err(
                        codes::TYPE,
                        format!("patrol route must be points, found {}", ty.name()),
                        record,
                    );
                }
                Task::Patrol {
                    route,
                    speed: a[1].map(|e| self.number(e, record, "speed")),
                }
            }
            "wait" => {
                let a = self.args(call, &["seconds"], 1, record)?;
                Task::Wait {
                    seconds: self.number(a[0]?, record, "seconds"),
                }
            }
            "face" => {
                let a = self.args(call, &["target"], 1, record)?;
                Task::Face {
                    target: self.position(a[0]?, record, "face target"),
                }
            }
            "act" => {
                let a = self.args(call, &["name", "target", "every", "duration"], 1, record)?;
                let Some(Expr::Str(name)) = a[0] else {
                    self.err(
                        codes::TYPE,
                        "act name must be a string literal".into(),
                        record,
                    );
                    return None;
                };
                Task::Act {
                    name: name.clone(),
                    target: a[1].map(|e| self.position(e, record, "act target")),
                    every: a[2].map(|e| self.number(e, record, "every")),
                    duration: a[3].map(|e| self.number(e, record, "duration")),
                }
            }
            "find" => {
                let a = self.args(call, &["query", "into"], 2, record)?;
                let (Some(Expr::Var(query)), Some(Expr::Var(into))) = (a[0], a[1]) else {
                    self.err(
                        codes::TYPE,
                        "find takes a query name and a vec3 variable".into(),
                        record,
                    );
                    return None;
                };
                let Some(&q) = self.queries.get(query.as_str()) else {
                    self.err(codes::NAME, format!("unknown query `{query}`"), record);
                    return None;
                };
                let (slot, ty) = self.slot(into, record)?;
                if ty != Type::Vec3 {
                    self.err(
                        codes::TYPE,
                        format!("find writes vec3, but `{into}` is {}", ty.name()),
                        record,
                    );
                }
                Task::Find {
                    query: q,
                    into: slot,
                }
            }
            "wander" => {
                let a = self.args(call, &["radius", "center", "speed"], 1, record)?;
                Task::Wander {
                    radius: self.number(a[0]?, record, "radius"),
                    center: a[1].map(|e| self.position(e, record, "center")),
                    speed: a[2].map(|e| self.number(e, record, "speed")),
                }
            }
            other => {
                self.err(
                    codes::TARGET,
                    format!(
                        "unknown task `{other}`; tasks are moveTo, patrol, wait, face, act, find, wander"
                    ),
                    record,
                );
                return None;
            }
        })
    }

    fn generator(&mut self, call: &Call, record: &str) -> Option<Generator> {
        Some(match call.name.as_str() {
            "ring" => {
                let a = self.args(call, &["center", "radius", "count"], 3, record)?;
                Generator::Ring(
                    self.position(a[0]?, record, "ring center"),
                    self.number(a[1]?, record, "radius"),
                    self.number(a[2]?, record, "count"),
                )
            }
            "grid" => {
                let a = self.args(call, &["center", "half", "spacing"], 3, record)?;
                Generator::Grid(
                    self.position(a[0]?, record, "grid center"),
                    self.number(a[1]?, record, "half"),
                    self.number(a[2]?, record, "spacing"),
                )
            }
            "cover" => {
                let a = self.args(call, &["center", "radius"], 2, record)?;
                Generator::Cover(
                    self.position(a[0]?, record, "cover center"),
                    self.number(a[1]?, record, "radius"),
                )
            }
            other => {
                self.err(
                    codes::TARGET,
                    format!("unknown generator `{other}`; use ring, grid or cover"),
                    record,
                );
                return None;
            }
        })
    }

    fn expr(&mut self, e: &Expr, record: &str) -> (ExprId, Ty) {
        match e {
            Expr::Num(n) => (
                self.push(Node::Const(Value::Float(*n as f32))),
                Ty::T(Type::Float),
            ),
            Expr::Bool(b) => (self.push(Node::Const(Value::Bool(*b))), Ty::T(Type::Bool)),
            Expr::Str(s) => (
                self.push(Node::Str(ara_perception::kind_hash(s), s.clone())),
                Ty::Str,
            ),
            Expr::None => (self.push(Node::Const(Value::None)), Ty::Nothing),
            Expr::SelfRef => (self.push(Node::SelfRef), Ty::T(Type::Entity)),
            Expr::Time => (self.push(Node::Time), Ty::T(Type::Float)),
            Expr::Item => {
                if !self.in_query {
                    self.err(
                        codes::NAME,
                        "`item` is only defined inside queries".into(),
                        record,
                    );
                }
                (self.push(Node::Item), Ty::T(Type::Vec3))
            }
            Expr::Var(name) => match self.slot(name, record) {
                Some((slot, ty)) => (self.push(Node::Var(slot)), Ty::T(ty)),
                None => (self.push(Node::Const(Value::None)), Ty::Nothing),
            },
            Expr::Vec3(parts) => {
                let ids = [0, 1, 2].map(|i| self.number(&parts[i], record, "vec3 component"));
                (self.push(Node::Vec3(ids)), Ty::T(Type::Vec3))
            }
            Expr::List(items) => {
                let mut ids = Vec::new();
                for item in items {
                    let (id, ty) = self.expr(item, record);
                    if ty != Ty::T(Type::Vec3) {
                        self.err(
                            codes::TYPE,
                            format!("points entries must be vec3, found {}", ty.name()),
                            record,
                        );
                    }
                    ids.push(id);
                }
                (self.push(Node::List(ids)), Ty::T(Type::Points))
            }
            Expr::Call(name, args) => {
                let Some(sig) = BUILTINS.iter().find(|s| s.name == name) else {
                    self.err(
                        codes::NAME,
                        format!(
                            "unknown function `{name}`{}",
                            ara_diag::did_you_mean(name, builtin_names())
                                .map(|n| format!("; did you mean `{n}`?"))
                                .unwrap_or_default()
                        ),
                        record,
                    );
                    return (self.push(Node::Const(Value::None)), Ty::Nothing);
                };
                if args.len() != sig.args.len() {
                    self.err(
                        codes::TYPE,
                        format!(
                            "`{name}` takes {} arguments, found {}",
                            sig.args.len(),
                            args.len()
                        ),
                        record,
                    );
                }
                let mut ids = Vec::new();
                for (arg, want) in args.iter().zip(sig.args) {
                    let (id, ty) = self.expr(arg, record);
                    let ok = match want {
                        Arg::Float => ty == Ty::T(Type::Float),
                        Arg::Vec3 => ty == Ty::T(Type::Vec3),
                        Arg::Pos => ty.positional(),
                        Arg::Entity => ty.fits(Type::Entity),
                        Arg::Str => ty == Ty::Str,
                        Arg::Any => ty != Ty::Str,
                    };
                    if !ok {
                        self.err(
                            codes::TYPE,
                            format!(
                                "argument `{}` of `{name}` has type {}",
                                crate::dsl::print_expr(arg),
                                ty.name()
                            ),
                            record,
                        );
                    }
                    ids.push(id);
                }
                (self.push(Node::Call(sig.builtin, ids)), Ty::T(sig.ret))
            }
            Expr::Unary(UnOp::Not, inner) => {
                let id = self.condition(inner, record, "operand of not");
                (self.push(Node::Not(id)), Ty::T(Type::Bool))
            }
            Expr::Unary(UnOp::Neg, inner) => {
                let (id, ty) = self.expr(inner, record);
                if !matches!(ty, Ty::T(Type::Float | Type::Vec3)) {
                    self.err(codes::TYPE, format!("cannot negate {}", ty.name()), record);
                }
                (self.push(Node::Neg(id)), ty)
            }
            Expr::Binary(op, l, r) => {
                let (a, ta) = self.expr(l, record);
                let (b, tb) = self.expr(r, record);
                use Type::*;
                let ty = match op {
                    BinOp::And | BinOp::Or => {
                        (ta == Ty::T(Bool) && tb == Ty::T(Bool)).then_some(Ty::T(Bool))
                    }
                    BinOp::Eq | BinOp::Ne => {
                        (ta == tb || ta == Ty::Nothing || tb == Ty::Nothing).then_some(Ty::T(Bool))
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        (ta == Ty::T(Float) && tb == Ty::T(Float)).then_some(Ty::T(Bool))
                    }
                    BinOp::Add | BinOp::Sub => match (ta, tb) {
                        (Ty::T(Float), Ty::T(Float)) => Some(Ty::T(Float)),
                        (Ty::T(Vec3), Ty::T(Vec3)) => Some(Ty::T(Vec3)),
                        _ => None,
                    },
                    BinOp::Mul | BinOp::Div => match (ta, tb) {
                        (Ty::T(Float), Ty::T(Float)) => Some(Ty::T(Float)),
                        (Ty::T(Vec3), Ty::T(Float)) => Some(Ty::T(Vec3)),
                        (Ty::T(Float), Ty::T(Vec3)) if *op == BinOp::Mul => Some(Ty::T(Vec3)),
                        _ => None,
                    },
                };
                let ty = ty.unwrap_or_else(|| {
                    self.err(
                        codes::TYPE,
                        format!(
                            "`{}` cannot combine {} and {}",
                            op.symbol(),
                            ta.name(),
                            tb.name()
                        ),
                        record,
                    );
                    Ty::Nothing
                });
                (self.push(Node::Bin(*op, a, b)), ty)
            }
        }
    }
}
