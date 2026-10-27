//! Versioned reference export; serialization always uses ara-json's canonical writer.
use super::{ENGINE_VERSION, csharp_examples, generate};
use ara_cmd::Registry;
use ara_diag::catalog;
use ara_proto::{MethodInfo, PROTOCOL};
use ara_schema::SchemaDb;
use serde_json::{Value, json};

/// Versioned contract for the downloadable structured reference.
pub const FORMAT: &str = "ara.docs/1";

/// The JSON-RPC result envelope is an object. Service-specific optional fields
/// remain extensible; params-dependent payloads deliberately accept any JSON value.
fn results(method: &MethodInfo) -> Value {
    let fields: &[(&str, &str)] = match method.name.as_str() {
        "rpc.discover" => &[
            ("protocol", "string"),
            ("engine", "string"),
            ("methods", "array"),
            ("scopes", "array"),
        ],
        "session.begin" => &[
            ("session", "string"),
            ("granted", "array"),
            ("denied", "array"),
            ("documents", "array"),
        ],
        "session.status" => &[
            ("session", "object"),
            ("revision", "integer"),
            ("transactions", "integer"),
            ("files", "array"),
        ],
        "session.checkpoint" => &[
            ("id", "string"),
            ("revision", "integer"),
            ("auto", "boolean"),
        ],
        "session.rollback" => &[
            ("to", "string"),
            ("reverted", "array"),
            ("revision", "integer"),
        ],
        "session.diff" => &[
            ("changes", "array"),
            ("revision", "integer"),
            ("text", "string"),
        ],
        "session.end" => &[("session", "string"), ("ended", "boolean")],
        "history.list" | "schema.list" | "schema.search" | "review.list" => {
            &[("items", "array"), ("text", "string")]
        }
        "schema.describe" => &[("items", "array")],
        "history.undo" | "history.redo" => &[("txn", "string"), ("revision", "integer")],
        "review.accept" => &[("accepted", "array")],
        "review.revert" => &[("reverted", "array"), ("revision", "integer")],
        "run" => &[
            ("tick", "integer"),
            ("hash", "string"),
            ("logs", "array"),
            ("diagnostics", "array"),
        ],
        "capture" => &[
            ("path", "string"),
            ("width", "integer"),
            ("height", "integer"),
            ("tick", "integer"),
            ("hash", "string"),
        ],
        "query" => &[("items", "array"), ("total", "integer"), ("text", "string")],
        "inspect" => &[
            ("doc", "string"),
            ("revision", "integer"),
            ("text", "string"),
            ("entities", "array"),
        ],
        "docs.search" => &[("results", "array"), ("text", "string")],
        "cmd.apply" => &[("ok", "boolean"), ("doc", "string"), ("text", "string")],
        "logs" | "diag.query" => &[
            ("cursor", "integer"),
            ("total", "integer"),
            ("text", "string"),
            ("entries", "array"),
        ],
        "diag.subscribe" => &[("subscription", "string"), ("cursor", "integer")],
        "diag.unsubscribe" => &[("unsubscribed", "string")],
        "diag.report" => &[
            ("recorded", "integer"),
            ("skipped", "array"),
            ("cursor", "integer"),
        ],
        "diag.clear" => &[("removed", "integer"), ("cursor", "integer")],
        "why" => &[("text", "string"), ("fixes", "array")],
        "profile.start" | "profile.stop" => &[("recording", "boolean")],
        "profile.summary" | "profile.hitches" => &[
            ("version", "integer"),
            ("frames", "integer"),
            ("cpu_frame", "object"),
            ("metrics", "array"),
            ("hitches", "array"),
            ("truncated", "boolean"),
        ],
        "profile.compare" => &[
            ("passed", "boolean"),
            ("deltas", "array"),
            ("issues", "array"),
        ],
        "jobs.list" => &[("jobs", "array")],
        "jobs.start" | "jobs.status" => &[
            ("job", "string"),
            ("name", "string"),
            ("status", "string"),
            ("progress", "number"),
            ("message", "string"),
        ],
        "jobs.cancel" => &[("job", "string"), ("cancelled", "boolean")],
        "subscribe" | "unsubscribe" => &[("topics", "array")],
        "daemon.shutdown" => &[("stopping", "boolean")],
        "doc.sync" => &[
            ("doc", "string"),
            ("path", "string"),
            ("kept", "string"),
            ("revision", "integer"),
        ],
        "editor.gesture" => &[("active", "boolean")],
        "collab.activity" => &[("participants", "array"), ("presence", "object")],
        "collab.join" => &[
            ("participant", "object"),
            ("revision", "integer"),
            ("presence", "object"),
            ("participants", "array"),
            ("claims", "array"),
            ("branches", "array"),
            ("conflicts", "array"),
            ("text", "string"),
        ],
        "collab.leave" => &[("left", "boolean")],
        "collab.presence" => &[("ok", "boolean")],
        "collab.who" => &[
            ("participants", "integer"),
            ("claims", "integer"),
            ("text", "string"),
        ],
        "collab.claim" => &[
            ("claims", "array"),
            ("overlaps", "array"),
            ("text", "string"),
        ],
        "collab.release" => &[("released", "integer"), ("text", "string")],
        "collab.heartbeat" => &[("claims", "integer")],
        "collab.claims" => &[("claims", "array")],
        "collab.submit" => &[
            ("txn", "string"),
            ("revision", "integer"),
            ("replayed", "boolean"),
            ("overwrote", "integer"),
        ],
        "collab.sync" => &[
            ("revision", "integer"),
            ("snapshot", "object"),
            ("ops", "array"),
        ],
        "collab.undo" | "collab.redo" => &[
            ("undone", "string"),
            ("txn", "string"),
            ("skipped", "array"),
        ],
        "collab.conflicts" => &[("conflicts", "array")],
        "branch.list" => &[("branches", "array")],
        "branch.open" | "branch.status" | "branch.submit" | "branch.discard" => &[
            ("id", "string"),
            ("owner", "string"),
            ("name", "string"),
            ("baseRevision", "integer"),
            ("transactions", "integer"),
            ("status", "string"),
            ("changes", "array"),
            ("blockedBy", "array"),
        ],
        "branch.merge" => &[
            ("txn", "string"),
            ("revision", "integer"),
            ("edits", "integer"),
            ("conflicts", "array"),
        ],
        _ => &[],
    };
    let properties: serde_json::Map<_, _> = fields
        .iter()
        .map(|(name, ty)| ((*name).to_owned(), json!({"type":ty})))
        .collect();
    json!({"type":"object","description":method.summary,"properties":properties,"additionalProperties":true})
}

