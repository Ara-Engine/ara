//! Generated docs (R03 §4.9): one markdown page per type, command, method, topic and
//! diagnostic code, built from the Schema DB, the command registry, the method
//! catalog and the code catalog (`ara_diag::catalog`) of this engine version.
//! `ara docs --llms` writes them with `llms.txt` and `llms-full.txt`; `docs.search`
//! searches the same pages.

use ara_cmd::Registry;
use ara_diag::catalog;
use ara_proto::{MethodInfo, PROTOCOL, Scope};
use ara_schema::{SchemaDb, Shape, StructRole, TypeDesc};
use serde_json::Value;

mod examples;
mod structured;
pub use examples::{CsharpExample, csharp_examples};
pub use structured::{FORMAT, structured};

/// Engine version the docs describe.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One generated page.
#[derive(Debug, Clone, PartialEq)]
pub struct DocPage {
    /// Path without extension (`types/Transform`); the URI is `ara://docs/<id>`.
    pub id: String,
    /// Title.
    pub title: String,
    /// One line.
    pub summary: String,
    /// Markdown body (without the title).
    pub body: String,
}

impl DocPage {
    /// The page as a markdown file.
    pub fn markdown(&self) -> String {
        format!("# {}\n\n{}\n", self.title, self.body.trim_end())
    }
}

const CONVENTIONS: &str = "\
- **Axes and units:** Y-up, right-handed. Lengths in meters, time in seconds, mass in \
kilograms, angles in degrees (`Transform.rotation` is XYZ Euler degrees). Every \
physical field's unit is in its schema (`x-unit`).
- **Documents** (`.scene`, `.prefab`) are canonical JSON: a flat `records` map keyed by \
short IDs. Defaults are omitted; key order is the formatter's (`ara fmt`), never by hand.
- **IDs:** `<prefix>_<base32>`: entities `e_8b21`, scenes `sc_…`, prefabs `pf_…`. An \
entity inside a prefab instance is addressed through it: `e_4f9a/e_m4tz`.
- **References:** `@path#id` (`@prefabs/crate.prefab#pf_crate001`). Either half repairs \
the other when it goes stale.
- **Edit with commands**, not raw file surgery, for hierarchy, prefabs and graphs: \
`ara_apply` / `ara do`. Batches are atomic; mint symbolic IDs with `\"as\": \"$crate\"` \
and use them in later calls. `dryRun` previews the diff.
- **Agent turns:** every change is a transaction with a `why`. Agent changes apply immediately. \
History, per-turn undo, overwrite notices and active-field gesture safety keep collaboration visible.
- **Sessions:** begin one (`session.begin`) and pass its handle to every call. Scopes \
limit what a session may do.
- **Find things:** `schema.search` for components and commands, `docs.search` for these \
pages, `query` for entities, `logs` for diagnostics (stable `ARA####` codes with fixes).";

fn first_line(doc: &str) -> String {
    doc.split("\n\n")
        .next()
        .unwrap_or_default()
        .replace('\n', " ")
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn fields_table(db: &SchemaDb, t: &TypeDesc) -> String {
    match &t.shape {
        Shape::Struct { fields, .. } if !fields.is_empty() => {
            let mut s = String::from(
                "| Field | Type | Default | Unit | Description |\n|---|---|---|---|---|\n",
            );
            for f in fields {
                let default = if f.required {
                    "required".to_owned()
                } else {
                    format!(
                        "`{}`",
                        serde_json::to_string(&db.field_default(f)).unwrap_or_default()
                    )
                };
                s.push_str(&format!(
                    "| `{}` | `{}` | {} | {} | {} |\n",
                    f.name,
                    f.ty,
                    cell(&default),
                    f.unit.map_or("", |u| u.symbol()),
                    cell(&f.doc)
                ));
            }
            s
        }
        Shape::Struct { .. } => "No fields.\n".into(),
        Shape::Enum { variants } => {
            let mut s = String::from("| Variant | Description |\n|---|---|\n");
            for v in variants {
                s.push_str(&format!("| `{}` | {} |\n", v.name, cell(&v.doc)));
            }
            s
        }
    }
}

fn schema_table(schema: &Value) -> String {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return "No params.\n".into();
    };
    if props.is_empty() {
        return "No params.\n".into();
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut s = String::from("| Param | Required | Description |\n|---|---|---|\n");
    for (name, p) in props {
        let doc = p.get("description").and_then(Value::as_str).unwrap_or("");
        let req = if required.contains(&name.as_str()) {
            "yes"
        } else {
            ""
        };
        s.push_str(&format!("| `{name}` | {req} | {} |\n", cell(doc)));
    }
    s
}

fn kind_label(t: &TypeDesc) -> &'static str {
    match t.role() {
        Some(StructRole::Component) => "Component",
        Some(StructRole::Record(_)) => "Record type",
        Some(StructRole::Params) => "Params",
        Some(StructRole::Value) => "Value struct",
        Some(StructRole::Resource) => "Resource",
        None => "Enum",
    }
}

