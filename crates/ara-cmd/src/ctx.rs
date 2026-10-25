//! What commands see: [`ReadCtx`] for reads, [`EditCtx`] for writes (every write
//! becomes an op in the batch's transaction), [`JobCtx`] for jobs.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use ara_diag::{Diagnostic, FixAction, codes};
use ara_doc::prefab::{Resolved, ResolvedEntity, resolve};
use ara_doc::{
    AssetId, Document, Edit, OrderKey, Record, RecordId, Workspace, field, key_between, order_key,
    prefab_of,
};
use ara_ops::{Staged, Store};
use ara_schema::{FieldPath, SchemaDb, TypeDesc, Validator};
use serde_json::Value;

/// An entity a command addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A plain record of the document.
    Plain(RecordId),
    /// A prefab instance (`inner: None`) or an entity inside one (`inner:
    /// Some("e_c001/e_lid1")`). Edits become overrides on the instance record.
    Prefab {
        /// The instance record.
        instance: RecordId,
        /// Path of record IDs inside the prefab.
        inner: Option<String>,
    },
}

impl Target {
    /// The entity address (`e_8b21`, `e_4f9a/m4tz`).
    pub fn address(&self) -> String {
        match self {
            Target::Plain(id)
            | Target::Prefab {
                instance: id,
                inner: None,
            } => id.to_string(),
            Target::Prefab {
                instance,
                inner: Some(inner),
            } => format!("{instance}/{inner}"),
        }
    }

    /// The document record that stores this entity's data.
    pub fn record(&self) -> &RecordId {
        match self {
            Target::Plain(id) | Target::Prefab { instance: id, .. } => id,
        }
    }

    /// The override key for `path` on this entity: `[<inner>/]<path>`.
    pub fn override_key(&self, path: &str) -> String {
        match self {
            Target::Prefab {
                inner: Some(inner), ..
            } => format!("{inner}/{path}"),
            _ => path.to_owned(),
        }
    }
}

/// A batch's prefab resolution of its document, shared by every read until the
/// next edit: resolving a large scene clones every entity, and one command may ask
/// several times (its target, its components, name selectors).
#[derive(Default)]
pub(crate) struct Resolutions(RefCell<Option<(usize, Rc<Resolved>)>>);

impl Resolutions {
    /// The resolution at `version` (the batch's applied op count), computed once.
    fn get(&self, version: usize, resolve: impl FnOnce() -> Resolved) -> Rc<Resolved> {
        if let Some((at, resolved)) = &*self.0.borrow()
            && *at == version
        {
            return Rc::clone(resolved);
        }
        let resolved = Rc::new(resolve());
        *self.0.borrow_mut() = Some((version, Rc::clone(&resolved)));
        resolved
    }
}

/// Read access to the store and the batch's document.
#[derive(Clone, Copy)]
pub struct ReadCtx<'a> {
    store: &'a Store,
    doc: &'a Document,
    symbols: &'a BTreeMap<String, String>,
    /// The batch's shared resolution and its current version, when there is one.
    resolutions: Option<(&'a Resolutions, usize)>,
}

impl<'a> ReadCtx<'a> {
    pub(crate) fn new(
        store: &'a Store,
        doc: &AssetId,
        symbols: &'a BTreeMap<String, String>,
    ) -> Result<Self, Diagnostic> {
        let doc = store.workspace().doc(doc.as_str()).ok_or_else(|| {
            Diagnostic::error(
                codes::DOC_NOT_FOUND,
                format!("no document `{doc}` is loaded"),
            )
        })?;
        Ok(ReadCtx {
            store,
            doc,
            symbols,
            resolutions: None,
        })
    }

    /// Reads through the batch's shared resolution at `version` (its op count).
    pub(crate) fn shared(mut self, resolutions: &'a Resolutions, version: usize) -> Self {
        self.resolutions = Some((resolutions, version));
        self
    }

