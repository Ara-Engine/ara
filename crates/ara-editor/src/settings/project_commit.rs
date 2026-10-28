//! Project settings enter the Host op stream; its file worker owns persistence.
use super::*;
use ara_diag::{Diagnostic, codes};
use ara_host::{
    Cx,
    project_policy::{PREFERENCES, PREFERENCES_ID},
};

/// A no-op has a revision but no owned transaction to exclude during Undo.
pub(super) struct HostCommit {
    revision: u64,
    transaction: Option<ara_ops::TxnId>,
}

impl Settings {
    /// Commit project policy to the calling Host before advancing local history.
    /// User preferences remain private and use their existing background writer.
    pub(crate) fn set_with_host(
        &mut self,
        cx: &mut Cx<'_>,
        key: &str,
        value: Option<Value>,
    ) -> Result<String, Vec<Diagnostic>> {
        self.reconcile_project(cx.store());
        let definition =
            def_of(key).ok_or_else(|| vec![crate::invalid(format!("no setting `{key}`"))])?;
        let author = caller(cx)?;
        if definition.scope == User {
            return self.set(key, value, author);
        }
        let (value, label) = match value {
            Some(value) => {
                valid(definition, &value).map_err(|error| vec![crate::invalid(error)])?;
                (
                    (!ara_json::json_eq(&value, &definition.default.value())).then_some(value),
                    format!("Set {}", definition.label),
                )
            }
            None => (None, format!("Reset {}", definition.label)),
        };
        if same_override(value.as_ref(), self.project.get(key)) {
            return Ok(label);
        }
        let mut desired = self.project.clone();
        match &value {
            Some(value) => {
                desired.insert(key.into(), value.clone());
            }
            None => {
                desired.remove(key);
            }
        }
        let mut meta = TxnMeta::by(author);
        meta.label = Some(label.clone());
        meta.why = Some(label.clone());
        let mut pending = self.store.begin(meta);
        pending
            .apply(Edit::Set {
                doc: self.doc.clone(),
                id: RecordId::parse(PROJECT).expect("private settings record ID is valid"),
                path: FieldPath::field("EditorSettings").child("values").key(key),
                value,
            })
            .map_err(|error| vec![error])?;
        let errors = pending.check();
        if !errors.is_empty() {
            return Err(errors);
        }
        let host_commit = commit(cx, &self.project, &desired, &label, None)?;
        // The held private transaction cannot change between check and commit.
        // A rejected Host edit drops it without advancing either local history.
        let private_id = pending
            .commit()?
            .txn()
            .map(|transaction| transaction.id.clone());
        if let Some(private_id) = private_id {
            self.project_host_commits.insert(private_id, host_commit);
        }
        self.reload(Some(Project));
        self.reconcile_project(cx.store());
        Ok(label)
    }

