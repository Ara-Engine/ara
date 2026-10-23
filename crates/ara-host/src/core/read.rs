//! Read methods: `rpc.discover`, `schema.*`, `query`, `inspect`, `logs`,
//! `docs.search`.

use ara_diag::{Diagnostic, Severity, codes, did_you_mean};
use ara_doc::AssetId;
use ara_proto::{PROTOCOL, Scope};
use ara_schema::TypeDesc;
use serde_json::{Value, json};

use crate::docs;
use crate::host::State;
use crate::logs::LogFilter;
use crate::params::*;
use crate::query;
use crate::text;
use crate::tree::Tree;
use ara_cmd::select;

type R = Result<Value, Vec<Diagnostic>>;

pub(crate) use super::pick::find_doc;

fn offset(cursor: Option<&str>) -> Result<usize, Vec<Diagnostic>> {
    match cursor {
        None => Ok(0),
        Some(c) => c
            .strip_prefix('c')
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| {
                vec![Diagnostic::error(
                    codes::INVALID_CURSOR,
                    format!("`{c}` is not a cursor from this method"),
                )]
            }),
    }
}

pub(super) fn discover(st: &State) -> Value {
    let scopes: Vec<Value> = Scope::ALL
        .iter()
        .map(|s| json!({ "name": s, "doc": s.doc() }))
        .collect();
    json!({
        "protocol": PROTOCOL,
        "engine": docs::ENGINE_VERSION,
        "project": st.name,
        "revision": st.store.revision(),
        "scopes": scopes,
        "methods": st.catalog,
    })
}

fn type_kind(t: &TypeDesc) -> SchemaKind {
    match t.kind_name() {
        "component" => SchemaKind::Component,
        "record" => SchemaKind::Record,
        "params" => SchemaKind::Params,
        "enum" => SchemaKind::Enum,
        _ => SchemaKind::Struct,
    }
}