    /// The Schema DB.
    pub fn schema(&self) -> &'a SchemaDb {
        self.store.schema()
    }

    /// All loaded documents.
    pub fn workspace(&self) -> &'a Workspace {
        self.store.workspace()
    }

    /// The document the batch edits.
    pub fn doc(&self) -> &'a Document {
        self.doc
    }

    /// The `Entity` record type.
    pub fn entity_type(&self) -> Result<&'a TypeDesc, Diagnostic> {
        self.schema().get("Entity").ok_or_else(|| {
            Diagnostic::error(
                codes::UNKNOWN_TYPE,
                "the `Entity` record type is not registered",
            )
        })
    }

    /// Resolves an entity address or batch symbol (`$crate`) to a [`Target`].
    pub fn target(&self, addr: &str) -> Result<Target, Diagnostic> {
        let addr = if addr.starts_with('$') {
            self.symbols.get(addr).map(String::as_str).ok_or_else(|| {
                Diagnostic::error(
                    codes::UNRESOLVED_SYMBOL,
                    format!("`{addr}` is not defined by an earlier call in this batch"),
                )
            })?
        } else {
            addr
        };
        let (head, inner) = match addr.split_once('/') {
            Some((h, rest)) => (h, Some(rest)),
            None => (addr, None),
        };
        let id = RecordId::parse(head).map_err(|msg| Diagnostic::error(codes::INVALID_ID, msg))?;
        let rec = self.doc.record(head).ok_or_else(|| self.missing(head))?;
        if prefab_of(rec).is_none() {
            return match inner {
                Some(_) => Err(Diagnostic::error(
                    codes::NOT_AN_INSTANCE,
                    format!("`{head}` is not a prefab instance, so `{addr}` does not exist"),
                )),
                None => Ok(Target::Plain(id)),
            };
        }
        // `instance/<prefab root ID>` names the instance itself (it *is* the root).
        let inner = match (inner, self.prefab_root(rec)) {
            (Some(i), Some(root)) if i == root => None,
            (Some(i), Some(root)) => Some(i.strip_prefix(&format!("{root}/")).unwrap_or(i)),
            (i, _) => i,
        };
        if let Some(i) = inner
            && !self.resolved().contains_key(&format!("{head}/{i}"))
        {
            return Err(Diagnostic::error(
                codes::RECORD_NOT_FOUND,
                format!("no entity `{addr}` in the prefab instance"),
            ));
        }
        Ok(Target::Prefab {
            instance: id,
            inner: inner.map(str::to_owned),
        })
    }

    /// The root record ID of the prefab an instance record instantiates.
    fn prefab_root(&self, instance: &Record) -> Option<String> {
        let prefab = self.workspace().find(prefab_of(instance)?)?;
        let doc = self.workspace().doc(prefab.as_str())?;
        doc.children(None).first().map(|id| id.to_string())
    }

    /// `ARA1204` for an ID this document lacks; when another loaded document has it,
    /// a fix that edits that document instead.
    fn missing(&self, head: &str) -> Diagnostic {
        let mut d = Diagnostic::error(
            codes::RECORD_NOT_FOUND,
            format!("no entity `{head}` in this document"),
        )
        .record(head);
        let elsewhere = self
            .workspace()
            .docs()
            .find(|(_, doc)| doc.id != self.doc.id && doc.record(head).is_some());
        if let Some((path, _)) = elsewhere {
            d.message = format!("no entity `{head}` in this document; it is in `{path}`");
            d = d.with_fix(
                format!("Edit `{path}` instead"),
                FixAction::Set {
                    record: None,
                    path: "doc".into(),
                    value: path.into(),
                },
            );
        }
        d
    }

    /// The document with prefab instances expanded (see [`ara_doc::prefab`]). Inside a
    /// batch it is computed once per document state and shared.
    pub fn resolved(&self) -> Rc<Resolved> {
        let resolve = || {
            let mut ignored = Vec::new();
            resolve(
                self.doc,
                self.schema(),
                self.workspace(),
                &Default::default(),
                &mut ignored,
            )
        };
        match self.resolutions {
            Some((resolutions, version)) => resolutions.get(version, resolve),
            None => Rc::new(resolve()),
        }
    }

    /// One entity after prefab resolution.
    pub fn entity(&self, target: &Target) -> Option<ResolvedEntity> {
        self.resolved().get(&target.address()).cloned()
    }

    /// True if the entity has the component (inline, from its prefab, or overridden).
    pub fn has_component(&self, target: &Target, component: &str) -> bool {
        self.entity(target)
            .is_some_and(|e| e.record.contains_key(component))
    }

    /// An order key placing an entity at `index` among `parent`'s children (`None`
    /// appends), ignoring `moving` itself.
    pub fn order_at(
        &self,
        parent: Option<&str>,
        index: Option<u32>,
        moving: Option<&RecordId>,
    ) -> Result<OrderKey, Diagnostic> {
        let keys: Vec<Option<OrderKey>> = self
            .doc
            .children(parent)
            .into_iter()
            .filter(|id| Some(*id) != moving)
            .map(|id| self.doc.record(id.as_str()).and_then(order_key))
            .collect();
        let i = index.map_or(keys.len(), |i| (i as usize).min(keys.len()));
        let lo = i.checked_sub(1).and_then(|j| keys[j].as_ref());
        let hi = keys.get(i).and_then(Option::as_ref);
        key_between(lo, hi).map_err(|msg| {
            Diagnostic::error(
                codes::INVALID_ORDER,
                format!("cannot place the entity there ({msg}); reorder its siblings first"),
            )
        })
    }

    /// Checks that `path` names an entity's `name` or component data and that `value`
    /// fits it.
    pub fn check_value(&self, path: &FieldPath, value: &Value) -> Result<(), Diagnostic> {
        let head = path.head();
        if head != field::NAME && !ara_doc::is_component_key(head) {
            return Err(Diagnostic::error(
                codes::INVALID_PATH,
                format!("`{path}`: only `name` and component fields are set this way (use `entity.reparent` for `parent` and `order`)"),
            )
            .path(path.to_string()));
        }
        let resolved = self.schema().resolve_path(self.entity_type()?, path)?;
        let mut diags = Vec::new();
        Validator::new(self.schema(), &mut diags).value(
            &resolved.ty,
            resolved.field,
            value,
            &path.to_string(),
        );
        diags
            .into_iter()
            .find(Diagnostic::is_error)
            .map_or(Ok(()), Err)
    }
}

