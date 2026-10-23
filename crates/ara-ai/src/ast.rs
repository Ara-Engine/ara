//! The single behaviour AST: what the DSL parses to, the graph stores and the
//! compiler checks. Names are authored; IDs are stable graph identities that
//! never appear in hand-written text.
use serde::{Deserialize, Serialize};

/// A complete behaviour: blackboard variables, EQS queries and a state tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Behavior {
    /// Asset ID supplied by the file/graph, never written in text.
    pub id: String,
    /// Authored name (`behavior Guard { ... }`).
    pub name: String,
    /// Blackboard variables in declaration order.
    pub vars: Vec<Var>,
    /// Spatial queries in declaration order.
    pub queries: Vec<QueryDef>,
    /// Top-level states: children of an implicit root that selects the first
    /// whose `when` holds (no re-evaluation; write an explicit root for that).
    pub states: Vec<State>,
}

/// A typed blackboard slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Var {
    /// Stable graph ID.
    pub id: String,
    /// Unique authored name.
    pub name: String,
    /// Value type.
    pub ty: Type,
    /// Initial value; `none`/zero when absent.
    pub default: Option<Expr>,
}

/// Blackboard and expression value types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Type {
    /// true / false.
    Bool,
    /// 32-bit float.
    Float,
    /// World position or direction.
    Vec3,
    /// Stable entity ID (or none).
    Entity,
    /// Ordered list of positions (patrol routes).
    Points,
}
impl Type {
    /// DSL keyword.
    pub fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Float => "float",
            Self::Vec3 => "vec3",
            Self::Entity => "entity",
            Self::Points => "points",
        }
    }
    /// Parses a DSL keyword.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "bool" => Self::Bool,
            "float" => Self::Float,
            "vec3" => Self::Vec3,
            "entity" => Self::Entity,
            "points" => Self::Points,
            _ => return None,
        })
    }
}

/// An EQS-style query: generate candidate points, filter, score, pick the best.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryDef {
    /// Stable graph ID.
    pub id: String,
    /// Unique authored name.
    pub name: String,
    /// Generator calls (`ring`, `grid`, `cover`) in order.
    pub generators: Vec<Call>,
    /// Boolean filters over `item`, in order (cheap ones first).
    pub filters: Vec<Expr>,
    /// Scores over `item` (clamped to 0..1) with weights; the weighted sum ranks.
    pub scores: Vec<(Expr, f32)>,
}

/// How a state picks among its children when entered or re-evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "mode")]
pub enum Select {
    /// First child (in order) whose `when` holds. With `every > 0` the choice is
    /// re-evaluated periodically (a priority selector).
    First {
        /// Re-evaluation period in seconds; 0 never re-evaluates.
        every: f32,
    },
    /// Child with the highest product of its `score` lines; re-evaluated every
    /// `every` seconds with hysteresis.
    Utility {
        /// Re-evaluation period in seconds; 0 never re-evaluates.
        every: f32,
    },
    /// Uniformly random qualifying child (deterministic per-brain RNG).
    Random,
}

/// A hierarchical state with tasks, transitions, entry/exit actions and children.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Stable graph ID.
    pub id: String,
    /// Unique authored name (transition target).
    pub name: String,
    /// Entry condition used by the parent's selection.
    pub when: Option<Expr>,
    /// Child selection mode.
    pub select: Select,
    /// Actions run on entry, in order.
    pub enter: Vec<Action>,
    /// Actions run on exit, in order.
    pub exit: Vec<Action>,
    /// Tasks run every tick while active (outer states' tasks run too).
    pub tasks: Vec<Call>,
    /// Utility considerations (product of clamped values) used by the parent.
    pub scores: Vec<Expr>,
    /// Transitions checked every tick, leaf state first, in order.
    pub transitions: Vec<Transition>,
    /// Child states.
    pub children: Vec<State>,
}

/// `on <trigger> -> Target;`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// When it fires.
    pub trigger: Trigger,
    /// Target state name (unique in the behaviour).
    pub target: String,
}

/// Transition trigger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Trigger {
    /// All of the state's finishing tasks succeeded.
    Done,
    /// Any of the state's tasks failed.
    Failed,
    /// A boolean expression holds.
    When(Expr),
}

/// Entry/exit action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// `set name = expr`
    Set(String, Expr),
    /// `clear name` (reset to the type's empty value).
    Clear(String),
    /// `emit name`: a named gameplay event in the debugger stream and intents.
    Emit(String),
}

/// A task or generator call with positional and named arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    /// Callee name.
    pub name: String,
    /// Arguments in authored order.
    pub args: Vec<Arg>,
}

/// One call argument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arg {
    /// `name:` for named arguments.
    pub name: Option<String>,
    /// Value.
    pub value: Expr,
}

/// Expression tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Expr {
    /// Number literal.
    Num(f64),
    /// Boolean literal.
    Bool(bool),
    /// String literal (stimulus kinds, action names).
    Str(String),
    /// `none`: unset entity/vec3.
    None,
    /// `self`: this agent (entity).
    SelfRef,
    /// `time`: seconds in the current state.
    Time,
    /// `item`: the EQS candidate point.
    Item,
    /// Blackboard variable.
    Var(String),
    /// `vec3(x, y, z)`.
    Vec3(Box<[Expr; 3]>),
    /// `[a, b, c]` list of vec3 (points).
    List(Vec<Expr>),
    /// Built-in function call.
    Call(String, Vec<Expr>),
    /// Unary operator.
    Unary(UnOp, Box<Expr>),
    /// Binary operator.
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    /// `not`
    Not,
    /// `-`
    Neg,
}

/// Binary operators, lowest precedence first: or, and, comparisons, + -, * /.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    /// `or`
    Or,
    /// `and`
    And,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
}
impl BinOp {
    /// Operator token.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Or => "or",
            Self::And => "and",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
        }
    }
    /// Binding strength (higher binds tighter).
    pub fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge => 3,
            Self::Add | Self::Sub => 4,
            Self::Mul | Self::Div => 5,
        }
    }
}

impl Behavior {
    /// All states depth-first in authored order.
    pub fn all_states(&self) -> Vec<&State> {
        fn walk<'a>(s: &'a State, out: &mut Vec<&'a State>) {
            out.push(s);
            for c in &s.children {
                walk(c, out);
            }
        }
        let mut out = Vec::new();
        for s in &self.states {
            walk(s, &mut out);
        }
        out
    }
    /// Mutable state by name, anywhere in the tree.
    pub fn state_mut(&mut self, name: &str) -> Option<&mut State> {
        fn find<'a>(states: &'a mut [State], name: &str) -> Option<&'a mut State> {
            for s in states {
                if s.name == name {
                    return Some(s);
                }
                if let Some(found) = find(&mut s.children, name) {
                    return Some(found);
                }
            }
            None
        }
        find(&mut self.states, name)
    }
}
