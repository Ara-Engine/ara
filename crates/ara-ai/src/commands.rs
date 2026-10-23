//! Typed behaviour commands. Every edit loads the graph, changes the AST and
//! reconciles the result into kernel ops (`apply_snapshot`): undo, review,
//! attribution and dry runs come from `ara-cmd`. Parse errors reject the edit;
//! check errors are saved and returned as diagnostics for interactive repair.
use crate::ast::*;
use crate::{codes, compile, dsl, projection};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_graph::{Graph, commands::apply_snapshot};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

/// Replaces the behaviour with DSL text, keeping IDs of surviving names.
#[derive(Schema, Deserialize)]
#[ara(params, module = "ai", name = "BehaviorTextParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextParams {
    /// Complete behaviour text.
    pub text: String,
}
/// Renames a state, variable or query, rewriting every reference.
#[derive(Schema, Deserialize)]
#[ara(params, module = "ai", name = "BehaviorRenameParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameParams {
    /// Existing name.
    pub from: String,
    /// New unique name.
    pub to: String,
    /// After a plain-text rename (delete + create): the deleted node's ID, to restore.
    pub restore_id: Option<String>,
}
/// Adds an empty state.
#[derive(Schema, Deserialize)]
#[ara(params, module = "ai", name = "BehaviorStateAddParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateAddParams {
    /// New unique state name.
    pub name: String,
    /// Parent state name; top level when absent.
    pub parent: Option<String>,
    /// Entry condition expression.
    pub when: Option<String>,
}
/// Removes a state, its children and transitions that target them.
#[derive(Schema, Deserialize)]
#[ara(params, module = "ai", name = "BehaviorStateRemoveParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateRemoveParams {
    /// State name.
    pub name: String,
}
/// Adds a transition.
#[derive(Schema, Deserialize)]
#[ara(params, module = "ai", name = "BehaviorTransitionParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransitionParams {
    /// Source state name.
    pub from: String,
    /// Target state name.
    pub to: String,
    /// `done`, `failed` or a condition expression.
    pub when: String,
}
/// Sets (or clears) a variable's default.
#[derive(Schema, Deserialize)]
#[ara(params, module = "ai", name = "BehaviorVarParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VarParams {
    /// Variable name.
    pub name: String,
    /// Default expression; absent clears it.
    pub default: Option<String>,
}

fn load(ctx: &EditCtx<'_, '_>) -> Result<(Behavior, ara_graph::Editor), Diagnostic> {
    let graph = Graph::from_document(ctx.read().doc())?;
    Ok((projection::from_graph(&graph)?, graph.editor))
}

/// Saves the AST as graph ops; returns check diagnostics (empty when valid).
fn save(
    ctx: &mut EditCtx<'_, '_>,
    b: &Behavior,
    editor: ara_graph::Editor,
    mut notes: Vec<Diagnostic>,
) -> Result<Value, Diagnostic> {
    let graph = projection::to_graph(b, editor)?;
    apply_snapshot(ctx, &graph)?;
    notes.extend(compile(b).err().unwrap_or_default());
    Ok(json!({ "diagnostics": notes, "ids": projection::identities(b) }))
}

fn missing(what: &str, name: &str) -> Diagnostic {
    Diagnostic::error(codes::NAME, format!("no {what} named `{name}`"))
}

/// Renames variable references inside an expression.
fn rename_expr(e: &mut Expr, from: &str, to: &str) {
    match e {
        Expr::Var(v) if v == from => *v = to.to_owned(),
        Expr::Vec3(parts) => parts.iter_mut().for_each(|p| rename_expr(p, from, to)),
        Expr::List(items) | Expr::Call(_, items) => {
            items.iter_mut().for_each(|p| rename_expr(p, from, to))
        }
        Expr::Unary(_, x) => rename_expr(x, from, to),
        Expr::Binary(_, a, b) => {
            rename_expr(a, from, to);
            rename_expr(b, from, to);
        }
        _ => {}
    }
}

