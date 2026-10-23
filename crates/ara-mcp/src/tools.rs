//! The twelve tools (R03 §4.5) and how their schemas are generated from the method
//! catalog: a tool backed by one method takes that method's params schema; a tool
//! backed by several gets an `action` enum plus the union of their params.

use ara_proto::MethodInfo;
use serde_json::{Map, Value, json};

/// One MCP tool.
pub struct ToolSpec {
    /// Tool name.
    pub name: &'static str,
    /// Display title.
    pub title: &'static str,
    /// What the model reads.
    pub description: &'static str,
    /// The method family called while no service backs the tool (the host answers
    /// `ARA1806`).
    pub reserved: &'static str,
}

/// The tools, in their fixed order (deterministic `tools/list`).
pub const TOOLS: [ToolSpec; 12] = [
    ToolSpec {
        name: "ara_query",
        title: "Query entities",
        description: "Find entities (or documents) without dumping scenes. Compact syntax in `q`: space-separated terms that must all hold: `Light` (has the component), `!Light`, `name:Crate*`, a bare word (`radio`: names containing it), `parent:e_1c0d`, `under:e_1c0d`, `prefab:crate`, `depth<=1`, comparisons such as `Light.intensity>500` or `Transform.position.y<=0`. Or the JSON form in `filter`. Paginated (`limit`, `cursor`); returns entity handles (`e_8b21`; `e_4f9a/e_m4tz` inside prefab instances) with one scene-to-text line each. `fields` returns chosen values.",
        reserved: "query",
    },
    ToolSpec {
        name: "ara_inspect",
        title: "Inspect a document",
        description: "Read a document as token-tuned text: the hierarchy summary (default; `depth`, paginated with `cursor`), or with `entities` every component field with its unit (defaults filled in). `format: \"json\"` for structured output. Y-up, meters, degrees.",
        reserved: "inspect",
    },
    ToolSpec {
        name: "ara_schema",
        title: "Search the schema",
        description: "The tool-search layer: find and describe components, value types, enums, commands, queries, jobs and protocol methods. `search` by words, `list` by kind, `describe` for JSON Schemas, units and examples. Look a command up here before using it in ara_apply.",
        reserved: "schema",
    },
    ToolSpec {
        name: "ara_apply",
        title: "Apply commands",
        description: "Change documents. Runs `commands` (each `{cmd, args, as?}`; the commands and their args are listed under `cmd`) in order as one atomic transaction on `doc` (default: the document holding the entities you name, else the main scene), recorded with your `why` (required) for human review; any error rolls everything back and returns located diagnostics with machine-applicable fixes. Entities: IDs (`e_8b21`, `e_4f9a/e_m4tz` inside a prefab instance), `$name` symbols minted by `as` in an earlier call, or `name:Crate` / `name:Floor/Crate`. The result lists every created, modified and deleted entity with its ID. `dryRun: true` returns the exact diff without applying. Each call is one turn with a checkpoint you can roll back to (ara_session).",
        reserved: "cmd",
    },
    ToolSpec {
        name: "ara_run",
        title: "Run code",
        description: "Run C# against the typed engine SDK inside a transaction (code mode). Edits still become commands, so they stay reviewable and revertible.",
        reserved: "run",
    },
    ToolSpec {
        name: "ara_capture",
        title: "Capture images",
        description: "Render images of a scene: camera specs (frame entities, angles, existing cameras), debug view modes, numbered marks with a pixel-to-entity legend, deterministic settling and golden-image comparison.",
        reserved: "capture",
    },
    ToolSpec {
        name: "ara_play",
        title: "Control play",
        description: "Control play worlds: start, stop, pause, step N ticks with action-level input, watch expressions, `until` conditions, record and replay.",
        reserved: "play",
    },
    ToolSpec {
        name: "ara_test",
        title: "Run tests",
        description: "Run unit, play and golden tests; structured results with captures and replay handles for failures.",
        reserved: "test",
    },
    ToolSpec {
        name: "ara_logs",
        title: "Read diagnostics",
        description: "Diagnostics from every subsystem (stable ARA#### codes, locations, machine-applicable fixes), de-duplicated with counts. Pass the returned `cursor` as `since` to see only what is new; filter by `codes`, `severity` or `entity`.",
        reserved: "logs",
    },
    ToolSpec {
        name: "ara_session",
        title: "Session, history and review",
        description: "Sessions, history and review. `action: \"begin\"` with your `client` name returns the `session` handle the other tools take, the scopes granted and the project's documents (over `ara mcp` a session also begins by itself on first use). Then: `status`, `diff` (what you changed, was → now), `checkpoint`, `rollback` (undo back to a checkpoint), `history`, `undo`, `redo`, `review` (unreviewed changes), `revert` (per transaction or entity, also another agent's), `end`.",
        reserved: "session",
    },
    ToolSpec {
        name: "ara_docs",
        title: "Search the docs",
        description: "Search this engine version's generated docs: conventions (axes, units, IDs, references), commands, components and error codes. The same pages as llms.txt.",
        reserved: "docs",
    },
    ToolSpec {
        name: "ara_jobs",
        title: "Jobs",
        description: "Long-running work (import, bake, build, generate): `start` a job on a document, then `status` (progress, result), `list` or `cancel`.",
        reserved: "jobs",
    },
];

