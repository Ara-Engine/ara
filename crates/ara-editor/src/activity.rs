//! Agent activity on the human side (D24, amended): agent edits just apply, so the
//! editor shows *that* they happened without asking for review. Recent agent changes
//! glow briefly in the viewport and hierarchy, and non-blocking notices report what an
//! agent is looking at (with View/Follow, never a silent camera takeover) and when
//! an edit overwrote a field someone touched recently.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ara_ops::{Author, Store, TxnId};

use crate::state::{Camera, EditorState};

/// How long a changed entity stays tinted after an agent edit.
pub const GLOW: Duration = Duration::from_secs(5);
/// How long notices stay up without interaction.
pub const NOTICE: Duration = Duration::from_secs(12);
/// How long after its last edit an agent still counts as active.
pub const ACTIVE: Duration = Duration::from_secs(10);

/// What a notice is about, and therefore which actions it offers.
#[derive(Debug, Clone, PartialEq)]
pub enum NoticeKind {
    /// An agent's view (scene, camera or selection) changed; offers View and Follow.
    AgentView {
        /// Agent session ID (the key of `EditorState::presence`).
        agent: String,
    },
    /// An edit overwrote a field another participant set recently.
    Overwrite,
}

/// One non-blocking message shown over the Scene view.
#[derive(Debug, Clone)]
pub struct Notice {
    /// Stable identity for its buttons.
    pub id: u64,
    /// Kind and actions.
    pub kind: NoticeKind,
    /// One line, for example "Claude is viewing hangar.scene · Crate".
    pub text: String,
    /// When it was raised or last updated.
    pub at: Instant,
}

/// Agent activity seen by this editor window. Owned by [`crate::Editor`].
#[derive(Debug, Default)]
pub struct Activity {
    seen_revision: u64,
    log_cursor: Option<u64>,
    last_log_poll: Option<Instant>,
    recent: Vec<(TxnId, Instant)>,
    seen_at: BTreeMap<TxnId, Instant>,
    views: BTreeMap<String, (String, Vec<String>, Camera)>,
    notices: Vec<Notice>,
    next_notice: u64,
    last_agent: Option<Instant>,
}

impl Activity {
    /// Starts after `revision`: history from before this window opened never glows.
    pub fn new(revision: u64) -> Self {
        Self {
            seen_revision: revision,
            ..Default::default()
        }
    }
    /// Records agent transactions committed since the last call: each touched record
    /// glows for [`GLOW`] (created green, deleted red, changed amber).
    pub fn observe_history(&mut self, store: &Store, now: Instant) -> bool {
        let revision = store.revision();
        if revision <= self.seen_revision {
            return false;
        }
        let mut changed = false;
        for txn in store
            .history()
            .iter()
            .filter(|t| t.result > self.seen_revision)
        {
            if !matches!(txn.author, Author::Agent(_)) || txn.reverts.is_some() {
                continue;
            }
            self.recent.push((txn.id.clone(), now));
            self.seen_at.insert(txn.id.clone(), now);
            self.last_agent = Some(now);
            changed = true;
        }
        self.seen_revision = revision;
        changed
    }

    /// Raises or refreshes an "agent is viewing" notice when an agent's scene, camera
    /// or selection changed and the human is not following it.
    pub fn observe_presence(
        &mut self,
        state: &EditorState,
        describe: impl Fn(&str, &[String]) -> String,
        now: Instant,
    ) {
        for (agent, presence) in &state.presence {
            let view = (
                presence.doc.clone(),
                presence.selection.clone(),
                presence.camera,
            );
            if self.views.get(agent) == Some(&view) {
                continue;
            }
            self.views.insert(agent.clone(), view);
            if state.follow.as_ref() == Some(agent) {
                continue;
            }
            let text = format!(
                "{} is viewing {}",
                short_author(&presence.author),
                describe(&presence.doc, &presence.selection)
            );
            self.raise(
                NoticeKind::AgentView {
                    agent: agent.clone(),
                },
                text,
                now,
            );
        }
        self.views
            .retain(|agent, _| state.presence.contains_key(agent));
    }