fn rename_state(s: &mut State, from: &str, to: &str, var: bool) {
    let fix = |e: &mut Expr| {
        if var {
            rename_expr(e, from, to);
        }
    };
    if let Some(w) = &mut s.when {
        fix(w);
    }
    for a in s.enter.iter_mut().chain(s.exit.iter_mut()) {
        match a {
            Action::Set(n, e) => {
                if var && n == from {
                    *n = to.to_owned();
                }
                fix(e);
            }
            Action::Clear(n) if var && n == from => *n = to.to_owned(),
            _ => {}
        }
    }
    for c in &mut s.tasks {
        // `find(Query, into: var)` names queries and variables as identifiers.
        for a in &mut c.args {
            if let Expr::Var(n) = &mut a.value
                && n == from
            {
                *n = to.to_owned();
            } else {
                fix(&mut a.value);
            }
        }
    }
    for e in &mut s.scores {
        fix(e);
    }
    for t in &mut s.transitions {
        if let Trigger::When(e) = &mut t.trigger {
            fix(e);
        }
        if !var && t.target == from {
            t.target = to.to_owned();
        }
    }
    for c in &mut s.children {
        rename_state(c, from, to, var);
    }
}

macro_rules! command {
    ($name:ident, $params:ty, $key:literal, $doc:literal, $example:expr, |$ctx:ident, $p:ident| $body:block) => {
        #[doc = $doc]
        pub struct $name;
        impl Command for $name {
            type Params = $params;
            type Output = Value;
            const NAME: &'static str = $key;
            const SUMMARY: &'static str = $doc;
            fn examples() -> Vec<Value> {
                vec![$example]
            }
            fn run($ctx: &mut EditCtx<'_, '_>, $p: Self::Params) -> Result<Value, Diagnostic> $body
        }
    };
}

command!(
    ApplyText,
    TextParams,
    "behavior.text.apply",
    "Apply behaviour text as undoable graph ops, keeping IDs of unchanged names.",
    json!({"text": "behavior Guard { state Idle { do wait(1); } }"}),
    |ctx, p| {
        let (previous, editor) = load(ctx)?;
        let reconciled = dsl::reconcile(&p.text, &previous)?;
        save(ctx, &reconciled.behavior, editor, reconciled.diagnostics)
    }
);

command!(
    Rename,
    RenameParams,
    "behavior.rename",
    "Rename a state, variable or query, keeping its identity and rewriting references.",
    json!({"from": "Idle", "to": "Rest"}),
    |ctx, p| {
        let (mut b, editor) = load(ctx)?;
        let taken = |b: &Behavior, n: &str| {
            b.vars.iter().any(|v| v.name == n)
                || b.queries.iter().any(|q| q.name == n)
                || b.all_states().iter().any(|s| s.name == n)
        };
        if let Some(restore) = &p.restore_id {
            // Recovery after a plain-text rename: give `to` the deleted identity back.
            let restore = restore.clone();
            if let Some(v) = b.vars.iter_mut().find(|v| v.name == p.to) {
                v.id = restore;
            } else if let Some(q) = b.queries.iter_mut().find(|q| q.name == p.to) {
                q.id = restore;
            } else if let Some(s) = b.state_mut(&p.to) {
                s.id = restore;
            } else {
                return Err(missing("state, variable or query", &p.to));
            }
            return save(ctx, &b, editor, Vec::new());
        }
        if dsl::KEYWORDS.contains(&p.to.as_str()) || taken(&b, &p.to) {
            return Err(Diagnostic::error(
                codes::NAME,
                format!("`{}` is a keyword or already used", p.to),
            ));
        }
        let var = b.vars.iter().any(|v| v.name == p.from);
        let query = b.queries.iter().any(|q| q.name == p.from);
        if var {
            for v in &mut b.vars {
                if v.name == p.from {
                    v.name = p.to.clone();
                }
                if let Some(d) = &mut v.default {
                    rename_expr(d, &p.from, &p.to);
                }
            }
            for q in &mut b.queries {
                for e in q
                    .filters
                    .iter_mut()
                    .chain(q.scores.iter_mut().map(|(e, _)| e))
                {
                    rename_expr(e, &p.from, &p.to);
                }
                for g in &mut q.generators {
                    for a in &mut g.args {
                        rename_expr(&mut a.value, &p.from, &p.to);
                    }
                }
            }
        } else if query {
            for q in &mut b.queries {
                if q.name == p.from {
                    q.name = p.to.clone();
                }
            }
        } else if let Some(s) = b.state_mut(&p.from) {
            s.name = p.to.clone();
        } else {
            return Err(missing("state, variable or query", &p.from));
        }
        for s in &mut b.states {
            // Query names appear only as `find` arguments: the same rewrite as vars.
            rename_state(s, &p.from, &p.to, var || query);
        }
        save(ctx, &b, editor, Vec::new())
    }
);