fn kind_name(k: SchemaKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// One searchable catalog entry.
struct Entry {
    name: String,
    kind: SchemaKind,
    summary: String,
    module: String,
    /// Field names and docs, for search.
    fields: Vec<(String, String)>,
}

fn entries(st: &State) -> Vec<Entry> {
    let db = st.store.schema();
    let mut out: Vec<Entry> = db
        .types()
        .map(|t| Entry {
            name: t.name.clone(),
            kind: type_kind(t),
            summary: t.doc.split("\n\n").next().unwrap_or_default().to_owned(),
            module: t.module.clone(),
            fields: t
                .fields()
                .iter()
                .map(|f| (f.name.clone(), f.doc.clone()))
                .collect(),
        })
        .collect();
    out.extend(st.registry.infos().map(|i| {
        Entry {
            name: i.name.to_owned(),
            kind: match i.kind {
                ara_cmd::Kind::Command => SchemaKind::Command,
                ara_cmd::Kind::Query => SchemaKind::Query,
                ara_cmd::Kind::Job => SchemaKind::Job,
            },
            summary: i.summary.to_owned(),
            module: String::new(),
            fields: db
                .get(&i.params)
                .map(|t| {
                    t.fields()
                        .iter()
                        .map(|f| (f.name.clone(), f.doc.clone()))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }));
    out.extend(st.catalog.iter().map(|m| Entry {
        name: m.name.clone(),
        kind: SchemaKind::Method,
        summary: m.summary.clone(),
        module: m.service.clone(),
        fields: Vec::new(),
    }));
    out
}

fn listing(items: &[&Entry]) -> (Vec<Value>, String) {
    let json = items
        .iter()
        .map(|e| json!({ "name": e.name, "kind": e.kind, "summary": e.summary }))
        .collect();
    let text = items
        .iter()
        .map(|e| format!("{} {} — {}\n", kind_name(e.kind), e.name, e.summary))
        .collect();
    (json, text)
}

pub(super) fn schema_list(st: &mut State, p: SchemaListParams) -> R {
    let all = entries(st);
    let items: Vec<&Entry> = all
        .iter()
        .filter(|e| p.kind.is_none_or(|k| e.kind == k))
        .filter(|e| p.module.as_ref().is_none_or(|m| e.module == *m))
        .collect();
    let (items, text) = listing(&items);
    Ok(json!({ "items": items, "text": text }))
}

pub(super) fn schema_describe(st: &mut State, p: SchemaDescribeParams) -> R {
    let db = st.store.schema();
    let mut items = Vec::new();
    let mut errors = Vec::new();
    for name in &p.names {
        if let Some((t, _)) = db.lookup(name) {
            items.push(json!({
                "name": t.name,
                "kind": type_kind(t),
                "doc": t.doc,
                "module": t.module,
                "schema": db.type_json_schema(t),
            }));
        } else if let Some(info) = st.registry.info(name) {
            items.push(json!({
                "name": info.name,
                "kind": info.kind,
                "summary": info.summary,
                "scope": info.scope,
                "hints": info.hints,
                "examples": info.examples,
                "params": db.get(&info.params).map(|t| db.type_json_schema(t)),
            }));
        } else if let Some(m) = st.catalog.iter().find(|m| m.name == *name) {
            items.push(json!(m));
        } else {
            let all = entries(st);
            let mut msg = format!("nothing is named `{name}`");
            if let Some(near) = did_you_mean(name, all.iter().map(|e| e.name.as_str())) {
                msg.push_str(&format!("; did you mean `{near}`?"));
            }
            errors.push(Diagnostic::error(codes::UNKNOWN_TYPE, msg).path("params.names"));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(json!({ "items": items }))
}

pub(super) fn schema_search(st: &mut State, p: SchemaSearchParams) -> R {
    let words: Vec<String> = p.query.split_whitespace().map(str::to_lowercase).collect();
    let all = entries(st);
    let mut scored: Vec<(usize, &Entry, Vec<&str>)> = all
        .iter()
        .filter(|e| p.kind.is_none_or(|k| e.kind == k))
        .filter_map(|e| {
            let (name, summary) = (e.name.to_lowercase(), e.summary.to_lowercase());
            let mut fields = Vec::new();
            let mut score = 0;
            for w in &words {
                score += 4 * usize::from(name.contains(w.as_str()))
                    + 2 * usize::from(summary.contains(w.as_str()));
                for (f, doc) in &e.fields {
                    if f.to_lowercase().contains(w.as_str())
                        || doc.to_lowercase().contains(w.as_str())
                    {
                        score += 1;
                        if !fields.contains(&f.as_str()) {
                            fields.push(f.as_str());
                        }
                    }
                }
            }
            (score > 0).then_some((score, e, fields))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
    scored.truncate(p.limit.unwrap_or(10).clamp(1, 100) as usize);
    let mut text = String::new();
    let items: Vec<Value> = scored
        .iter()
        .map(|(_, e, fields)| {
            text.push_str(&format!("{} {} — {}", kind_name(e.kind), e.name, e.summary));
            if !fields.is_empty() {
                text.push_str(&format!(" (fields: {})", fields.join(", ")));
            }
            text.push('\n');
            json!({ "name": e.name, "kind": e.kind, "summary": e.summary, "fields": fields })
        })
        .collect();
    if items.is_empty() {
        text = format!("nothing matches `{}`; try schema.list\n", p.query);
    }
    Ok(json!({ "items": items, "text": text }))
}

fn documents(st: &State, p: &QueryParams) -> R {
    let pattern = p.q.as_deref().filter(|q| !q.is_empty());
    let mut items = Vec::new();
    let mut text = String::new();
    for (path, doc) in st.store.workspace().docs() {
        if pattern.is_some_and(|g| !query::glob(g, path)) {
            continue;
        }
        text.push_str(&format!(
            "{} {} {} · {} records\n",
            doc.id,
            doc.kind,
            path,
            doc.records().len()
        ));
        items.push(json!({ "doc": doc.id, "kind": doc.kind, "path": path, "records": doc.records().len() }));
    }
    Ok(json!({ "total": items.len(), "items": items, "text": text }))
}

pub(super) fn query(st: &mut State, p: QueryParams) -> R {
    if p.kind == Some(QueryKind::Documents) {
        return documents(st, &p);
    }
    let db = st.store.schema();
    let mut filter = match p.q.as_deref().filter(|q| !q.trim().is_empty()) {
        Some(q) => query::parse(q, db).map_err(|d| vec![d])?,
        None => Filter::default(),
    };
    if let Some(f) = p.filter.clone() {
        query::check(&f, db, None).map_err(|d| vec![d.path("params.filter")])?;
        filter = query::and(filter, f);
    }
    let docs: Vec<AssetId> = match &p.doc {
        Some(d) => vec![find_doc(st, d)?],
        None => {
            let mut all: Vec<(&str, AssetId)> = st
                .store
                .workspace()
                .docs()
                .map(|(path, d)| (path, d.id.clone()))
                .collect();
            all.sort();
            all.into_iter().map(|(_, id)| id).collect()
        }
    };
    let ws = st.store.workspace();
    let trees: Vec<Tree> = docs.iter().filter_map(|d| Tree::build(ws, db, d)).collect();
    let mut hits: Vec<(&Tree, &str)> = Vec::new();
    for tree in &trees {
        for (addr, depth) in &tree.order {
            let prefab = tree.prefab_name(ws, addr);
            if query::matches(&filter, db, tree, prefab.as_deref(), addr, *depth) {
                hits.push((tree, addr));
            }
        }
    }
    let start = offset(p.cursor.as_deref())?;
    let limit = p.limit.unwrap_or(50).clamp(1, 500) as usize;
    let end = (start + limit).min(hits.len());
    let detail = p.detail.unwrap_or_default();
    let fields = p.fields.clone().unwrap_or_default();
    let multi = trees.len() > 1;
    let mut items = Vec::new();
    let mut text = format!(
        "{} matches in {} documents · showing {}-{end} · units m/deg · Y-up\n",
        hits.len(),
        trees.len(),
        (start + 1).min(end.max(1)),
    );
    for (tree, addr) in hits.get(start..end).unwrap_or_default() {
        let Some(e) = tree.get(addr) else { continue };
        let prefab = tree.prefab_name(ws, addr);
        let mut item = json!({ "entity": addr, "doc": tree.doc });
        if detail != Detail::Handles {
            let comps: Vec<&String> = e
                .record
                .keys()
                .filter(|k| ara_doc::is_component_key(k))
                .collect();
            item["name"] = json!(e.record.get("name"));
            item["components"] = json!(comps);
            if let Some(pf) = &prefab {
                item["prefab"] = json!(pf);
            }
        }
        if detail == Detail::Full {
            item["record"] = Value::Object(e.record.clone());
            item["parent"] = json!(tree.parent(addr));
        }
        let mut shown = Vec::new();
        if !fields.is_empty() {
            let mut values = serde_json::Map::new();
            for f in &fields {
                let v = query::field(db, e, f);
                if let Some(v) = &v {
                    shown.push(format!("{f}={}", text::short(v)));
                }
                values.insert(f.clone(), v.unwrap_or(Value::Null));
            }
            item["fields"] = Value::Object(values);
        }
        text.push_str(&text::entity_line(db, addr, e, prefab.as_deref()));
        if !shown.is_empty() {
            text.push_str(&format!(" {}", shown.join(" ")));
        }
        if multi {
            text.push_str(&format!(" · {}", tree.path));
        }
        text.push('\n');
        items.push(item);
    }
    let next = (end < hits.len()).then(|| format!("c{end}"));
    if let Some(n) = &next {
        text.push_str(&format!(
            "… {} more · next: {{\"cursor\":\"{n}\"}}\n",
            hits.len() - end
        ));
    }
    Ok(
        json!({ "total": hits.len(), "items": items, "next": next, "text": text, "revision": st.store.revision() }),
    )
}

/// The address an `inspect` entity names: an address, or a name selector (`name:Crate`,
/// also a bare name) matching exactly one entity.
fn inspect_target(tree: &Tree, key: &str) -> Result<String, Diagnostic> {
    if tree.get(key).is_some() {
        return Ok(key.to_owned());
    }
    let names = key.strip_prefix(select::NAME_PREFIX).unwrap_or(key);
    let hits = select::by_names(&tree.entities, names);
    let fix = |d: Diagnostic, addr: &String| {
        let title = format!("Use `{addr}` ({})", select::describe(&tree.entities, addr));
        d.with_fix(
            title,
            ara_diag::FixAction::Set {
                record: None,
                path: "entities".into(),
                value: json!([addr]),
            },
        )
    };
    match hits.as_slice() {
        [one] => Ok(one.clone()),
        [] => {
            let mut msg = format!("no entity `{key}` in `{}`", tree.path);
            if let Some(near) = did_you_mean(key, tree.entities.keys().map(String::as_str)) {
                msg.push_str(&format!("; did you mean `{near}`?"));
            }
            Err(Diagnostic::error(codes::RECORD_NOT_FOUND, msg).record(key))
        }
        many => Err(many.iter().fold(
            Diagnostic::error(
                codes::AMBIGUOUS_ENTITY,
                format!("`{key}` matches {} entities in `{}`", many.len(), tree.path),
            ),
            fix,
        )),
    }
}

pub(super) fn inspect(st: &mut State, p: InspectParams) -> R {
    let refs = p.entities.clone().unwrap_or_default();
    let id = super::pick::pick(st, p.doc.as_deref(), &refs)?;
    let db = st.store.schema();
    let ws = st.store.workspace();
    let tree = Tree::build(ws, db, &id).ok_or_else(|| {
        vec![Diagnostic::error(
            codes::DOC_NOT_FOUND,
            format!("`{id}` is not loaded"),
        )]
    })?;
    let format = p.format.unwrap_or_default();
    if let Some(list) = &p.entities {
        let mut out_text = String::new();
        let mut out = Vec::new();
        for key in list {
            let addr = &inspect_target(&tree, key).map_err(|d| vec![d])?;
            let Some(e) = tree.get(addr) else {
                continue;
            };
            if format == Format::Text {
                out_text.push_str(&text::details(ws, db, &tree, addr).unwrap_or_default());
                continue;
            }
            let mut comps = serde_json::Map::new();
            let mut refs = serde_json::Map::new();
            for (name, value) in &e.record {
                if !ara_doc::is_component_key(name) {
                    continue;
                }
                let fields: serde_json::Map<String, Value> = text::effective(db, name, value)
                    .into_iter()
                    .map(|(f, v, _)| (f, v))
                    .collect();
                comps.insert(name.clone(), Value::Object(fields));
                refs.insert(name.clone(), json!(format!("ara://schema/type/{name}")));
            }
            out.push(json!({
                "entity": addr,
                "name": e.record.get("name"),
                "parent": tree.parent(addr),
                "prefab": tree.prefab_name(ws, addr),
                "source": { "doc": e.source.0, "record": e.source.1 },
                "components": comps,
                "schemaRef": refs,
            }));
        }
        return Ok(match format {
            Format::Text => json!({ "doc": id, "revision": st.store.revision(), "text": out_text }),
            Format::Json => json!({ "doc": id, "revision": st.store.revision(), "entities": out }),
        });
    }
    let depth = p.depth.unwrap_or(2);
    let start = offset(p.cursor.as_deref())?;
    let limit = p.limit.unwrap_or(100).clamp(1, 1000) as usize;
    let (summary, next) = text::summary(ws, db, &tree, depth, start, limit);
    let next = next.map(|n| format!("c{n}"));
    Ok(match format {
        Format::Text => {
            json!({ "doc": id, "revision": st.store.revision(), "text": summary, "next": next })
        }
        Format::Json => {
            let entities: Vec<Value> = tree
                .order
                .iter()
                .filter(|(_, d)| *d <= depth)
                .skip(start)
                .take(limit)
                .filter_map(|(addr, d)| {
                    let e = tree.get(addr)?;
                    let comps: Vec<&String> = e
                        .record
                        .keys()
                        .filter(|k| ara_doc::is_component_key(k))
                        .collect();
                    Some(json!({
                        "entity": addr,
                        "depth": d,
                        "name": e.record.get("name"),
                        "parent": tree.parent(addr),
                        "components": comps,
                    }))
                })
                .collect();
            json!({ "doc": id, "revision": st.store.revision(), "entities": entities, "next": next })
        }
    })
}

pub(super) fn logs(st: &mut State, p: LogsParams) -> R {
    let filter = LogFilter {
        since: p.since.unwrap_or(0),
        codes: p.codes.unwrap_or_default(),
        severity: p.severity.map(|s| match s {
            SeverityParam::Error => Severity::Error,
            SeverityParam::Warning => Severity::Warning,
            SeverityParam::Info => Severity::Info,
        }),
        entity: p.entity,
        limit: p.limit.unwrap_or(50).clamp(1, 1000) as usize,
    };
    let entries = st.logs.query(&filter);
    let mut text = String::new();
    for e in &entries {
        let count = if e.count > 1 {
            format!(" ×{}", e.count)
        } else {
            String::new()
        };
        text.push_str(&format!("#{}{count} {}\n", e.seq, e.diagnostic));
    }
    if entries.is_empty() {
        text.push_str("no new entries\n");
    }
    Ok(json!({ "entries": entries, "cursor": st.logs.cursor(), "text": text }))
}

pub(super) fn docs_search(st: &mut State, p: DocsSearchParams) -> R {
    let pages = docs::generate(st.store.schema(), &st.registry, &st.catalog, &st.name);
    let hits = docs::search(&pages, &p.query, p.limit.unwrap_or(5).clamp(1, 50) as usize);
    let full = p.full.unwrap_or(false);
    let mut text = String::new();
    let results: Vec<Value> = hits
        .iter()
        .map(|page| {
            let body = if full { page.markdown() } else { docs::snippet(page, &p.query) };
            text.push_str(&format!("## {} (ara://docs/{})\n{}\n\n", page.title, page.id, body));
            json!({ "id": page.id, "uri": format!("ara://docs/{}", page.id), "title": page.title, "summary": page.summary, "text": body })
        })
        .collect();
    if results.is_empty() {
        text = format!("no docs match `{}`\n", p.query);
    }
    Ok(json!({ "results": results, "text": text }))
}
