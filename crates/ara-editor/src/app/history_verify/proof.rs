use super::*;
fn text<'a>(shell: &'a Shell, id: &str) -> Option<&'a str> {
    shell
        .shown_node(id)
        .and_then(|index| shell.ui.nodes[index].text.as_ref())
        .map(|text| text.value.as_str())
}
fn enabled(shell: &Shell, id: &str) -> bool {
    shell
        .shown_node(id)
        .is_some_and(|index| !shell.ui.nodes[index].state.disabled)
}
impl HistoryCheck {
    pub(super) fn capture(
        &mut self,
        stage: &str,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        shell.leave();
        gpu.select_target(0);
        std::fs::create_dir_all(&self.output)?;
        gpu.pixels()?
            .save(self.output.join(format!("history-{stage}.png")))?;
        std::fs::write(
            self.output.join(format!("history-{stage}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        let store = editor.host.store();
        let rows = shell
            .ui
            .nodes
            .iter()
            .enumerate()
            .filter(|(index, node)| {
                shell.ui.shown(*index)
                    && node.element.tag == "button"
                    && node.element.id.starts_with("history-row-")
            })
            .count();
        let copied = if stage == "selected-copy" {
            Some(ara_json::parse(&self.clipboard.0)?)
        } else {
            None
        };
        self.trace.push(json!({"stage":stage,"records":records(editor)?,"revision":store.revision(),"last":store.history().last(),"nextUndo":store.next_undo(&ara_ops::Author::Human).map(|txn|&txn.id),"nextRedo":store.next_redo(&ara_ops::Author::Human).map(|txn|&txn.id),"selected":text(shell,"history-selected-id"),"reason":text(shell,"history-detail-reason"),"author":text(shell,"history-detail-author"),"reverts":text(shell,"history-detail-reverts"),"revertedBy":text(shell,"history-detail-reverted-by"),"copy":copied,"rows":rows,"count":text(shell,"history-page-count"),"undoEnabled":enabled(shell,"history-undo"),"redoEnabled":enabled(shell,"history-redo"),"query":editor.state.searches.get("history"),"selection":editor.state.selection,"camera":editor.state.camera}));
        Ok(())
    }
    pub(super) fn observe(&self, editor: &Editor) -> ara_ui::Result<Value> {
        let stage = |name: &str| self.trace.iter().find(|trace| trace["stage"] == name);
        let changed = stage("changed");
        let copied = stage("selected-copy");
        let undo = stage("undo");
        let redo = stage("redo");
        let search = stage("search");
        let baseline = changed.is_some_and(|trace| {
            trace["records"] == self.after
                && trace["revision"] == self.revision
                && trace["undoEnabled"] == true
        });
        let inspected = copied.is_some_and(|trace| {
            trace["selected"] == self.txn
                && trace["reason"] == "History acceptance rename"
                && trace["author"]
                    .as_str()
                    .is_some_and(|author| author.starts_with("human ·"))
                && trace["copy"]["id"] == self.txn
                && trace["copy"]["author"] == "human"
                && trace["copy"]["ops"] == trace["last"]["ops"]
                && trace["revision"] == self.revision
        });
        let undone = undo.is_some_and(|trace| {
            trace["records"] == self.before
                && trace["revision"] == self.revision + 1
                && trace["last"]["author"] == "human"
                && trace["last"]["reverts"] == self.txn
                && trace["redoEnabled"] == true
        });
        let redone = redo.is_some_and(|trace| {
            trace["records"] == self.after
                && trace["revision"] == self.revision + 2
                && trace["last"]["author"] == "human"
                && undo.is_some_and(|undo| trace["last"]["reverts"] == undo["last"]["id"])
        });
        let filtered = search.is_some_and(|trace| {
            trace["query"] == self.txn
                && trace["selected"] == self.txn
                && trace["rows"] == 1
                && trace["revision"] == self.revision + 2
        });
        let navigation = self.trace.iter().all(|trace| {
            trace["selection"] == json!(self.selection) && trace["camera"] == json!(self.camera)
        });
        let final_state =
            editor.host.store().revision() == self.revision + 2 && records(editor)? == self.after;
        Ok(
            json!({"journey":"history-native-inspect-copy-undo-redo","ok":baseline&&inspected&&undone&&redone&&filtered&&navigation&&final_state,"transaction":self.txn,"detail":{"realBaseline":baseline,"nativeInspectAndFullCanonicalCopy":inspected,"nativeUndoRevertsTarget":undone,"keyboardRedoRevertsUndo":redone,"nativeSearch":filtered,"documentSelectionAndCameraPreserved":navigation,"onlyUndoRedoAuthor":final_state}}),
        )
    }
}
