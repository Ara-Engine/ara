//! Agent activity read from the store: turns (one agent's transactions grouped by
//! gesture), how long ago they happened, and file operations in words.

/// One agent turn in the feed: its transactions, newest first.
pub(crate) struct Turn {
    /// The turn's identity: its gesture group, or its only transaction's id.
    pub key: String,
    /// The agent, as attributed.
    pub author: String,
    /// Why, in the agent's words.
    pub why: String,
    /// Its transactions, newest first.
    pub txns: Vec<ara_ops::TxnId>,
    /// Every transaction has been reverted.
    pub undone: bool,
}

/// Agent turns in the store's recent history, newest first (at most 100).
pub(crate) fn turns(store: &ara_ops::Store) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for txn in store.history().iter().rev().take(400) {
        if !matches!(txn.author, ara_ops::Author::Agent(_)) || txn.reverts.is_some() {
            continue;
        }
        // Gesture safety may split one turn into several transactions; ara-host labels
        // them `gesture-turn:<group>|<label>`.
        let key = txn
            .label
            .as_deref()
            .and_then(|l| l.strip_prefix("gesture-turn:"))
            .and_then(|l| l.split_once('|'))
            .map(|(group, _)| group.to_owned())
            .unwrap_or_else(|| txn.id.0.clone());
        let undone = store.reverted_by(&txn.id).is_some();
        if let Some(turn) = turns.iter_mut().find(|t| t.key == key) {
            turn.txns.push(txn.id.clone());
            turn.undone &= undone;
            continue;
        }
        if turns.len() >= 100 {
            break;
        }
        turns.push(Turn {
            key,
            author: txn.author.to_string(),
            why: txn
                .why
                .clone()
                .or_else(|| {
                    txn.label.as_deref().map(|l| {
                        l.strip_prefix("gesture-turn:")
                            .and_then(|l| l.split_once('|'))
                            .map_or(l, |(_, label)| label)
                            .to_owned()
                    })
                })
                .unwrap_or_else(|| "Edit".into()),
            txns: vec![txn.id.clone()],
            undone,
        });
    }
    turns
}

/// How long ago, in words ("just now", "3 min ago").
pub(crate) fn ago(elapsed: std::time::Duration) -> String {
    let s = elapsed.as_secs();
    match s {
        0..=4 => "just now".into(),
        5..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}

/// A file op in words, with its tone.
pub(crate) fn file_change(
    path: &str,
    previous: Option<&ara_doc::file_state::FileState>,
    state: Option<&ara_doc::file_state::FileState>,
) -> (&'static str, String) {
    match (previous, state) {
        (_, None) => ("destructive", format!("Delete {path}")),
        (None, Some(_)) => ("success", format!("Create {path}")),
        _ => ("warning", format!("Modify {path}")),
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;
    #[test]
    fn file_activity_names_the_path_and_the_operation() {
        let file = ara_doc::file_state::FileState::bytes(b"source");
        assert_eq!(
            file_change("game/Σ.cs", None, Some(&file)),
            ("success", "Create game/Σ.cs".into())
        );
        assert_eq!(
            file_change("game/Σ.cs", Some(&file), Some(&file)),
            ("warning", "Modify game/Σ.cs".into())
        );
        assert_eq!(
            file_change("game/Σ.cs", Some(&file), None),
            ("destructive", "Delete game/Σ.cs".into())
        );
    }
}
