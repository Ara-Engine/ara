use super::*;
fn selected(values: &Value) -> BTreeSet<String> {
    values
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
fn expected(values: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    values.into_iter().collect()
}
impl SelectionCheck {
    pub(super) fn observe(
        &self,
        act: Act,
        editor: &Editor,
        shell: &Shell,
    ) -> ara_ui::Result<(bool, Value)> {
        let after = fixture::records(editor)?;
        let actual: BTreeSet<_> = editor.state.selection.iter().cloned().collect();
        let preview = self.trace.iter().find(|trace| trace["stage"] == "preview");
        let unchanged =
            after == self.before_records && editor.host.store().revision() == self.before_revision;
        let (ok, detail) = match act {
            Act::ShiftClick
            | Act::CtrlToggle
            | Act::CtrlShift
            | Act::KeyboardEscape
            | Act::DeletedAnchor => {
                let wanted = match act {
                    Act::ShiftClick => expected((2..=6).map(row)),
                    Act::CtrlToggle => expected([row(5)]),
                    Act::CtrlShift => expected([2, 5, 6, 7, 8].into_iter().map(row)),
                    Act::KeyboardEscape => expected([row(5)]),
                    Act::DeletedAnchor => expected([row(6)]),
                    _ => unreachable!(),
                };
                let extra = if act == Act::KeyboardEscape {
                    preview.is_some_and(|trace|selected(&trace["selection"])== expected((5..ROWS).map(row)) && trace["hierarchyOffset"].as_f64().is_some_and(|offset|offset>0.))
                        && shell.input.focused().and_then(|id|shell.bindings.get(&id)).is_some_and(|binding|matches!(binding,crate::shell::Binding::Select(id) if id==&row(5)))
                } else if act == Act::DeletedAnchor {
                    after.get(row(2).as_str()).is_none()
                        && editor.host.store().revision() == self.before_revision + 1
                } else {
                    unchanged
                };
                (
                    actual == wanted && extra,
                    json!({"selected":actual,"expected":wanted,"contentUnchanged":unchanged,"extraChecks":extra,"rangePreview":preview.map(|trace|&trace["selection"])}),
                )
            }
            Act::MarqueeReplace
            | Act::MarqueeReverse
            | Act::MarqueeAdd
            | Act::MarqueeSubtract
            | Act::MarqueeEscape => {
                let wanted = match act {
                    Act::MarqueeAdd => expected(MESHES.into_iter().map(str::to_owned)),
                    Act::MarqueeSubtract | Act::MarqueeEscape => expected([MESHES[2].into()]),
                    _ => expected(MESHES[..2].iter().map(|id| (*id).into())),
                };
                let initial = if matches!(act, Act::MarqueeAdd | Act::MarqueeEscape) {
                    expected([MESHES[2].into()])
                } else if act == Act::MarqueeSubtract {
                    expected(MESHES.into_iter().map(str::to_owned))
                } else {
                    BTreeSet::new()
                };
                let staged = preview.is_some_and(|trace| {
                    selected(&trace["selection"]) == initial
                        && trace["revision"] == self.before_revision
                });
                (
                    actual == wanted && unchanged && staged,
                    json!({"selected":actual,"expected":wanted,"selectionDeferredUntilRelease":staged,"hiddenExcluded":!actual.contains(HIDDEN),"lockedExcluded":!actual.contains(LOCKED),"from":self.from,"to":self.to}),
                )
            }
            Act::Duplicate | Act::DuplicateEscape => {
                self.duplicate_result(act, editor, &after, preview)
            }
        };
        Ok((ok, detail))
    }
    fn duplicate_result(
        &self,
        act: Act,
        editor: &Editor,
        after: &Value,
        preview: Option<&Value>,
    ) -> (bool, Value) {
        let preview_staged = preview.is_some_and(|trace| {
            trace["revision"] == self.before_revision
                && trace["records"] == self.before_records
                && trace["gesture"].is_object()
        });
        let sources_unchanged = MESHES
            .iter()
            .all(|source| after.get(*source) == self.before_records.get(*source));
        if act == Act::DuplicateEscape {
            let unchanged = after == &self.before_records
                && editor.host.store().revision() == self.before_revision;
            return (
                unchanged && preview_staged && editor.gesture.is_none(),
                json!({"unchanged":unchanged,"previewStaged":preview_staged,"captureReleased":editor.gesture.is_none()}),
            );
        }
        let committed = self
            .trace
            .iter()
            .find(|trace| trace["stage"] == "committed");
        let undo = self.trace.iter().find(|trace| trace["stage"] == "undo");
        let copied: Vec<_> = after
            .as_object()
            .into_iter()
            .flat_map(|records| records.keys())
            .filter(|id| self.before_records.get(*id).is_none())
            .collect();
        let copied_source = |source: &str| {
            let name = format!(
                "{} copy",
                self.before_records.get(source)?.get("name")?.as_str()?
            );
            copied
                .iter()
                .find(|id| {
                    after
                        .get(id.as_str())
                        .and_then(|record| record.get("name"))
                        .and_then(Value::as_str)
                        == Some(name.as_str())
                })
                .map(|id| id.as_str())
        };
        let a = copied_source(MESHES[0]);
        let b = copied_source(MESHES[1]);
        let remapped = a
            .zip(b)
            .is_some_and(|(a, b)| after[a]["FollowCamera"]["target"] == b);
        let moved = a.is_some_and(|a| {
            after[a]["Transform"]["position"]
                != self.before_records[MESHES[0]]["Transform"]["position"]
        });
        let atomic = committed.is_some_and(|trace| trace["revision"] == self.before_revision + 1);
        let undone = undo.is_some_and(|trace| trace["records"] == self.before_records);
        let redone = self.committed_records.as_ref() == Some(after);
        let ok = preview_staged
            && sources_unchanged
            && copied.len() == 2
            && moved
            && remapped
            && atomic
            && undone
            && redone
            && editor.gesture.is_none();
        (
            ok,
            json!({"copies":copied,"sourcesUnchanged":sources_unchanged,"previewStaged":preview_staged,"moved":moved,"crossRootReferenceRemapped":remapped,"oneCommitTransaction":atomic,"oneUndoRestoresAll":undone,"oneRedoRestoresAll":redone}),
        )
    }
}
