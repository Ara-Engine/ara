//! The registry: command, query and job definitions, their catalog, and batch
//! execution.

use std::collections::BTreeMap;

use ara_diag::{Diagnostic, codes, did_you_mean};
use ara_doc::{AssetId, Op};
use ara_ops::{Author, Commit, Store, TxnId, TxnMeta};
use ara_schema::{Schema, SchemaDb, Validator};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ctx::{EditCtx, JobCtx, ReadCtx};

/// What a registry entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Changes documents; recorded as ops.
    Command,
    /// Reads only.
    Query,
    /// Long-running work with progress; may change documents.
    Job,
}

/// Behaviour hints for approval policies and MCP tool annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Hints {
    /// Repeating the call with the same params changes nothing further.
    pub idempotent: bool,
    /// The call removes data.
    pub destructive: bool,
}

/// A registry entry's description: what the catalog, CLI, palette and MCP show.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// Stable name (`entity.create`).
    pub name: &'static str,
    /// Command, query or job.
    pub kind: Kind,
    /// One line on what it does.
    pub summary: &'static str,
    /// Name of the params type in the Schema DB.
    pub params: String,
    /// Behaviour hints.
    pub hints: Hints,
    /// Capability scope a session needs (`edit:scene`, `read`; see the Engine
    /// Protocol scope vocabulary).
    pub scope: &'static str,
    /// Example params.
    pub examples: Vec<Value>,
}

/// A command: validated params in, ops out. `run` may change documents only
/// through `ctx`.
pub trait Command: 'static {
    /// Params, described in the Schema DB and deserialized with serde (camelCase).
    type Params: Schema + DeserializeOwned;
    /// Result returned to the caller.
    type Output: Serialize;
    /// Stable name (`entity.create`).
    const NAME: &'static str;
    /// One line on what it does.
    const SUMMARY: &'static str;
    /// Capability scope required. Scene and prefab edits need `edit:scene`.
    const SCOPE: &'static str = "edit:scene";
    /// Behaviour hints.
    const HINTS: Hints = Hints {
        idempotent: false,
        destructive: false,
    };
    /// Example params; each must validate and deserialize (tests check).
    fn examples() -> Vec<Value>;
    /// Runs the command.
    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Self::Output, Diagnostic>;
}

/// A query: reads only.
pub trait Query: 'static {
    /// Params type.
    type Params: Schema + DeserializeOwned;
    /// Result type.
    type Output: Serialize;
    /// Stable name.
    const NAME: &'static str;
    /// One line on what it does.
    const SUMMARY: &'static str;
    /// Capability scope required.
    const SCOPE: &'static str = "read";
    /// Example params.
    fn examples() -> Vec<Value>;
    /// Runs the query.
    fn run(ctx: &ReadCtx<'_>, params: Self::Params) -> Result<Self::Output, Diagnostic>;
}

/// A job: like a command, plus progress reporting. Jobs run synchronously for now.
pub trait Job: 'static {
    /// Params type.
    type Params: Schema + DeserializeOwned;
    /// Result type.
    type Output: Serialize;
    /// Stable name.
    const NAME: &'static str;
    /// One line on what it does.
    const SUMMARY: &'static str;
    /// Capability scope required. Scene and prefab edits need `edit:scene`.
    const SCOPE: &'static str = "edit:scene";
    /// Behaviour hints.
    const HINTS: Hints = Hints {
        idempotent: false,
        destructive: false,
    };
    /// Example params.
    fn examples() -> Vec<Value>;
    /// Runs the job.
    fn run(ctx: &mut JobCtx<'_, '_>, params: Self::Params) -> Result<Self::Output, Diagnostic>;
}

