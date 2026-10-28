//! Generic Host job parsing. Keep raw results and per-operation diagnostic indexes.
use super::*;
pub(super) fn accepted(value: &Value) -> Option<&str> {
    value["job"].as_str().or_else(|| {
        value["outputs"]
            .as_array()?
            .iter()
            .find_map(|output| output["job"].as_str())
    })
}
pub(super) fn running(status: &Value) -> bool {
    status["status"] == "running"
}
pub(super) fn transaction(status: &Value) -> Option<&str> {
    status["result"]["txn"].as_str()
}
pub(super) fn can_cancel(status: &Value) -> bool {
    running(status) && status["cancellable"].as_bool() == Some(true)
}
pub(super) fn message(status: &Value, labels: &[String]) -> String {
    if status["status"] == "failed" {
        let messages: Vec<_> = status["diagnostics"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|problem| {
                let message = problem["message"]
                    .as_str()
                    .unwrap_or("The asset operation failed");
                problem["command"]
                    .as_u64()
                    .or_else(|| problem["location"]["command"].as_u64())
                    .and_then(|index| labels.get(index as usize))
                    .map_or_else(|| message.to_owned(), |label| format!("{label}: {message}"))
            })
            .collect();
        if !messages.is_empty() {
            return messages.join("\n");
        }
    }
    status["message"]
        .as_str()
        .filter(|message| !message.is_empty())
        .unwrap_or(match status["status"].as_str() {
            Some("done") => "Asset operation complete",
            Some("failed") => "Asset operation failed",
            Some("cancelled") => "Asset operation cancelled",
            _ => "Preparing asset transaction…",
        })
        .into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_batch_item_labels_and_commit_cancellation_follow_authority_status() {
        let labels = vec!["Move Art/a".into(), "Delete Art/b".into()];
        assert_eq!(
            message(
                &json!({"status":"failed","diagnostics":[{"location":{"command":1},"message":"Still referenced by scene"}]}),
                &labels
            ),
            "Delete Art/b: Still referenced by scene"
        );
        assert!(!can_cancel(
            &json!({"status":"running","phase":"committing"})
        ));
        assert!(!can_cancel(&json!({"status":"running","progress":0.99})));
        assert!(!can_cancel(
            &json!({"status":"running","phase":"preparing"})
        ));
        assert!(!can_cancel(
            &json!({"status":"running","progress":0.1,"cancellable":false})
        ));
        assert!(!can_cancel(&json!({"status":"done","cancellable":true})));
        assert!(can_cancel(
            &json!({"status":"running","phase":"committing","progress":1.,"cancellable":true})
        ));
        assert_eq!(
            accepted(&json!({"ok":true,"job":"jb_asset","preparing":true})),
            Some("jb_asset")
        );
        assert_eq!(
            transaction(&json!({"status":"done","result":{"txn":"tx_assets","ops":2}})),
            Some("tx_assets")
        );
    }
}