command!(
    AddState,
    StateAddParams,
    "behavior.state.add",
    "Add an empty state at the top level or under a parent.",
    json!({"name": "Search", "parent": "Alert", "when": "known(target)"}),
    |ctx, p| {
        let (mut b, editor) = load(ctx)?;
        if dsl::KEYWORDS.contains(&p.name.as_str())
            || b.all_states().iter().any(|s| s.name == p.name)
        {
            return Err(Diagnostic::error(
                codes::NAME,
                format!("state `{}` already exists or is a keyword", p.name),
            ));
        }
        let when = p.when.as_deref().map(dsl::parse_expr).transpose()?;
        let state = State {
            id: dsl::fresh_id(&b.id, "state", &p.name),
            name: p.name.clone(),
            when,
            select: Select::First { every: 0.0 },
            enter: Vec::new(),
            exit: Vec::new(),
            tasks: Vec::new(),
            scores: Vec::new(),
            transitions: Vec::new(),
            children: Vec::new(),
        };
        match &p.parent {
            Some(parent) => b
                .state_mut(parent)
                .ok_or_else(|| missing("state", parent))?
                .children
                .push(state),
            None => b.states.push(state),
        }
        save(ctx, &b, editor, Vec::new())
    }
);

command!(
    RemoveState,
    StateRemoveParams,
    "behavior.state.remove",
    "Remove a state with its children and every transition into them.",
    json!({"name": "Search"}),
    |ctx, p| {
        let (mut b, editor) = load(ctx)?;
        fn take(states: &mut Vec<State>, name: &str) -> Option<State> {
            if let Some(i) = states.iter().position(|s| s.name == name) {
                return Some(states.remove(i));
            }
            states.iter_mut().find_map(|s| take(&mut s.children, name))
        }
        let removed = take(&mut b.states, &p.name).ok_or_else(|| missing("state", &p.name))?;
        let mut gone = Vec::new();
        fn names(s: &State, out: &mut Vec<String>) {
            out.push(s.name.clone());
            s.children.iter().for_each(|c| names(c, out));
        }
        names(&removed, &mut gone);
        fn prune(s: &mut State, gone: &[String]) {
            s.transitions.retain(|t| !gone.contains(&t.target));
            s.children.iter_mut().for_each(|c| prune(c, gone));
        }
        b.states.iter_mut().for_each(|s| prune(s, &gone));
        save(ctx, &b, editor, Vec::new())
    }
);

command!(
    AddTransition,
    TransitionParams,
    "behavior.transition.add",
    "Add `on <when> -> <to>` to a state.",
    json!({"from": "Patrol", "to": "Chase", "when": "sees(nearest(\"player\"))"}),
    |ctx, p| {
        let (mut b, editor) = load(ctx)?;
        let trigger = match p.when.as_str() {
            "done" => Trigger::Done,
            "failed" => Trigger::Failed,
            text => Trigger::When(dsl::parse_expr(text)?),
        };
        if !b.all_states().iter().any(|s| s.name == p.to) {
            return Err(missing("state", &p.to));
        }
        b.state_mut(&p.from)
            .ok_or_else(|| missing("state", &p.from))?
            .transitions
            .push(Transition {
                trigger,
                target: p.to.clone(),
            });
        save(ctx, &b, editor, Vec::new())
    }
);

command!(
    SetVar,
    VarParams,
    "behavior.var.set",
    "Set or clear a variable's default value expression.",
    json!({"name": "alert", "default": "0.5"}),
    |ctx, p| {
        let (mut b, editor) = load(ctx)?;
        let default = p.default.as_deref().map(dsl::parse_expr).transpose()?;
        b.vars
            .iter_mut()
            .find(|v| v.name == p.name)
            .ok_or_else(|| missing("variable", &p.name))?
            .default = default;
        save(ctx, &b, editor, Vec::new())
    }
);

/// Adds the behaviour commands and their parameter schemas to a registry.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry
        .command::<ApplyText>(db)
        .command::<Rename>(db)
        .command::<AddState>(db)
        .command::<RemoveState>(db)
        .command::<AddTransition>(db)
        .command::<SetVar>(db);
}
