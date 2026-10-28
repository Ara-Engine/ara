//! Owned choice results for real conflicts, using the same modal stack and focus contract.
use super::{DialogFuture, DialogOptions, Dialogs, Outcome, Reply, Response, escape, stable_id};

pub(super) struct Choice {
    labels: Vec<String>,
    default: usize,
    reply: Reply<Option<usize>>,
}

impl Choice {
    pub(super) fn default_id(&self, id: &str) -> String {
        format!("{id}-choice-{}", self.default)
    }

    pub(super) fn index(&self, id: &str, node: u64) -> Option<usize> {
        (0..self.labels.len()).find(|index| stable_id(&format!("{id}-choice-{index}")) == node)
    }

    pub(super) fn finish(self, outcome: Outcome) {
        let index = match outcome {
            Outcome::Accept => Some(self.default),
            Outcome::Choice(index) if index < self.labels.len() => Some(index),
            _ => None,
        };
        self.reply.finish(index);
    }

    pub(super) fn markup(&self, id: &str, destructive: bool) -> String {
        let mut out = format!("<scroll id='{id}-choices' class='kit-dialog-choice-list'>");
        for (index, label) in self.labels.iter().enumerate() {
            let primary = if index == self.default {
                if destructive {
                    "kit-dialog-destructive"
                } else {
                    "kit-dialog-accept"
                }
            } else {
                ""
            };
            out.push_str(&format!("<button id='{id}-choice-{index}' class='kit-dialog-button kit-dialog-choice {primary}' action='ara.dialog.choose' text='{}' />",escape(label,b'\'')));
        }
        out.push_str("</scroll>");
        out
    }
}

impl Dialogs {
    /// Ask the user to choose an owned label by index. Esc, Cancel and controller
    /// drop resolve None. Empty choices resolve immediately without mounting UI;
    /// an out-of-range default is clamped to the last choice. `accept_label` is unused.
    pub fn choose(
        &mut self,
        options: DialogOptions,
        choices: Vec<String>,
        default: usize,
    ) -> DialogFuture<Option<usize>> {
        let (reply, future) = Reply::new();
        if choices.is_empty() {
            reply.finish(None);
            return future;
        }
        let default = default.min(choices.len() - 1);
        self.push(
            options,
            None,
            Response::Choose(Choice {
                labels: choices,
                default,
                reply,
            }),
        );
        future
    }
}

#[cfg(test)]
#[path = "choice_tests.rs"]
mod tests;