const UNAVAILABLE: &str = " Not available in this engine build yet: calls return diagnostic ARA1806 until a service provides it.";

/// The methods backing `tool`, in catalog order.
pub fn methods_of<'a>(catalog: &'a [MethodInfo], tool: &str) -> Vec<&'a MethodInfo> {
    catalog
        .iter()
        .filter(|m| m.tool.as_ref().is_some_and(|t| t.tool == tool))
        .collect()
}

/// A params schema as a tool input schema: no `$schema`/`$id`/`title`, no root
/// description (the tool has one), optional fields simplified.
fn clean(schema: &Value) -> Map<String, Value> {
    let mut v = schema.clone();
    simplify(&mut v);
    let mut s = v.as_object().cloned().unwrap_or_default();
    for key in ["$schema", "$id", "title", "description"] {
        s.shift_remove(key);
    }
    s
}

/// Rewrites `{"anyOf": [X, {"type": "null"}], "default": null}` (an optional field)
/// as plain `X`: optional already means "may be absent", and it saves tokens.
fn simplify(v: &mut Value) {
    match v {
        Value::Object(o) => {
            let null = json!({ "type": "null" });
            let other = match o.get("anyOf") {
                Some(Value::Array(any)) if any.len() == 2 && any.contains(&null) => {
                    any.iter().find(|x| **x != null).cloned()
                }
                _ => None,
            };
            if let Some(Value::Object(other)) = other {
                o.shift_remove("anyOf");
                if o.get("default") == Some(&Value::Null) {
                    o.shift_remove("default");
                }
                for (k, val) in other {
                    o.entry(k).or_insert(val);
                }
            }
            o.values_mut().for_each(simplify);
        }
        Value::Array(items) => items.iter_mut().for_each(simplify),
        _ => {}
    }
}

fn without_description(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.shift_remove("description");
    }
    v
}

