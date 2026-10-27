//! The Host owns canonical policy; the Settings store is an undoable local projection.
use super::*;

impl Settings {
    /// Consume recorded Host transactions once. Even a Set which ends at the
    /// previously observed value can explicitly replace a newer private setting.
    pub(crate) fn reconcile_project(&mut self, host: &Store) -> bool {
        let revision = host.revision();
        let previous = self.observed_host_revision.replace(revision);
        if previous == Some(revision) {
            return false;
        }
        let transactions = host.since(previous.unwrap_or(0));
        if !transactions
            .iter()
            .flat_map(|transaction| &transaction.ops)
            .any(|op| {
                matches!(
                    op.doc().as_str(),
                    "pj_project" | ara_host::project_policy::PREFERENCES_ID
                )
            })
        {
            return false;
        }
        self.reconcile_values(host.workspace(), transactions)
    }

    /// Merge only policy changes observed in the transactional workspace. A local
    /// settings edit must not be replayed over an unchanged opening Host snapshot.
    /// Reading this projection neither saves files nor records content transactions.
    pub(super) fn reconcile_values(
        &mut self,
        workspace: &Workspace,
        transactions: &[ara_ops::Transaction],
    ) -> bool {
        let changed = DEFS.iter().any(|definition| {
            definition.scope == Project
                && (project_value(workspace, definition)
                    != self.observed_project.get(definition.key)
                    || (project_value(workspace, definition) != self.project.get(definition.key)
                        && project_touched(workspace, transactions, definition)))
        });
        if !changed {
            return false;
        }
        let mut projected = self.project.clone();
        for definition in DEFS.iter().filter(|definition| definition.scope == Project) {
            let current = project_value(workspace, definition);
            if current == self.observed_project.get(definition.key)
                && !project_touched(workspace, transactions, definition)
            {
                continue;
            }
            match current {
                Some(value) => {
                    projected.insert(definition.key.into(), value.clone());
                    self.observed_project
                        .insert(definition.key.into(), value.clone());
                }
                None => {
                    projected.remove(definition.key);
                    self.observed_project.remove(definition.key);
                }
            }
        }
        if projected == self.project {
            return false;
        }
        self.project = projected;
        // This private document is a read model, never an authored project file.
        // Replacing its observed state retains the existing per-author history.
        let mut document = Document::new("scene", self.doc.clone());
        for (id, values) in [(USER, &self.user), (PROJECT, &self.project)] {
            document.insert_unchecked(
                RecordId::parse(id).expect("private settings record IDs are valid"),
                Map::from_iter([("EditorSettings".into(), json!({"values":values}))]),
            );
        }
        self.store.load_doc(".ara/editor-settings.scene", document);
        true
    }

    /// Observed Host updates carry no duplicate private transactions. Refuse an
    /// inverse when its exact value has changed since the local edit. Older
    /// whole-map entries are checked as a whole, so hotpatch cannot clobber policy.
    pub(super) fn check_history_values(
        &self,
        author: &Author,
        redo: bool,
    ) -> Result<(), Vec<ara_diag::Diagnostic>> {
        let target = if redo {
            self.store.next_redo(author)
        } else {
            self.store.next_undo(author)
        };
        let Some(target) = target else {
            return Ok(());
        };
        for op in &target.ops {
            let ara_doc::Op::Set {
                doc,
                id,
                path,
                value,
                ..
            } = op
            else {
                continue;
            };
            let record = self
                .store
                .workspace()
                .doc(doc.as_str())
                .and_then(|document| document.record(id.as_str()));
            let mut current = None;
            for (index, segment) in path.segments().iter().enumerate() {
                let (ara_schema::Segment::Field(key) | ara_schema::Segment::Key(key)) = segment;
                current = if index == 0 {
                    record.and_then(|record| record.get(key))
                } else {
                    current.and_then(|value: &Value| value.get(key))
                };
            }
            if current != value.as_ref() {
                return Err(vec![
                    ara_diag::Diagnostic::error(
                        ara_diag::codes::UNDO_CONFLICT,
                        format!(
                            "{} would overwrite `{path}`, changed after this settings edit",
                            if redo { "Redo" } else { "Undo" }
                        ),
                    )
                    .doc(doc.as_str())
                    .record(id.as_str())
                    .path(path.to_string()),
                ]);
            }
        }
        Ok(())
    }
}

/// Only mutations of the effective policy source replace local settings. A
/// canonical section also owns its deletion/reset, exposing legacy fallback.
fn project_touched(
    workspace: &Workspace,
    transactions: &[ara_ops::Transaction],
    definition: &Def,
) -> bool {
    let touched =
        |doc: &str, path: &[&str]| {
            transactions
                .iter()
                .flat_map(|transaction| &transaction.ops)
                .any(|op| {
                    if op.doc().as_str() != doc {
                        return false;
                    }
                    match op {
                        ara_doc::Op::Set {
                            id, path: changed, ..
                        } if id.as_str() == "p_settings" => changed
                            .segments()
                            .iter()
                            .zip(path)
                            .all(|(segment, expected)| {
                                let (ara_schema::Segment::Field(key)
                                | ara_schema::Segment::Key(key)) = segment;
                                key.as_str() == *expected
                            }),
                        ara_doc::Op::Document { .. } => true,
                        ara_doc::Op::Create { id, .. } | ara_doc::Op::Delete { id, .. } => {
                            id.as_str() == "p_settings"
                        }
                        _ => false,
                    }
                })
        };
    let canonical_section = |section: &str| {
        workspace
            .doc(ara_host::project_policy::PREFERENCES_ID)
            .and_then(|document| document.record("p_settings"))
            .and_then(|record| record.get("settings"))
            .and_then(|settings| settings.get(section))
            .is_some()
    };
    let section_touched = |section: &str, path: &[&str]| {
        touched(ara_host::project_policy::PREFERENCES_ID, path)
            || (!canonical_section(section) && touched("pj_project", path))
    };
    match definition.key {
        "project.name" => touched("pj_project", &["settings", "name"]),
        "project.mainScene" => touched("pj_project", &["settings", "main"]),
        "rendering.quality" => {
            section_touched("rendering", &["settings", "rendering", "quality"])
                || (ara_host::project_policy::policy(workspace, "rendering").is_none()
                    && section_touched("settings", &["settings", "settings", "rendering.quality"]))
        }
        key => section_touched("settings", &["settings", "settings", key]),
    }
}

/// Match Settings::open precedence, including an explicit empty/null policy
/// section suppressing legacy fallback. Identity metadata stays in the marker.
fn project_value<'a>(workspace: &'a Workspace, definition: &Def) -> Option<&'a Value> {
    let policy = |key| ara_host::project_policy::policy(workspace, key);
    let value = match definition.key {
        "project.name" | "project.mainScene" => workspace
            .doc("pj_project")
            .and_then(|document| document.record("p_settings"))
            .and_then(|record| record.get("settings"))
            .and_then(|settings| {
                settings.get(if definition.key == "project.name" {
                    "name"
                } else {
                    "main"
                })
            }),
        "rendering.quality" => match policy("rendering") {
            Some(rendering) => rendering.get("quality"),
            None => policy("settings").and_then(|settings| settings.get(definition.key)),
        },
        _ => policy("settings").and_then(|settings| settings.get(definition.key)),
    };
    value.filter(|value| valid(definition, value).is_ok())
}