type EditFn = fn(&mut EditCtx<'_, '_>, Value) -> Result<Value, Diagnostic>;
type ReadFn = fn(&ReadCtx<'_>, Value) -> Result<Value, Diagnostic>;
type JobFn = fn(&mut JobCtx<'_, '_>, Value) -> Result<Value, Diagnostic>;

/// Initializes a format's canonical document skeleton using ordinary staged edits.
pub type DocumentTemplate =
    fn(&mut EditCtx<'_, '_>, &str, Option<&str>) -> Result<AssetId, Diagnostic>;

enum Exec {
    Edit(EditFn),
    Read(ReadFn),
    Job(JobFn),
}

struct Entry {
    info: Info,
    exec: Exec,
    parse: fn(Value) -> Result<(), String>,
}

/// All registered commands, queries and jobs.
#[derive(Default)]
pub struct Registry {
    entries: BTreeMap<&'static str, Entry>,
    document_templates: BTreeMap<&'static str, DocumentTemplate>,
}

fn params_of<P: DeserializeOwned>(v: Value) -> Result<P, Diagnostic> {
    serde_json::from_value(v).map_err(|e| Diagnostic::error(codes::INVALID_PARAMS, e.to_string()))
}

fn output<O: Serialize>(o: O) -> Result<Value, Diagnostic> {
    ara_json::to_value(&o).map_err(|e| Diagnostic::error(codes::INVALID_PARAMS, e.to_string()))
}

fn edit_fn<C: Command>(ctx: &mut EditCtx<'_, '_>, v: Value) -> Result<Value, Diagnostic> {
    output(C::run(ctx, params_of(v)?)?)
}

fn read_fn<Q: Query>(ctx: &ReadCtx<'_>, v: Value) -> Result<Value, Diagnostic> {
    output(Q::run(ctx, params_of(v)?)?)
}

fn job_fn<J: Job>(ctx: &mut JobCtx<'_, '_>, v: Value) -> Result<Value, Diagnostic> {
    output(J::run(ctx, params_of(v)?)?)
}

fn parse_fn<P: DeserializeOwned>(v: Value) -> Result<(), String> {
    serde_json::from_value::<P>(v)
        .map(drop)
        .map_err(|e| e.to_string())
}

impl Registry {
    /// Registers a document format/template for `doc.create`; it must create and
    /// initialize its documents through the supplied edit context, without file I/O.
    pub fn document_template(
        &mut self,
        kind: &'static str,
        template: DocumentTemplate,
    ) -> &mut Self {
        self.document_templates.insert(kind, template);
        self
    }
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// The starter set (see [`crate::builtin`]), registering its params types in `db`.
    pub fn builtin(db: &mut SchemaDb) -> Self {
        let mut reg = Registry::new();
        crate::builtin::register(&mut reg, db);
        reg
    }

    fn insert<P: Schema + DeserializeOwned>(
        &mut self,
        db: &mut SchemaDb,
        info: Info,
        exec: Exec,
    ) -> &mut Self {
        db.register::<P>();
        self.entries.insert(
            info.name,
            Entry {
                info,
                exec,
                parse: parse_fn::<P>,
            },
        );
        self
    }

    /// Registers a command and its params type.
    pub fn command<C: Command>(&mut self, db: &mut SchemaDb) -> &mut Self {
        let info = Info {
            name: C::NAME,
            kind: Kind::Command,
            summary: C::SUMMARY,
            params: C::Params::describe().name,
            hints: C::HINTS,
            scope: C::SCOPE,
            examples: C::examples(),
        };
        self.insert::<C::Params>(db, info, Exec::Edit(edit_fn::<C>))
    }

    /// Registers a query and its params type.
    pub fn query<Q: Query>(&mut self, db: &mut SchemaDb) -> &mut Self {
        let info = Info {
            name: Q::NAME,
            kind: Kind::Query,
            summary: Q::SUMMARY,
            params: Q::Params::describe().name,
            hints: Hints {
                idempotent: true,
                destructive: false,
            },
            scope: Q::SCOPE,
            examples: Q::examples(),
        };
        self.insert::<Q::Params>(db, info, Exec::Read(read_fn::<Q>))
    }

    /// Registers a job and its params type.
    pub fn job<J: Job>(&mut self, db: &mut SchemaDb) -> &mut Self {
        let info = Info {
            name: J::NAME,
            kind: Kind::Job,
            summary: J::SUMMARY,
            params: J::Params::describe().name,
            hints: J::HINTS,
            scope: J::SCOPE,
            examples: J::examples(),
        };
        self.insert::<J::Params>(db, info, Exec::Job(job_fn::<J>))
    }

    /// One entry's description.
    pub fn info(&self, name: &str) -> Option<&Info> {
        self.entries.get(name).map(|e| &e.info)
    }

    /// Every entry, sorted by name.
    pub fn infos(&self) -> impl Iterator<Item = &Info> {
        self.entries.values().map(|e| &e.info)
    }

    /// The `commands` section of `reflection.json`.
    pub fn catalog(&self) -> Value {
        Value::Object(
            self.entries
                .values()
                .map(|e| {
                    let mut v = serde_json::to_value(&e.info).unwrap_or(Value::Null);
                    if let Some(o) = v.as_object_mut() {
                        o.shift_remove("name");
                    }
                    (e.info.name.to_owned(), v)
                })
                .collect(),
        )
    }

    /// Checks that every entry's examples validate against its params schema and
    /// deserialize into its params type.
    pub fn check_examples(&self, db: &SchemaDb) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for e in self.entries.values() {
            for (i, example) in e.info.examples.iter().enumerate() {
                let at = |d: Diagnostic| d.record(e.info.name).command(i);
                out.extend(validate_params(db, &e.info, example).into_iter().map(at));
                if let Err(msg) = (e.parse)(example.clone()) {
                    out.push(at(Diagnostic::error(codes::INVALID_PARAMS, msg)));
                }
            }
            if e.info.examples.is_empty() {
                out.push(
                    Diagnostic::error(codes::INVALID_PARAMS, "every command needs an example")
                        .record(e.info.name),
                );
            }
        }
        out
    }

    /// Runs a batch as one transaction: every call in order, symbols (`$c1`) bound by
    /// earlier calls substituted into later ones and name selectors (`name:Crate`)
    /// resolved against the staged document, then commit (or, for a dry run, check
    /// and roll back). Fails as a whole with located diagnostics.
    pub fn apply(
        &self,
        store: &mut Store,
        batch: &Batch,
        session: &Session,
    ) -> Result<BatchResult, Vec<Diagnostic>> {
        self.run(store, batch, session, &mut |_, _| {})
    }

    /// [`apply`](Self::apply), reporting job progress to `progress`.
    pub fn run(
        &self,
        store: &mut Store,
        batch: &Batch,
        session: &Session,
        progress: &mut dyn FnMut(f32, &str),
    ) -> Result<BatchResult, Vec<Diagnostic>> {
        let one = |d: Diagnostic| vec![d];
        let doc = store.workspace().find(&batch.doc).cloned().ok_or_else(|| {
            one(Diagnostic::error(
                codes::DOC_NOT_FOUND,
                format!("no document `{}` is loaded", batch.doc),
            ))
        })?;
        if let Some(t) = batch
            .idempotency_key
            .as_deref()
            .and_then(|k| store.find_by_key(k))
        {
            return Ok(BatchResult::replayed(
                t.id.clone(),
                t.ops.clone(),
                store.revision(),
            ));
        }
        let mut names: Vec<&str> = batch.calls.iter().map(|c| c.command.as_str()).collect();
        names.dedup();
        let meta = TxnMeta {
            author: session.author.clone(),
            why: batch.why.clone(),
            label: Some(names.join(", ")),
            base: batch.base,
            idempotency_key: batch.idempotency_key.clone(),
        };
        let mut symbols = BTreeMap::new();
        let mut outputs = Vec::new();
        let mut staged = store.begin(meta);
        for (i, call) in batch.calls.iter().enumerate() {
            let at = |d: Diagnostic| d.command(i);
            let entry = self.entries.get(call.command.as_str()).ok_or_else(|| {
                let mut msg = format!("unknown command `{}`", call.command);
                if let Some(s) = did_you_mean(&call.command, self.entries.keys().copied()) {
                    msg.push_str(&format!("; did you mean `{s}`?"));
                }
                one(at(Diagnostic::error(codes::UNKNOWN_COMMAND, msg)))
            })?;
            if !session.allows(entry.info.scope) {
                return Err(one(at(Diagnostic::error(
                    codes::MISSING_SCOPE,
                    format!(
                        "`{}` needs the `{}` scope, which this session lacks",
                        entry.info.name, entry.info.scope
                    ),
                ))));
            }
            let mut params = call.params.clone();
            substitute(&mut params, &symbols);
            let entities = ReadCtx::new(staged.store(), &doc, &symbols)
                .map_err(|d| one(at(d)))?
                .resolved();
            crate::select::resolve(
                staged.store().schema(),
                &entry.info.params,
                &mut params,
                &entities,
            )
            .map_err(|d| one(at(d)))?;
            let problems: Vec<Diagnostic> =
                validate_params(staged.store().schema(), &entry.info, &params)
                    .into_iter()
                    .filter(Diagnostic::is_error)
                    .map(at)
                    .collect();
            if !problems.is_empty() {
                return Err(problems);
            }
            let result = match entry.exec {
                Exec::Edit(f) => f(
                    &mut EditCtx {
                        staged: &mut staged,
                        doc: doc.clone(),
                        symbols: &mut symbols,
                        templates: &self.document_templates,
                    },
                    params,
                ),
                Exec::Read(f) => {
                    ReadCtx::new(staged.store(), &doc, &symbols).and_then(|ctx| f(&ctx, params))
                }
                Exec::Job(f) => {
                    let edit = EditCtx {
                        staged: &mut staged,
                        doc: doc.clone(),
                        symbols: &mut symbols,
                        templates: &self.document_templates,
                    };
                    f(&mut JobCtx { edit, progress }, params)
                }
            };
            outputs.push(result.map_err(|d| one(at(d)))?);
        }
        if batch.dry_run {
            let errors = staged.check();
            if !errors.is_empty() {
                return Err(errors);
            }
            let ops = staged.rollback();
            return Ok(BatchResult {
                dry_run: true,
                txn: None,
                replayed: false,
                revision: store.revision(),
                outputs,
                symbols,
                ops,
            });
        }
        let (txn, ops, replayed) = match staged.commit()? {
            Commit::Applied(t) => (Some(t.id.clone()), t.ops.clone(), false),
            Commit::Replayed(t) => (Some(t.id.clone()), t.ops.clone(), true),
            Commit::Empty => (None, Vec::new(), false),
        };
        Ok(BatchResult {
            dry_run: false,
            txn,
            replayed,
            revision: store.revision(),
            outputs,
            symbols,
            ops,
        })
    }
}