    /// Undo only this author's settings entry, lowering its project-key delta
    /// against current Host policy rather than undoing unrelated scene history.
    pub(crate) fn undo_with_host(
        &mut self,
        cx: &mut Cx<'_>,
        redo: bool,
    ) -> Result<String, Vec<Diagnostic>> {
        self.reconcile_project(cx.store());
        let author = caller(cx)?;
        self.check_history_values(&author, redo)?;
        let target = if redo {
            self.store.next_redo(&author)
        } else {
            self.store.next_undo(&author)
        }
        .cloned();
        let Some(target) = target else {
            return self.undo(&author, redo);
        };
        let label = self.next_label(&author, redo).unwrap_or_default();
        let mut meta = TxnMeta::by(author.clone());
        meta.base = Some(target.result);
        let mut pending = self.store.begin(meta);
        for op in target.ops.iter().rev() {
            pending
                .apply(op.inverse().to_edit())
                .map_err(|error| vec![error])?;
        }
        let errors = undo_conflicts(pending.check());
        if !errors.is_empty() {
            return Err(errors);
        }
        let desired = pending
            .store()
            .workspace()
            .doc(self.doc.as_str())
            .and_then(|document| document.record(PROJECT))
            .and_then(|record| record.get("EditorSettings"))
            .and_then(|settings| settings.get("values"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        drop(pending.rollback());
        let host_commit = if desired != self.project {
            self.check_project_inverse(cx.store(), &target, &desired)?;
            // The service call owns the authority between this overlap check
            // and commit. Rebase past only the cancelled/unrelated edits checked.
            let base = cx.store().revision();
            Some(
                commit(
                    cx,
                    &self.project,
                    &desired,
                    &format!("{} {label}", if redo { "Redo" } else { "Undo" }),
                    Some(base),
                )
                .map_err(undo_conflicts)?,
            )
        } else {
            None
        };
        self.undo(&author, redo)?;
        if let Some(host_commit) = host_commit {
            let private_id = self
                .store
                .history()
                .last()
                .expect("successful settings Undo/Redo records its inverse")
                .id
                .clone();
            self.project_host_commits.insert(private_id, host_commit);
        }
        self.reconcile_project(cx.store());
        Ok(label)
    }

    /// Check active canonical intent, including direct commands which never
    /// create private settings entries. Cancelled pairs obey Store's Undo rules.
    fn check_project_inverse(
        &self,
        host: &Store,
        target: &ara_ops::Transaction,
        desired: &Map<String, Value>,
    ) -> Result<(), Vec<Diagnostic>> {
        let linked = self.project_host_commits.get(&target.id).ok_or_else(|| {
            vec![Diagnostic::error(
                codes::UNDO_CONFLICT,
                "This settings edit cannot be safely undone; set the value explicitly instead",
            )]
        })?;
        let cancelled_local: std::collections::BTreeSet<_> = self
            .project_host_commits
            .iter()
            .filter(|(private_id, _)| {
                self.store
                    .txn(private_id)
                    .is_some_and(|transaction| cancelled(&self.store, transaction, target.result))
            })
            .filter_map(|(_, commit)| commit.transaction.as_ref())
            .collect();
        let changed: Vec<_> = DEFS
            .iter()
            .filter(|definition| {
                definition.scope == Project
                    && self.project.get(definition.key) != desired.get(definition.key)
            })
            .collect();
        for transaction in host.since(linked.revision) {
            if cancelled(host, transaction, linked.revision)
                || cancelled_local.contains(&transaction.id)
            {
                continue;
            }
            for definition in &changed {
                if super::project_live::project_touched(
                    host.workspace(),
                    std::slice::from_ref(transaction),
                    definition,
                ) {
                    return Err(vec![Diagnostic::error(codes::UNDO_CONFLICT, format!("Settings Undo/Redo would overwrite `{}`, edited later by `{}`", definition.key, transaction.id))
                        .data(json!({"setting":definition.key,"settingsTxn":target.id,"laterHostTxn":transaction.id,"afterHostRevision":linked.revision}))]);
                }
            }
        }
        Ok(())
    }
}

/// Mirror Store::cancelled through its public history API. An inverse cancels
/// an intervening edit only when that edit was itself newer than this target.
fn cancelled(store: &Store, transaction: &ara_ops::Transaction, base: u64) -> bool {
    store.reverted_by(&transaction.id).is_some()
        || transaction
            .reverts
            .as_ref()
            .and_then(|id| store.txn(id))
            .is_some_and(|original| original.result > base)
}

fn caller(cx: &Cx<'_>) -> Result<Author, Vec<Diagnostic>> {
    cx.session()
        .map(|session| session.author.clone())
        .ok_or_else(|| {
            vec![Diagnostic::error(
                codes::SESSION_NOT_FOUND,
                "settings edits need a live session",
            )]
        })
}

fn commit(
    cx: &mut Cx<'_>,
    before: &Map<String, Value>,
    after: &Map<String, Value>,
    label: &str,
    base: Option<u64>,
) -> Result<HostCommit, Vec<Diagnostic>> {
    if !cx
        .session()
        .is_some_and(|session| session.allows(ara_proto::Scope::EditAssets))
    {
        return Err(vec![Diagnostic::error(
            codes::MISSING_SCOPE,
            "project settings need the `edit:assets` scope",
        )]);
    }
    for (doc, path) in [
        ("pj_project", "ara.project.json"),
        (PREFERENCES_ID, PREFERENCES),
    ] {
        if cx.document_diverged(doc) {
            return Err(vec![
                Diagnostic::error(
                    codes::EXTERNAL_CONFLICT,
                    "Project settings changed on disk; resolve that file before editing settings",
                )
                .doc(doc)
                .file(path),
            ]);
        }
    }
    let edits =
        project_edits(cx.store().workspace(), before, after).map_err(|error| vec![error])?;
    let result = cx.transact(Some(label.into()), |store, mut meta| {
        meta.label = Some(label.into());
        meta.base = base;
        store.transact(meta, edits).map(|commit| {
            commit
                .txn()
                .map(|transaction| (transaction.id.clone(), transaction.result))
        })
    })??;
    Ok(match result {
        Some((transaction, revision)) => HostCommit {
            revision,
            transaction: Some(transaction),
        },
        None => HostCommit {
            revision: cx.store().revision(),
            transaction: None,
        },
    })
}

/// Both checks are preflights of a known settings inverse. Their Staged
/// transactions have no `reverts` flag, but overlap still means Undo conflict.
fn undo_conflicts(mut diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    for diagnostic in &mut diagnostics {
        if diagnostic.code == codes::STALE_BASE {
            diagnostic.code = codes::UNDO_CONFLICT;
            diagnostic.message = format!(
                "Settings Undo/Redo would overwrite a later edit: {}",
                diagnostic.message
            );
        }
    }
    diagnostics
}

const POLICY: &[&str] = &["settings", "rendering", "input", "physics", "look", "ui"];

/// Lower only changed definitions, preserving the authority's unknown values and
/// top-level canonical/legacy precedence, including explicit empty/null sections.
fn project_edits(
    workspace: &Workspace,
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> Result<Vec<Edit>, Diagnostic> {
    let marker = object(workspace, "pj_project")?
        .ok_or_else(|| crate::invalid("project marker is not loaded"))?;
    let preferences = object(workspace, PREFERENCES_ID)?;
    let mut next_preferences = preferences.cloned().unwrap_or_default();
    for section in POLICY {
        if !next_preferences.contains_key(*section)
            && let Some(value) = marker.get(*section)
        {
            next_preferences.insert((*section).into(), value.clone());
        }
    }
    let mut next_marker = marker.clone();
    for definition in DEFS.iter().filter(|definition| {
        definition.scope == Project && before.get(definition.key) != after.get(definition.key)
    }) {
        let destination = match definition.key {
            "project.name" => (&mut next_marker, "name"),
            "project.mainScene" => (&mut next_marker, "main"),
            key => {
                let (section, key) = if key == "rendering.quality" {
                    ("rendering", "quality")
                } else {
                    ("settings", key)
                };
                let section = next_preferences.entry(section).or_insert_with(|| json!({}));
                if !section.is_object() {
                    *section = json!({});
                }
                (
                    section
                        .as_object_mut()
                        .expect("policy section was made an object"),
                    key,
                )
            }
        };
        match after.get(definition.key) {
            Some(value) => {
                destination.0.insert(destination.1.into(), value.clone());
            }
            None => {
                destination.0.remove(destination.1);
            }
        }
    }
    // Canonical policy and marker cleanup commit together; undoing a setting
    // later edits its key, so unrelated policy added after migration is retained.
    for section in POLICY {
        if next_preferences.contains_key(*section) {
            next_marker.remove(*section);
        }
    }
    let mut edits = Vec::new();
    match preferences {
        Some(previous) => append_delta(
            &mut edits,
            PREFERENCES_ID,
            previous,
            &next_preferences,
            &FieldPath::field("settings"),
        ),
        None => {
            let doc = AssetId::parse(PREFERENCES_ID).expect("canonical preferences ID is valid");
            edits.push(Edit::Document {
                doc: doc.clone(),
                path: PREFERENCES.into(),
                kind: Some("project".into()),
            });
            edits.push(Edit::Create {
                doc,
                id: RecordId::parse("p_settings").expect("policy record ID is valid"),
                record: Map::from_iter([("settings".into(), Value::Object(next_preferences))]),
            });
        }
    }
    append_delta(
        &mut edits,
        "pj_project",
        marker,
        &next_marker,
        &FieldPath::field("settings"),
    );
    Ok(edits)
}

fn object<'a>(
    workspace: &'a Workspace,
    doc: &str,
) -> Result<Option<&'a Map<String, Value>>, Diagnostic> {
    workspace
        .doc(doc)
        .map(|document| {
            document
                .record("p_settings")
                .and_then(|record| record.get("settings"))
                .and_then(Value::as_object)
                .ok_or_else(|| crate::invalid(format!("project policy `{doc}` is not an object")))
        })
        .transpose()
}

fn append_delta(
    edits: &mut Vec<Edit>,
    doc: &str,
    before: &Map<String, Value>,
    after: &Map<String, Value>,
    path: &FieldPath,
) {
    let keys: std::collections::BTreeSet<_> = before.keys().chain(after.keys()).collect();
    for key in keys {
        let (previous, next) = (before.get(key), after.get(key));
        if previous == next {
            continue;
        }
        let path = path.key(key);
        if let (Some(Value::Object(previous)), Some(Value::Object(next))) = (previous, next) {
            append_delta(edits, doc, previous, next, &path);
        } else {
            edits.push(Edit::Set {
                doc: AssetId::parse(doc).expect("project policy IDs are valid"),
                id: RecordId::parse("p_settings").expect("policy record ID is valid"),
                path,
                value: next.cloned(),
            });
        }
    }
}
