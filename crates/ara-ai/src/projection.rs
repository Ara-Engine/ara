//! Behaviour <-> `ara-graph` document (domain `behavior`). One node per variable,
//! query and state, keyed by stable IDs; expressions are stored as canonical DSL
//! text and transitions point at target node IDs, so renames keep identity and
//! canvas layout. Edges are not used: the hierarchy and transitions are payload.
use crate::ast::*;
use crate::{codes, dsl};
use ara_diag::Diagnostic;
use ara_doc::AssetId;
use ara_graph::{Editor, Graph, Node};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;

/// Graph header domain for behaviours.
pub const DOMAIN: &str = "behavior";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Header {
    name: String,
    vars: Vec<String>,
    queries: Vec<String>,
    roots: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VarData {
    name: String,
    ty: Type,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryData {
    name: String,
    generate: Vec<String>,
    filter: Vec<String>,
    score: Vec<(String, f32)>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransitionData {
    /// `done`, `failed` or a condition expression.
    trigger: String,
    /// Target state node ID.
    target: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StateData {
    name: String,
    children: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    when: Option<String>,
    select: Select,
    enter: Vec<String>,
    exit: Vec<String>,
    tasks: Vec<String>,
    scores: Vec<String>,
    transitions: Vec<TransitionData>,
}

fn value<T: Serialize>(v: &T) -> Result<Json, Diagnostic> {
    ara_json::to_value(v).map_err(|e| Diagnostic::error(codes::SYNTAX, e.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(v: Json, record: &str) -> Result<T, Diagnostic> {
    serde_json::from_value(v)
        .map_err(|e| Diagnostic::error(codes::SYNTAX, e.to_string()).record(record))
}

/// Text of one action / call, reusing the canonical printer through a tiny
/// behaviour so the stored form is exactly what the DSL prints.
fn action_text(a: &Action) -> String {
    match a {
        Action::Set(n, e) => format!("set {n} = {}", dsl::print_expr(e)),
        Action::Clear(n) => format!("clear {n}"),
        Action::Emit(n) => format!("emit {n}"),
    }
}
fn call_text(c: &Call) -> String {
    let args: Vec<String> = c
        .args
        .iter()
        .map(|a| match &a.name {
            Some(n) => format!("{n}: {}", dsl::print_expr(&a.value)),
            None => dsl::print_expr(&a.value),
        })
        .collect();
    format!("{}({})", c.name, args.join(", "))
}

/// Projects a behaviour into a graph, keeping `editor` (canvas layout).
pub fn to_graph(b: &Behavior, editor: Editor) -> Result<Graph, Diagnostic> {
    let id = AssetId::parse(&b.id).map_err(|e| Diagnostic::error(codes::SYNTAX, e))?;
    let mut g = Graph::new(id, DOMAIN);
    g.editor = editor;
    g.header.payload = value(&Header {
        name: b.name.clone(),
        vars: b.vars.iter().map(|v| v.id.clone()).collect(),
        queries: b.queries.iter().map(|q| q.id.clone()).collect(),
        roots: b.states.iter().map(|s| s.id.clone()).collect(),
    })?;
    let node = |kind: &str, payload: Json| Node {
        kind: kind.into(),
        version: 1,
        ports: BTreeMap::new(),
        params: BTreeMap::new(),
        payload,
        subgraph: None,
    };
    for v in &b.vars {
        let data = VarData {
            name: v.name.clone(),
            ty: v.ty,
            default: v.default.as_ref().map(dsl::print_expr),
        };
        g.nodes
            .insert(v.id.clone(), node("behavior.var", value(&data)?));
    }
    for q in &b.queries {
        let data = QueryData {
            name: q.name.clone(),
            generate: q.generators.iter().map(call_text).collect(),
            filter: q.filters.iter().map(dsl::print_expr).collect(),
            score: q
                .scores
                .iter()
                .map(|(e, w)| (dsl::print_expr(e), *w))
                .collect(),
        };
        g.nodes
            .insert(q.id.clone(), node("behavior.query", value(&data)?));
    }
    let ids: BTreeMap<&str, &str> = b
        .all_states()
        .into_iter()
        .map(|s| (s.name.as_str(), s.id.as_str()))
        .collect();
    for s in b.all_states() {
        let data = StateData {
            name: s.name.clone(),
            children: s.children.iter().map(|c| c.id.clone()).collect(),
            when: s.when.as_ref().map(dsl::print_expr),
            select: s.select,
            enter: s.enter.iter().map(action_text).collect(),
            exit: s.exit.iter().map(action_text).collect(),
            tasks: s.tasks.iter().map(call_text).collect(),
            scores: s.scores.iter().map(dsl::print_expr).collect(),
            transitions: s
                .transitions
                .iter()
                .map(|t| {
                    let target = ids.get(t.target.as_str()).map_or_else(
                        // Unknown names are kept as `?Name` so checking can report them.
                        || format!("?{}", t.target),
                        |id| (*id).to_owned(),
                    );
                    let trigger = match &t.trigger {
                        Trigger::Done => "done".to_owned(),
                        Trigger::Failed => "failed".to_owned(),
                        Trigger::When(e) => dsl::print_expr(e),
                    };
                    TransitionData { trigger, target }
                })
                .collect(),
        };
        g.nodes
            .insert(s.id.clone(), node("behavior.state", value(&data)?));
    }
    Ok(g)
}

/// Rebuilds the behaviour stored in a graph.
pub fn from_graph(g: &Graph) -> Result<Behavior, Diagnostic> {
    if g.header.domain != DOMAIN {
        return Err(Diagnostic::error(
            codes::SYNTAX,
            format!("expected a `{DOMAIN}` graph, found `{}`", g.header.domain),
        ));
    }
    let header: Header = decode(g.header.payload.clone(), "g_meta")?;
    let payload = |id: &str| -> Result<Json, Diagnostic> {
        g.nodes.get(id).map(|n| n.payload.clone()).ok_or_else(|| {
            Diagnostic::error(codes::NAME, format!("graph references missing node `{id}`"))
                .record(id)
        })
    };
    let expr = |text: &str, record: &str| dsl::parse_expr(text).map_err(|d| d.record(record));
    let mut vars = Vec::new();
    for id in &header.vars {
        let d: VarData = decode(payload(id)?, id)?;
        vars.push(Var {
            id: id.clone(),
            name: d.name,
            ty: d.ty,
            default: d.default.as_deref().map(|t| expr(t, id)).transpose()?,
        });
    }
    let mut queries = Vec::new();
    for id in &header.queries {
        let d: QueryData = decode(payload(id)?, id)?;
        queries.push(QueryDef {
            id: id.clone(),
            name: d.name,
            generators: d
                .generate
                .iter()
                .map(|t| call(t, id))
                .collect::<Result<_, _>>()?,
            filters: d
                .filter
                .iter()
                .map(|t| expr(t, id))
                .collect::<Result<_, _>>()?,
            scores: d
                .score
                .iter()
                .map(|(t, w)| expr(t, id).map(|e| (e, *w)))
                .collect::<Result<_, _>>()?,
        });
    }
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for (id, n) in &g.nodes {
        if n.kind == "behavior.state"
            && let Some(name) = n.payload.get("name").and_then(Json::as_str)
        {
            names.insert(id.clone(), name.to_owned());
        }
    }
    fn state(
        g: &Graph,
        id: &str,
        names: &BTreeMap<String, String>,
        depth: usize,
    ) -> Result<State, Diagnostic> {
        if depth > 64 {
            return Err(Diagnostic::error(
                codes::NAME,
                "state hierarchy is cyclic or deeper than 64",
            )
            .record(id));
        }
        let n = g.nodes.get(id).ok_or_else(|| {
            Diagnostic::error(codes::NAME, format!("missing state node `{id}`")).record(id)
        })?;
        let d: StateData = decode(n.payload.clone(), id)?;
        let expr = |t: &str| dsl::parse_expr(t).map_err(|e| e.record(id));
        Ok(State {
            id: id.to_owned(),
            name: d.name,
            when: d.when.as_deref().map(expr).transpose()?,
            select: d.select,
            enter: d
                .enter
                .iter()
                .map(|t| action(t, id))
                .collect::<Result<_, _>>()?,
            exit: d
                .exit
                .iter()
                .map(|t| action(t, id))
                .collect::<Result<_, _>>()?,
            tasks: d
                .tasks
                .iter()
                .map(|t| call(t, id))
                .collect::<Result<_, _>>()?,
            scores: d.scores.iter().map(|t| expr(t)).collect::<Result<_, _>>()?,
            transitions: d
                .transitions
                .iter()
                .map(|t| {
                    Ok(Transition {
                        trigger: match t.trigger.as_str() {
                            "done" => Trigger::Done,
                            "failed" => Trigger::Failed,
                            other => Trigger::When(expr(other)?),
                        },
                        target: names.get(&t.target).cloned().unwrap_or_else(|| {
                            t.target.strip_prefix('?').unwrap_or(&t.target).to_owned()
                        }),
                    })
                })
                .collect::<Result<_, Diagnostic>>()?,
            children: d
                .children
                .iter()
                .map(|c| state(g, c, names, depth + 1))
                .collect::<Result<_, _>>()?,
        })
    }
    let states = header
        .roots
        .iter()
        .map(|id| state(g, id, &names, 0))
        .collect::<Result<_, _>>()?;
    Ok(Behavior {
        id: g.id.to_string(),
        name: header.name,
        vars,
        queries,
        states,
    })
}

/// Parses one stored call through a probe behaviour (shares the DSL grammar).
fn call(text: &str, record: &str) -> Result<Call, Diagnostic> {
    let probe = format!("behavior P {{ state S {{ do {text}; }} }}");
    let b = dsl::parse(&probe, "gr_probe").map_err(|d| d.record(record))?;
    b.states
        .into_iter()
        .next()
        .and_then(|s| s.tasks.into_iter().next())
        .ok_or_else(|| {
            Diagnostic::error(codes::SYNTAX, format!("invalid call `{text}`")).record(record)
        })
}

fn action(text: &str, record: &str) -> Result<Action, Diagnostic> {
    let probe = format!("behavior P {{ state S {{ enter {text}; }} }}");
    let b = dsl::parse(&probe, "gr_probe").map_err(|d| d.record(record))?;
    b.states
        .into_iter()
        .next()
        .and_then(|s| s.enter.into_iter().next())
        .ok_or_else(|| {
            Diagnostic::error(codes::SYNTAX, format!("invalid action `{text}`")).record(record)
        })
}

/// JSON summary for tools: node IDs by authored name.
pub fn identities(b: &Behavior) -> Json {
    json!({
        "vars": b.vars.iter().map(|v| (v.name.clone(), v.id.clone())).collect::<BTreeMap<_, _>>(),
        "queries": b.queries.iter().map(|q| (q.name.clone(), q.id.clone())).collect::<BTreeMap<_, _>>(),
        "states": b.all_states().iter().map(|s| (s.name.clone(), s.id.clone())).collect::<BTreeMap<_, _>>(),
    })
}