fn validate_params(db: &SchemaDb, info: &Info, params: &Value) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(ty) = db.get(&info.params) else {
        out.push(Diagnostic::error(
            codes::UNKNOWN_TYPE,
            format!("params type `{}` is not registered", info.params),
        ));
        return out;
    };
    match params.as_object() {
        Some(obj) => Validator::new(db, &mut out)
            .allow_symbols()
            .fields(ty, obj, "params"),
        None => out.push(
            Diagnostic::error(codes::TYPE_MISMATCH, "params must be an object").path("params"),
        ),
    }
    out
}

fn is_symbol(s: &str) -> bool {
    s.strip_prefix('$').is_some_and(|rest| {
        rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Replaces bound symbols with their IDs, also as the head of an address
/// (`$stack/e_c001`). Unbound symbols stay for the command to define or reject.
fn substitute(v: &mut Value, symbols: &BTreeMap<String, String>) {
    match v {
        Value::String(s) => {
            let (head, rest) = s.split_once('/').map_or((s.as_str(), ""), |(h, r)| (h, r));
            if is_symbol(head)
                && let Some(id) = symbols.get(head)
            {
                *s = if rest.is_empty() {
                    id.clone()
                } else {
                    format!("{id}/{rest}")
                };
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| substitute(i, symbols)),
        Value::Object(o) => o.values_mut().for_each(|i| substitute(i, symbols)),
        _ => {}
    }
}

/// One call in a batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    /// Registry name (`entity.create`).
    pub command: String,
    /// Params object.
    #[serde(default = "empty_object")]
    pub params: Value,
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

/// Calls to run as one transaction on one document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Batch {
    /// The document: asset ID, project path or `@path#id`.
    pub doc: String,
    /// Calls, in order.
    pub calls: Vec<Call>,
    /// Why, shown in review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Retrying with the same key returns the first result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// Revision the caller based the batch on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<u64>,
    /// Return the diff without applying it.
    #[serde(default)]
    pub dry_run: bool,
}

impl Batch {
    /// A batch of calls on `doc`.
    pub fn new(
        doc: impl Into<String>,
        calls: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Self {
        Batch {
            doc: doc.into(),
            calls: calls
                .into_iter()
                .map(|(c, p)| Call {
                    command: c.to_owned(),
                    params: p,
                })
                .collect(),
            why: None,
            idempotency_key: None,
            base: None,
            dry_run: false,
        }
    }
}

/// Who is calling, and what they may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Recorded as the transaction author.
    pub author: Author,
    /// Granted scopes: exact (`edit:scene`), prefixed (`edit:*`) or everything (`*`).
    pub scopes: Vec<String>,
}

impl Session {
    /// A human with every scope.
    pub fn human() -> Self {
        Session {
            author: Author::Human,
            scopes: vec!["*".into()],
        }
    }

    /// True if one of the granted scopes covers `scope`.
    pub fn allows(&self, scope: &str) -> bool {
        self.scopes.iter().any(|g| {
            g == "*"
                || g == scope
                || g.strip_suffix('*')
                    .is_some_and(|prefix| scope.starts_with(prefix))
        })
    }
}

/// The result of a batch.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchResult {
    /// True if nothing was applied.
    pub dry_run: bool,
    /// The committed transaction, unless dry-run or empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txn: Option<TxnId>,
    /// True if the idempotency key matched an earlier batch (outputs are then empty).
    pub replayed: bool,
    /// Store revision afterwards.
    pub revision: u64,
    /// Each call's output.
    pub outputs: Vec<Value>,
    /// Symbol → minted ID.
    pub symbols: BTreeMap<String, String>,
    /// The diff: ops applied (or that would be applied).
    pub ops: Vec<Op>,
}

impl BatchResult {
    fn replayed(txn: TxnId, ops: Vec<Op>, revision: u64) -> Self {
        BatchResult {
            dry_run: false,
            txn: Some(txn),
            replayed: true,
            revision,
            outputs: Vec::new(),
            symbols: BTreeMap::new(),
            ops,
        }
    }
}