/// Write access for commands. Every write applies immediately (later reads see it)
/// and is recorded as an op of the batch's transaction.
pub struct EditCtx<'a, 's> {
    pub(crate) staged: &'a mut Staged<'s>,
    pub(crate) doc: AssetId,
    pub(crate) symbols: &'a mut BTreeMap<String, String>,
    pub(crate) templates: &'a BTreeMap<&'static str, crate::registry::DocumentTemplate>,
    pub(crate) resolutions: &'a Resolutions,
}

impl EditCtx<'_, '_> {
    /// Creates a registered format's skeleton, falling back to an empty JSON document.
    pub fn create_from_template(
        &mut self,
        path: &str,
        kind: &str,
        id: Option<&str>,
    ) -> Result<AssetId, Diagnostic> {
        match self.templates.get(kind).copied() {
            Some(template) => template(self, path, id),
            None => self.create_document(path, kind, id),
        }
    }
    /// Apply an existing subsystem's typed intent after it lowers to a kernel edit.
    pub fn apply_edit(&mut self, edit: Edit) -> Result<(), Diagnostic> {
        self.staged.apply(edit).map(drop)
    }
    /// Create an empty document through a workspace lifecycle op, with a stable or symbolic ID.
    pub fn create_document(
        &mut self,
        path: &str,
        kind: &str,
        id: Option<&str>,
    ) -> Result<AssetId, Diagnostic> {
        let prefix = self
            .read()
            .schema()
            .kind(kind)
            .ok_or_else(|| {
                Diagnostic::error(
                    codes::UNKNOWN_SCHEMA,
                    format!("unknown document kind `{kind}`"),
                )
            })?
            .id_prefix
            .clone();
        let asset = match id {
            Some(symbol) if symbol.starts_with('$') => {
                if self.symbols.contains_key(symbol) {
                    return Err(Diagnostic::error(
                        codes::INVALID_ID,
                        "symbol is already bound",
                    ));
                }
                let id = self.staged.ids().asset_id(&prefix);
                self.symbols.insert(symbol.to_owned(), id.to_string());
                id
            }
            Some(id) => AssetId::parse(id).map_err(|e| Diagnostic::error(codes::INVALID_ID, e))?,
            None => self.staged.ids().asset_id(&prefix),
        };
        self.staged.apply(Edit::Document {
            doc: asset.clone(),
            path: path.to_owned(),
            kind: Some(kind.to_owned()),
        })?;
        Ok(asset)
    }
    /// Read access, reflecting the writes made so far.
    pub fn read(&self) -> ReadCtx<'_> {
        ReadCtx::new(self.staged.store(), &self.doc, self.symbols)
            .expect("the batch's document stays loaded for the whole batch")
            .shared(self.resolutions, self.staged.ops().len())
    }

    /// The ID for a new record: a symbol (`$crate`, minted and bound for later calls),
    /// a concrete unused ID, or a minted one when `requested` is `None`.
    pub fn new_id(
        &mut self,
        requested: Option<&str>,
        prefix: &str,
    ) -> Result<RecordId, Diagnostic> {
        let taken: BTreeSet<String> = self
            .read()
            .doc()
            .records()
            .keys()
            .map(ToString::to_string)
            .collect();
        match requested {
            Some(sym) if sym.starts_with('$') => {
                if self.symbols.contains_key(sym) {
                    return Err(Diagnostic::error(
                        codes::SYMBOL_REDEFINED,
                        format!("`{sym}` is already defined in this batch"),
                    ));
                }
                let id = self.staged.ids().record_id(prefix, |s| taken.contains(s));
                self.symbols.insert(sym.to_owned(), id.to_string());
                Ok(id)
            }
            Some(text) => {
                let id = RecordId::parse(text)
                    .map_err(|msg| Diagnostic::error(codes::INVALID_ID, msg))?;
                if id.prefix() != prefix {
                    return Err(Diagnostic::error(
                        codes::INVALID_ID,
                        format!("`{id}` must start with `{prefix}_`"),
                    ));
                }
                if taken.contains(text) {
                    return Err(Diagnostic::error(
                        codes::RECORD_EXISTS,
                        format!("record `{id}` already exists"),
                    ));
                }
                Ok(id)
            }
            None => Ok(self.staged.ids().record_id(prefix, |s| taken.contains(s))),
        }
    }

    /// Adds a record.
    pub fn create(&mut self, id: RecordId, record: Record) -> Result<(), Diagnostic> {
        let doc = self.doc.clone();
        self.staged
            .apply(Edit::Create { doc, id, record })
            .map(drop)
    }

    /// Writes or removes (`None`) the value at `path` of a document record.
    pub fn set(
        &mut self,
        id: &RecordId,
        path: FieldPath,
        value: Option<Value>,
    ) -> Result<(), Diagnostic> {
        let doc = self.doc.clone();
        self.staged
            .apply(Edit::Set {
                doc,
                id: id.clone(),
                path,
                value,
            })
            .map(drop)
    }

    /// Removes a record.
    pub fn delete(&mut self, id: &RecordId) -> Result<(), Diagnostic> {
        let doc = self.doc.clone();
        self.staged
            .apply(Edit::Delete {
                doc,
                id: id.clone(),
            })
            .map(drop)
    }

    /// Reparents and reorders a record.
    pub fn move_to(
        &mut self,
        id: &RecordId,
        parent: Option<String>,
        order: Option<OrderKey>,
    ) -> Result<(), Diagnostic> {
        let doc = self.doc.clone();
        self.staged
            .apply(Edit::Move {
                doc,
                id: id.clone(),
                parent,
                order,
            })
            .map(drop)
    }

    /// Writes entity data at `path`: directly on a plain record, or as an override on
    /// a prefab instance. `None` removes the value (or the override).
    pub fn set_entity(
        &mut self,
        target: &Target,
        path: &FieldPath,
        value: Option<Value>,
    ) -> Result<(), Diagnostic> {
        match target {
            Target::Plain(id) => self.set(id, path.clone(), value),
            Target::Prefab { instance, .. } => {
                let key = target.override_key(&path.to_string());
                self.set(instance, FieldPath::field(field::OVERRIDES).key(key), value)
            }
        }
    }
}

/// What a job sees: an [`EditCtx`] plus progress reporting.
pub struct JobCtx<'a, 's> {
    /// Edits, recorded like a command's.
    pub edit: EditCtx<'a, 's>,
    pub(crate) progress: &'a mut dyn FnMut(f32, &str),
}

impl JobCtx<'_, '_> {
    /// Reports progress in `[0, 1]` with a short message.
    pub fn progress(&mut self, fraction: f32, message: &str) {
        (self.progress)(fraction.clamp(0.0, 1.0), message);
    }
}
