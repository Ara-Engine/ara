//! Transaction records: who changed what, why, and against which revision.

use std::fmt;
use std::str::FromStr;

use ara_diag::{Diagnostic, codes};
use ara_doc::Op;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Who made a change: a human, or a named agent (`agent:claude`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Author {
    /// A person, through the editor or CLI.
    Human,
    /// An agent, by name.
    Agent(String),
}

impl fmt::Display for Author {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Author::Human => f.write_str("human"),
            Author::Agent(name) => write!(f, "agent:{name}"),
        }
    }
}

impl FromStr for Author {
    type Err = Diagnostic;

    fn from_str(s: &str) -> Result<Self, Diagnostic> {
        let valid_name = |n: &str| {
            !n.is_empty()
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        };
        match s {
            "human" => Ok(Author::Human),
            _ => match s.strip_prefix("agent:") {
                Some(name) if valid_name(name) => Ok(Author::Agent(name.to_owned())),
                _ => Err(Diagnostic::error(
                    codes::INVALID_AUTHOR,
                    format!("author `{s}` must be `human` or `agent:<name>`"),
                )),
            },
        }
    }
}

impl Serialize for Author {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Author {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(|e: Diagnostic| serde::de::Error::custom(e.message))
    }
}

/// A transaction ID (`tx_4k9d2m7q`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TxnId(pub String);

impl fmt::Display for TxnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the caller says about a transaction before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxnMeta {
    /// Who is making the change.
    pub author: Author,
    /// Why, in the author's words; shown in review.
    pub why: Option<String>,
    /// Short label ("entity.create, transform.set").
    pub label: Option<String>,
    /// The revision the author based the change on. `None` means "the current one".
    /// If later transactions touched the same paths, the commit is rejected.
    pub base: Option<u64>,
    /// Retrying with the same key returns the first commit instead of applying twice.
    pub idempotency_key: Option<String>,
}

impl TxnMeta {
    /// Metadata with only an author.
    pub fn by(author: Author) -> Self {
        TxnMeta {
            author,
            why: None,
            label: None,
            base: None,
            idempotency_key: None,
        }
    }
}

/// A committed transaction: one undo step (an agent turn or a human gesture).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    /// Unique ID.
    pub id: TxnId,
    /// Who made the change.
    pub author: Author,
    /// Why, if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Label, if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Revision the author based the change on.
    pub base: u64,
    /// Store revision after this transaction.
    pub result: u64,
    /// Idempotency key, if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// The transaction this one undoes, if it is an undo (or a redo, which undoes an undo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverts: Option<TxnId>,
    /// What changed, in order.
    pub ops: Vec<Op>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authors_parse_and_print() {
        for text in ["human", "agent:claude", "agent:codex-1.2"] {
            let a: Author = text.parse().expect("valid author");
            assert_eq!(a.to_string(), text);
        }
        for bad in ["agent:", "robot", "agent:a b"] {
            assert_eq!(
                bad.parse::<Author>().expect_err(bad).code,
                codes::INVALID_AUTHOR
            );
        }
    }
}
