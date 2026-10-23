//! Sessions: who is connected, which scopes they hold, and their checkpoints.
//!
//! `session.begin` asks the [`Approval`] hook which of the requested scopes to grant;
//! the decision is stored on the session and journaled. Every other method carries
//! the session handle, and the host checks its scope before running anything.

use std::collections::BTreeMap;

use ara_ops::Author;
use ara_proto::Scope;
use serde::{Deserialize, Serialize};

/// A connection-approval request.
#[derive(Debug, Clone)]
pub struct ApprovalRequest<'a> {
    /// Client name (`claude-code`).
    pub client: &'a str,
    /// Who the session's transactions are attributed to.
    pub author: &'a Author,
    /// Requested scopes; `None` asks for the policy's default set.
    pub requested: Option<&'a [Scope]>,
    /// The session label.
    pub label: Option<&'a str>,
}

/// What the approval hook decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// Scopes granted.
    pub granted: Vec<Scope>,
    /// Who decided: `policy`, or the approving human (`human`).
    pub by: String,
}

/// Decides which scopes a new session gets. The editor installs one that asks the
/// human on first connection; headless hosts use [`DefaultPolicy`].
pub trait Approval {
    /// Decides on `request`. Scopes not granted are reported as denied.
    fn decide(&mut self, request: &ApprovalRequest<'_>) -> Decision;
}

/// Humans get what they ask for (default: everything). Agents get what they ask for
/// within [`Scope::agent_default`] (no spending, no network, no files outside the
/// project).
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultPolicy;

impl Approval for DefaultPolicy {
    fn decide(&mut self, request: &ApprovalRequest<'_>) -> Decision {
        let allowed: Vec<Scope> = match request.author {
            Author::Human => Scope::ALL.to_vec(),
            Author::Agent(_) => Scope::agent_default(),
        };
        let granted = match request.requested {
            Some(want) => allowed.into_iter().filter(|s| want.contains(s)).collect(),
            None => allowed,
        };
        Decision {
            granted,
            by: "policy".into(),
        }
    }
}

/// A point to roll back to: the store revision when it was taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    /// Checkpoint ID (`cp_...`).
    pub id: String,
    /// Store revision at the checkpoint.
    pub revision: u64,
    /// What it marks (the turn's `why` for automatic ones).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// True for the checkpoint taken automatically before an agent turn.
    pub auto: bool,
}

/// One session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Handle (`ses_...`).
    pub id: String,
    /// Client name.
    pub client: String,
    /// Transaction author.
    pub author: Author,
    /// Label, if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Granted scopes.
    pub granted: Vec<Scope>,
    /// Requested but refused scopes.
    pub denied: Vec<Scope>,
    /// Who approved it.
    pub approved_by: String,
    /// Store revision when it began.
    pub started: u64,
    /// Checkpoints, oldest first.
    #[serde(default)]
    pub checkpoints: Vec<Checkpoint>,
    /// True once `session.end` was called.
    #[serde(default)]
    pub ended: bool,
}

impl Session {
    /// True if the session holds `scope`.
    pub fn allows(&self, scope: Scope) -> bool {
        self.granted.contains(&scope)
    }

    /// The scopes as the command registry expects them.
    pub fn cmd_session(&self) -> ara_cmd::Session {
        ara_cmd::Session {
            author: self.author.clone(),
            scopes: self.granted.iter().map(|s| s.as_str().to_owned()).collect(),
        }
    }

    /// A checkpoint by ID.
    pub fn checkpoint(&self, id: &str) -> Option<&Checkpoint> {
        self.checkpoints.iter().find(|c| c.id == id)
    }
}

/// Every session the host knows, live or ended (ended ones stay for history).
#[derive(Debug, Default)]
pub struct Sessions {
    by_id: BTreeMap<String, Session>,
}

impl Sessions {
    /// Adds a session.
    pub fn insert(&mut self, s: Session) {
        self.by_id.insert(s.id.clone(), s);
    }

    /// A live session.
    pub fn live(&self, id: &str) -> Option<&Session> {
        self.by_id.get(id).filter(|s| !s.ended)
    }

    /// A session, live or ended.
    pub fn get(&self, id: &str) -> Option<&Session> {
        self.by_id.get(id)
    }

    /// A session for mutation.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.by_id.get_mut(id)
    }

    /// All sessions, by ID.
    pub fn all(&self) -> impl Iterator<Item = &Session> {
        self.by_id.values()
    }
}

/// `agent:<client>` with the client name reduced to author-safe characters.
pub fn default_author(client: &str) -> Author {
    let name: String = client
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    Author::Agent(if name.is_empty() {
        "agent".into()
    } else {
        name
    })
}