    /// Adds a notice, replacing an existing one of the same agent view.
    pub fn raise(&mut self, kind: NoticeKind, text: String, now: Instant) {
        if let Some(n) = self
            .notices
            .iter_mut()
            .find(|n| n.kind == kind && matches!(kind, NoticeKind::AgentView { .. }))
        {
            n.text = text;
            n.at = now;
            return;
        }
        self.next_notice += 1;
        self.notices.push(Notice {
            id: self.next_notice,
            kind,
            text,
            at: now,
        });
        // Keep the stack short: the oldest go first.
        if self.notices.len() > 4 {
            self.notices.remove(0);
        }
    }

    /// Removes a notice (its close button, or after View/Follow).
    pub fn dismiss(&mut self, id: u64) {
        self.notices.retain(|n| n.id != id);
    }

    /// Drops expired notices and glows; true when something disappeared.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.notices.len() + self.recent.len();
        self.notices.retain(|n| now.duration_since(n.at) < NOTICE);
        self.recent
            .retain(|(.., at)| now.duration_since(*at) < GLOW);
        before != self.notices.len() + self.recent.len()
    }

    /// When this window saw an agent transaction commit (`None` for history that
    /// predates the window, such as journal replay).
    pub fn seen_at(&self, txn: &TxnId) -> Option<Instant> {
        self.seen_at.get(txn).copied()
    }

    /// Current notices, oldest first.
    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    /// Ops of agent transactions committed within [`GLOW`], oldest first.
    pub fn recent_ops<'a>(
        &'a self,
        store: &'a Store,
        now: Instant,
    ) -> impl Iterator<Item = &'a ara_doc::Op> + 'a {
        self.recent
            .iter()
            .filter(move |(_, at)| now.duration_since(*at) < GLOW)
            .filter_map(|(id, _)| store.txn(id))
            .flat_map(|t| t.ops.iter())
    }
    /// Render-ID tints of records an agent changed in `doc` within [`GLOW`].
    pub fn tints(&self, store: &Store, doc: &str, now: Instant) -> BTreeMap<u64, &'static str> {
        self.recent_ops(store, now)
            .filter(|op| op.doc().as_str() == doc)
            .map(|op| {
                (
                    crate::world::render_id(op.id().as_str()),
                    match op {
                        ara_doc::Op::Create { .. } => "success-foreground",
                        ara_doc::Op::Delete { .. } => "destructive-foreground",
                        _ => "warning-foreground",
                    },
                )
            })
            .collect()
    }

    /// True while an agent edited within [`ACTIVE`].
    pub fn agent_active(&self, now: Instant) -> bool {
        self.last_agent
            .is_some_and(|at| now.duration_since(at) < ACTIVE)
    }

    /// Whether the overwrite log is due for a poll (four times a second), and the
    /// cursor to read from.
    pub fn log_poll(&mut self, now: Instant) -> Option<Option<u64>> {
        if self
            .last_log_poll
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(250))
        {
            return None;
        }
        self.last_log_poll = Some(now);
        Some(self.log_cursor)
    }

    /// Stores the cursor returned by `logs` (the first read only sets it, so old
    /// overwrites from before this window opened are not replayed).
    pub fn logs_read(&mut self, cursor: u64, messages: Vec<String>, now: Instant) {
        if self.log_cursor.is_some() {
            for message in messages {
                self.raise(NoticeKind::Overwrite, message, now);
            }
        }
        self.log_cursor = Some(cursor);
    }
}

/// "agent:claude" → "Claude".
pub fn short_author(author: &str) -> String {
    let name = author.strip_prefix("agent:").unwrap_or(author);
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_else(|| "Agent".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_replace_per_agent_and_expire() {
        let mut activity = Activity::default();
        let now = Instant::now();
        let view = NoticeKind::AgentView { agent: "s1".into() };
        activity.raise(view.clone(), "Claude is viewing a".into(), now);
        activity.raise(view, "Claude is viewing b".into(), now);
        activity.raise(NoticeKind::Overwrite, "Overwritten".into(), now);
        assert_eq!(activity.notices().len(), 2);
        assert_eq!(activity.notices()[0].text, "Claude is viewing b");
        assert!(activity.expire(now + NOTICE + Duration::from_millis(1)));
        assert!(activity.notices().is_empty());
        assert_eq!(short_author("agent:claude"), "Claude");
    }
}