/// One input schema for several methods: an `action` enum and the union of their
/// properties (each noting the actions that use it when not all do).
fn merged(methods: &[&MethodInfo]) -> Map<String, Value> {
    let action_of = |m: &MethodInfo| {
        m.tool
            .as_ref()
            .and_then(|t| t.action.clone())
            .unwrap_or_else(|| m.name.clone())
    };
    let actions: Vec<String> = methods.iter().map(|m| action_of(m)).collect();
    let mut props: Vec<(String, Vec<String>, Vec<Value>)> = Vec::new();
    let mut defs = Map::new();
    let mut required_by_all: Option<Vec<String>> = None;
    for m in methods {
        let action = action_of(m);
        let schema = clean(&m.params);
        if let Some(Value::Object(d)) = schema.get("$defs") {
            defs.extend(d.clone());
        }
        let required: Vec<String> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        required_by_all = Some(match required_by_all {
            None => required,
            Some(prev) => prev.into_iter().filter(|r| required.contains(r)).collect(),
        });
        if let Some(Value::Object(p)) = schema.get("properties") {
            for (name, s) in p {
                match props.iter_mut().find(|(n, _, _)| n == name) {
                    Some((_, used, schemas)) => {
                        used.push(action.clone());
                        schemas.push(s.clone());
                    }
                    None => props.push((name.clone(), vec![action.clone()], vec![s.clone()])),
                }
            }
        }
    }
    let summary: Vec<String> = methods
        .iter()
        .map(|m| format!("`{}`: {}", action_of(m), m.summary))
        .collect();
    let mut properties = Map::new();
    properties.insert(
        "action".into(),
        json!({ "type": "string", "enum": actions, "description": format!("What to do. {}", summary.join(" ")) }),
    );
    for (name, used, schemas) in props {
        let mut descriptions: Vec<&str> = Vec::new();
        for s in &schemas {
            if let Some(d) = s.get("description").and_then(Value::as_str)
                && !descriptions.contains(&d)
            {
                descriptions.push(d);
            }
        }
        let mut shapes: Vec<Value> = Vec::new();
        for s in &schemas {
            let shape = without_description(s);
            if !shapes.contains(&shape) {
                shapes.push(shape);
            }
        }
        let mut out = if shapes.len() == 1 {
            shapes.remove(0)
        } else {
            json!({ "anyOf": shapes })
        };
        let mut description = descriptions.join(" / ");
        if used.len() < actions.len() {
            let unused: Vec<&String> = actions.iter().filter(|a| !used.contains(a)).collect();
            if unused.len() < used.len() {
                let list: Vec<&str> = unused.iter().map(|a| a.as_str()).collect();
                description.push_str(&format!(" (not for: {})", list.join(", ")));
            } else {
                description.push_str(&format!(" (for: {})", used.join(", ")));
            }
        }
        if let Some(o) = out.as_object_mut() {
            o.insert("description".into(), description.into());
        }
        properties.insert(name, out);
    }
    let mut required = vec![Value::from("action")];
    required.extend(
        required_by_all
            .unwrap_or_default()
            .into_iter()
            .map(Value::from),
    );
    let mut s = Map::new();
    s.insert("type".into(), "object".into());
    s.insert("properties".into(), Value::Object(properties));
    s.insert("required".into(), Value::Array(required));
    if !defs.is_empty() {
        s.insert("$defs".into(), Value::Object(defs));
    }
    s
}

const OPTIONAL_SESSION: &str = "Session handle from ara_session `begin`. Optional: without it, calls use this connection's session (begun on first use).";

/// Makes `session` optional in a tool's input schema (per-connection adapters).
fn session_optional(schema: &mut Value) {
    if let Some(req) = schema.get_mut("required").and_then(Value::as_array_mut) {
        req.retain(|r| r != "session");
    }
    if let Some(p) = schema.pointer_mut("/properties/session") {
        p["description"] = OPTIONAL_SESSION.into();
    }
}

/// The tool definitions for `tools/list`, generated from the live catalog. With
/// `optional_session` (one client per adapter), `session` is not required.
pub fn definitions(catalog: &[MethodInfo], optional_session: bool) -> Vec<Value> {
    TOOLS
        .iter()
        .map(|spec| {
            let methods = methods_of(catalog, spec.name);
            let single = methods.len() == 1
                && methods[0].tool.as_ref().is_some_and(|t| t.action.is_none());
            let (mut schema, description) = match methods.as_slice() {
                [] => (
                    json!({
                        "type": "object",
                        "properties": { "session": { "type": "string", "description": "Session handle from ara_session `begin`." } },
                        "required": ["session"],
                    }),
                    format!("{}{UNAVAILABLE}", spec.description),
                ),
                [m] if single => (Value::Object(clean(&m.params)), spec.description.to_owned()),
                many => (Value::Object(merged(many)), spec.description.to_owned()),
            };
            if optional_session {
                session_optional(&mut schema);
            }
            let all = |f: fn(&MethodInfo) -> bool| !methods.is_empty() && methods.iter().all(|m| f(m));
            json!({
                "name": spec.name,
                "title": spec.title,
                "description": description,
                "inputSchema": schema,
                "annotations": {
                    "title": spec.title,
                    "readOnlyHint": all(|m| m.hints.read_only),
                    "destructiveHint": methods.iter().any(|m| m.hints.destructive),
                    "idempotentHint": all(|m| m.hints.idempotent),
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}