/// Every page for this engine build and project.
pub fn generate(
    db: &SchemaDb,
    registry: &Registry,
    methods: &[MethodInfo],
    project: &str,
) -> Vec<DocPage> {
    let mut pages = vec![DocPage {
        id: "conventions".into(),
        title: "Conventions".into(),
        summary:
            "Y-up, meters, degrees, IDs, `@path#id` references, commands over file surgery, review."
                .into(),
        body: CONVENTIONS.into(),
    }];
    let mut scopes = String::from("| Scope | Allows |\n|---|---|\n");
    for s in Scope::ALL {
        scopes.push_str(&format!("| `{s}` | {} |\n", s.doc()));
    }
    scopes.push_str("\nThe default policy grants agents every scope except `generate:$`, `fs:outside-project` and `net`.\n");
    pages.push(DocPage {
        id: "scopes".into(),
        title: "Session scopes".into(),
        summary: "Capabilities a session can hold; checked before every method and command.".into(),
        body: scopes,
    });
    for m in methods {
        let mut body = format!("{}\n\n", m.summary);
        body.push_str(&format!(
            "- Scope: {}\n- Service: `{}`\n",
            m.scope
                .map_or("none (no session needed)".to_owned(), |s| format!("`{s}`")),
            m.service
        ));
        if let Some(t) = &m.tool {
            let action = t
                .action
                .as_ref()
                .map(|a| format!(" with `\"action\": \"{a}\"`"))
                .unwrap_or_default();
            body.push_str(&format!("- MCP: `{}`{action}\n", t.tool));
        }
        body.push_str(&format!("\n## Params\n\n{}", schema_table(&m.params)));
        pages.push(DocPage {
            id: format!("methods/{}", m.name),
            title: format!("Method `{}`", m.name),
            summary: m.summary.clone(),
            body,
        });
    }
    for info in registry.infos() {
        let mut body = format!(
            "{}\n\n- Kind: {:?}\n- Scope: `{}`\n- Idempotent: {} · destructive: {}\n",
            info.summary, info.kind, info.scope, info.hints.idempotent, info.hints.destructive
        );
        if let Some(t) = db.get(&info.params) {
            body.push_str(&format!(
                "\n## Params (`{}`)\n\n{}",
                t.name,
                fields_table(db, t)
            ));
        }
        if !info.examples.is_empty() {
            body.push_str("\n## Examples\n\n");
            for ex in &info.examples {
                body.push_str(&format!(
                    "```json\n{{ \"cmd\": \"{}\", \"args\": {} }}\n```\n",
                    info.name,
                    serde_json::to_string(ex).unwrap_or_default()
                ));
            }
        }
        examples::append(&mut body, &format!("commands/{}", info.name));
        pages.push(DocPage {
            id: format!("commands/{}", info.name),
            title: format!("Command `{}`", info.name),
            summary: info.summary.to_owned(),
            body,
        });
    }
    for t in db.types() {
        let mut body = format!("{}\n\n- Kind: {}\n", t.doc, kind_label(t));
        if !t.module.is_empty() {
            body.push_str(&format!("- Module: `{}`\n", t.module));
        }
        if t.version > 1 {
            body.push_str(&format!("- Version: {}\n", t.version));
        }
        if let Some(d) = &t.deprecated {
            body.push_str(&format!("- Deprecated: {d}\n"));
        }
        body.push_str(&format!(
            "- JSON Schema: `ara://schema/type/{}`\n\n",
            t.name
        ));
        body.push_str(&fields_table(db, t));
        examples::append(&mut body, &format!("types/{}", t.name));
        pages.push(DocPage {
            id: format!("types/{}", t.name),
            title: format!("{} `{}`", kind_label(t), t.name),
            summary: first_line(&t.doc),
            body,
        });
    }
    let mut errors = String::from(
        "Every code has a page (`errors/<code>`): what it means, common causes and fixes. `ara why <code>` adds where it happens now.\n\n| Code | Title | Meaning | Reported by |\n|---|---|---|---|\n",
    );
    for doc in catalog::all() {
        errors.push_str(&format!(
            "| [`{}`](errors/{}.md) | {} | {} | {} |\n",
            doc.code,
            doc.code,
            cell(doc.title),
            cell(doc.summary()),
            cell(doc.owner)
        ));
        pages.push(DocPage {
            id: format!("errors/{}", doc.code),
            title: format!("{} {}", doc.code, doc.title),
            summary: doc.summary().to_owned(),
            body: doc.markdown(),
        });
    }
    pages.push(DocPage {
        id: "errors".into(),
        title: "Diagnostic codes".into(),
        summary: "Every stable diagnostic code: title, meaning and who reports it.".into(),
        body: errors,
    });
    pages.push(DocPage {
        id: "project".into(),
        title: format!("Project `{project}`"),
        summary: format!("Docs generated for project `{project}`."),
        body: format!(
            "Generated by ara {ENGINE_VERSION} (protocol `{PROTOCOL}`). Regenerate with `ara docs --llms` after changing types or upgrading the engine.\n"
        ),
    });
    for example in csharp_examples()
        .iter()
        .filter(|e| e.page.starts_with("topics/"))
    {
        pages.push(DocPage {
            id: example.page.clone(),
            title: "C# structural commands".into(),
            summary: "Spawn, add, remove and despawn through deterministic ECS commands.".into(),
            body: format!("```csharp\n{}\n```\n", example.code),
        });
    }
    pages
}