/// All structured reference data for one engine/protocol version and schema world.
pub fn structured(
    db: &SchemaDb,
    registry: &Registry,
    methods: &[MethodInfo],
    project: &str,
) -> String {
    let examples = csharp_examples();
    let matching = |page: &str| {
        examples
            .iter()
            .filter(|e| e.page == page)
            .collect::<Vec<_>>()
    };
    let mut types = db.reflection()["types"].clone();
    for t in db.types() {
        let ty = &mut types[&t.name];
        ty["csharpExamples"] = json!(matching(&format!("types/{}", t.name)));
        for f in t.fields() {
            let field = &mut ty["fields"][&f.name];
            field["unit"] = json!(f.unit.map(|u| u.symbol()));
            field["range"] = json!(f.range.map(|r| [r.min, r.max]));
            field["default"] = db.field_default(f);
            field["replication"] = json!(f.replicate);
            field["examples"] = json!(f.examples);
        }
    }
    let mut commands = registry.catalog();
    for info in registry.infos() {
        commands[&info.name]["parameters"] = db
            .get(&info.params)
            .map(|t| db.type_json_schema(t))
            .unwrap_or(Value::Null);
        commands[&info.name]["csharpExamples"] =
            json!(matching(&format!("commands/{}", info.name)));
    }
    let methods: serde_json::Map<_, _> = methods
        .iter()
        .map(|m| {
            let mut value = json!(m);
            value["scope"] = json!(m.scope);
            value["results"] = results(m);
            (m.name.clone(), value)
        })
        .collect();
    let diagnostics: serde_json::Map<_,_> = catalog::all().iter().map(|d| (d.code.to_string(),json!({
        "name":d.name,"label":d.label,"aliases":d.aliases,"title":d.title,"doc":d.explanation,
        "cause":d.causes,"fixes":d.fixes,"owner":d.owner
    }))).collect();
    let topics: Vec<_> = generate(db, registry, &[], project)
        .into_iter()
        .filter(|p| {
            !p.id.starts_with("types/")
                && !p.id.starts_with("commands/")
                && !p.id.starts_with("errors/")
        })
        .map(|p| json!({"id":p.id,"title":p.title,"summary":p.summary,"markdown":p.markdown()}))
        .collect();
    ara_json::to_canonical_string(&ara_json::sort_keys(&json!({
        "format":FORMAT,"engineVersion":ENGINE_VERSION,"protocolVersion":PROTOCOL,
        "types":types,"commands":commands,"methods":methods,"diagnostics":diagnostics,
        "csharpExamples":examples,"topics":topics
    })))
}
