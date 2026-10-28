//! Observe exact authoring history receipts; native key events remain the only ingress.
use super::*;

impl AssetOpsCheck {
    pub(super) fn history_ready(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        steps: usize,
    ) -> ara_ui::Result<bool> {
        let source = self
            .terminal
            .as_ref()
            .and_then(|terminal| terminal["status"]["result"]["txn"].as_str())
            .ok_or("Native Assets history probe has no committed transaction identity")?;
        let source = ara_ops::TxnId(source.into());
        let original = editor
            .host
            .store()
            .txn(&source)
            .ok_or("Committed Assets history transaction is missing")?;
        let mut head = &original.id;
        let mut chain = Vec::new();
        let mut interrupted = Vec::new();
        for transaction in editor.host.store().since(original.result) {
            if transaction.reverts.as_ref() == Some(head) {
                head = &transaction.id;
                chain.push(json!({"txn":transaction.id,"revision":transaction.result,"reverts":transaction.reverts,"author":transaction.author}));
            } else if transaction.author == ara_ops::Author::Human && transaction.reverts.is_none()
            {
                interrupted.push(json!({"txn":transaction.id,"revision":transaction.result,"label":transaction.label,"why":transaction.why}));
            }
        }
        self.history_probe = json!({"source":source,"expectedReverts":steps,"observedReverts":chain.len(),"chain":chain,"interveningHumanEdits":interrupted,"revision":editor.host.store().revision(),"pendingSave":editor.host.unsaved(),"followingHistory":shell.asset_operation_report()["followingHistory"]});
        if chain.len() < steps {
            if !interrupted.is_empty() {
                return Err("A separate human authoring transaction interrupted the native Assets Undo/Redo chain; inspect historyReceipt.interveningHumanEdits".into());
            }
            return Ok(false);
        }
        if chain.len() != steps {
            return Err(
                "A single native history chord produced an unexpected number of Assets reversions"
                    .into(),
            );
        }
        Ok(!editor.host.unsaved() && shell.asset_operation_report()["followingHistory"] != true)
    }
}
