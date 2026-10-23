//! Participants: who is connected (a human or an agent), their display name and
//! colour. A participant is one host session; its ID is the session handle.

use std::collections::BTreeMap;
use std::fmt;

use ara_ops::Author;
use serde::{Deserialize, Serialize};

/// A participant: the session handle (`ses_…`) it joined with.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ParticipantId(pub String);

impl fmt::Display for ParticipantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ParticipantId {
    fn from(s: &str) -> Self {
        ParticipantId(s.to_owned())
    }
}

/// Human or agent, for attribution and presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// A person in an editor or the CLI.
    Human,
    /// An AI agent.
    Agent,
}

impl Kind {
    /// The kind of an author.
    pub fn of(author: &Author) -> Kind {
        match author {
            Author::Human => Kind::Human,
            Author::Agent(_) => Kind::Agent,
        }
    }
}

/// One connected participant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Participant {
    /// The session handle.
    pub id: ParticipantId,
    /// Who its transactions are attributed to.
    pub author: Author,
    /// Human or agent.
    pub kind: Kind,
    /// Display name (`Mia`, `claude-code`).
    pub name: String,
    /// Avatar colour, `#rrggbb`.
    pub color: String,
    /// When it joined (host clock, ms).
    pub joined_ms: u64,
}

/// Avatar colours, assigned in join order (cycled). Picked to read on Ara Dark and
/// Ara Light.
pub const PALETTE: [&str; 8] = [
    "#e5484d", "#3e9bf0", "#30a46c", "#f5a524", "#8e4ec6", "#12a594", "#e93d82", "#978365",
];

/// Everyone who has joined, by ID.
#[derive(Debug, Default)]
pub struct Roster {
    by_id: BTreeMap<ParticipantId, Participant>,
    joins: usize,
}

impl Roster {
    /// Adds (or refreshes) a participant. Returns it with its colour: the requested
    /// one, the one it had, or the next palette entry.
    pub fn join(
        &mut self,
        id: ParticipantId,
        author: Author,
        name: Option<String>,
        color: Option<String>,
        now: u64,
    ) -> &Participant {
        let kept = self.by_id.get(&id).map(|p| (p.color.clone(), p.joined_ms));
        let color = color.or_else(|| kept.as_ref().map(|k| k.0.clone()));
        let color = color.unwrap_or_else(|| {
            self.joins += 1;
            PALETTE[(self.joins - 1) % PALETTE.len()].to_owned()
        });
        let name = name.unwrap_or_else(|| match &author {
            Author::Human => "human".to_owned(),
            Author::Agent(n) => n.clone(),
        });
        let p = Participant {
            kind: Kind::of(&author),
            id: id.clone(),
            author,
            name,
            color,
            joined_ms: kept.map_or(now, |k| k.1),
        };
        self.by_id.insert(id.clone(), p);
        &self.by_id[&id]
    }

    /// Removes a participant.
    pub fn leave(&mut self, id: &ParticipantId) -> Option<Participant> {
        self.by_id.remove(id)
    }

    /// A participant.
    pub fn get(&self, id: &ParticipantId) -> Option<&Participant> {
        self.by_id.get(id)
    }

    /// Everyone, by ID.
    pub fn all(&self) -> impl Iterator<Item = &Participant> {
        self.by_id.values()
    }

    /// The display name of `id`, or the ID itself.
    pub fn name(&self, id: &ParticipantId) -> String {
        self.get(id)
            .map_or_else(|| id.0.clone(), |p| p.name.clone())
    }
}