/// `llms.txt` (llmstxt.org): a title, a summary and one link per page.
pub fn llms_txt(pages: &[DocPage], project: &str) -> String {
    let mut s = format!(
        "# Ara · {project}\n\n> Ara is an AI-native game engine. This is the generated reference for project `{project}`: conventions, Engine Protocol methods (also the MCP tools), commands, component types and diagnostic codes.\n\nGenerated by ara {ENGINE_VERSION} · protocol {PROTOCOL}.\n"
    );
    let sections = [
        ("Start here", ""),
        ("Engine Protocol methods", "methods/"),
        ("Commands", "commands/"),
        ("Types", "types/"),
        ("Topics", "topics/"),
    ];
    for (title, prefix) in sections {
        s.push_str(&format!("\n## {title}\n\n"));
        for p in pages {
            let top = !p.id.contains('/');
            if (prefix.is_empty() && top) || (!prefix.is_empty() && p.id.starts_with(prefix)) {
                s.push_str(&format!("- [{}]({}.md): {}\n", p.title, p.id, p.summary));
            }
        }
    }
    s
}

/// `llms-full.txt`: every page in one file.
pub fn llms_full(pages: &[DocPage], project: &str) -> String {
    let mut s = llms_txt(pages, project);
    for p in pages {
        s.push_str("\n---\n\n");
        s.push_str(&p.markdown());
    }
    s
}

/// Pages ranked for `query`: words matched in the title score 3, in the summary 2,
/// in the body 1. Ties keep page order.
pub fn search<'a>(pages: &'a [DocPage], query: &str, limit: usize) -> Vec<&'a DocPage> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut scored: Vec<(usize, usize, &DocPage)> = pages
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let (title, summary, body) = (
                p.title.to_lowercase(),
                p.summary.to_lowercase(),
                p.body.to_lowercase(),
            );
            let score: usize = words
                .iter()
                .map(|w| {
                    3 * usize::from(title.contains(w.as_str()))
                        + 2 * usize::from(summary.contains(w.as_str()))
                        + usize::from(body.contains(w.as_str()))
                })
                .sum();
            (score > 0).then_some((score, i, p))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, _, p)| p).collect()
}

/// The first lines of a page's body around the first matching word.
pub fn snippet(page: &DocPage, query: &str) -> String {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let lines: Vec<&str> = page.body.lines().collect();
    let at = lines
        .iter()
        .position(|l| {
            let l = l.to_lowercase();
            words.iter().any(|w| l.contains(w.as_str()))
        })
        .unwrap_or(0);
    lines[at..]
        .iter()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}
