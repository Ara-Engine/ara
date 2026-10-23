//! Legacy advisory claims. They convey presence and never gate any edit or merge.
use ara_diag::Diagnostic;
use ara_doc::{IdGen, Op};
use ara_ops::Author;
use serde::{Deserialize, Serialize};

use crate::participant::{Kind, Participant, ParticipantId};
use crate::subject::Subject;

/// Default claim lifetime without heartbeats.
pub const DEFAULT_TTL_MS: u64 = 60_000;
/// Longest claim lifetime a request may ask for.
pub const MAX_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// One claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    /// Claim ID (`cl_…`).
    pub id: String,
    /// What is claimed.
    pub subject: Subject,
    /// How to show it: `levels/outpost.scene#e_door01 (Gate Door)`.
    pub label: String,
    /// The claimant.
    pub holder: ParticipantId,
    /// The claimant's author.
    pub author: Author,
    /// The claimant's display name.
    pub name: String,
    /// Human (advisory) or agent.
    pub kind: Kind,
    /// Why, in the claimant's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// When it was taken (ms).
    pub since: u64,
    /// Lifetime after the last heartbeat (ms).
    pub ttl: u64,
    /// When it ends unless a heartbeat extends it (ms).
    pub expires: u64,
}

/// A subject someone asks to claim, with its display label.
#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
    /// The subject.
    pub subject: Subject,
    /// How to show it.
    pub label: String,
}

/// What a claim request got.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Granted {
    /// The claims now held (new or refreshed).
    pub claims: Vec<Claim>,
    /// Other participants' overlapping claims (humans claim over them; advisory).
    pub overlaps: Vec<Claim>,
}

/// Every live claim.
#[derive(Debug, Default, Clone)]
pub struct Claims {
    list: Vec<Claim>,
}

impl Claims {
    /// Record advisory subjects and report overlaps without rejecting anyone.
    pub fn request(
        &mut self,
        holder: &Participant,
        wanted: Vec<Wanted>,
        ttl: u64,
        why: Option<String>,
        now: u64,
        ids: &mut IdGen,
    ) -> Result<Granted, Vec<Diagnostic>> {
        let ttl = ttl.clamp(1, MAX_TTL_MS);
        let mut overlaps: Vec<Claim> = Vec::new();
        for w in &wanted {
            for c in self.list.iter().filter(|c| c.holder != holder.id) {
                if c.subject.overlaps(&w.subject) && !overlaps.iter().any(|o| o.id == c.id) {
                    overlaps.push(c.clone());
                }
            }
        }
        let mut granted = Vec::new();
        for w in wanted {
            let existing = self
                .list
                .iter_mut()
                .find(|c| c.holder == holder.id && c.subject == w.subject);
            let claim = match existing {
                Some(c) => {
                    c.ttl = ttl;
                    c.expires = now + ttl;
                    if why.is_some() {
                        c.why = why.clone();
                    }
                    c.clone()
                }
                None => {
                    let c = Claim {
                        id: ids.token("cl", 8),
                        subject: w.subject,
                        label: w.label,
                        holder: holder.id.clone(),
                        author: holder.author.clone(),
                        name: holder.name.clone(),
                        kind: holder.kind,
                        why: why.clone(),
                        since: now,
                        ttl,
                        expires: now + ttl,
                    };
                    self.list.push(c.clone());
                    c
                }
            };
            granted.push(claim);
        }
        Ok(Granted {
            claims: granted,
            overlaps,
        })
    }

    /// Releases `holder`'s claims with these IDs (all of them for `None`). Returns
    /// what was released.
    pub fn release(&mut self, holder: &ParticipantId, ids: Option<&[String]>) -> Vec<Claim> {
        let (gone, kept) = std::mem::take(&mut self.list)
            .into_iter()
            .partition(|c| &c.holder == holder && ids.is_none_or(|ids| ids.contains(&c.id)));
        self.list = kept;
        gone
    }

    /// Extends every claim of `holder` by its lifetime from `now`. Returns how many.
    pub fn heartbeat(&mut self, holder: &ParticipantId, now: u64) -> usize {
        let mut n = 0;
        for c in self.list.iter_mut().filter(|c| &c.holder == holder) {
            c.expires = now + c.ttl;
            n += 1;
        }
        n
    }

    /// Drops claims that ended by `now`; returns them.
    pub fn expire(&mut self, now: u64) -> Vec<Claim> {
        let (gone, kept) = std::mem::take(&mut self.list)
            .into_iter()
            .partition(|c| c.expires <= now);
        self.list = kept;
        gone
    }

    /// Every live claim, oldest first.
    pub fn all(&self) -> &[Claim] {
        &self.list
    }

    /// Restores claims (persisted across a host restart).
    pub fn restore(&mut self, claims: Vec<Claim>) {
        self.list = claims;
    }

    /// Claims by others than `me` that `op` would touch.
    pub fn blocking<'a>(
        &'a self,
        me: &'a ParticipantId,
        op: &'a Op,
    ) -> impl Iterator<Item = &'a Claim> {
        self.list
            .iter()
            .filter(move |c| &c.holder != me && c.subject.touched_by(op))
    }
}

/// "about 4 min left".
pub fn remaining(claim: &Claim, now: u64) -> String {
    let left = claim.expires.saturating_sub(now);
    if left >= 60_000 {
        format!("about {} min left", left.div_ceil(60_000))
    } else {
        format!("{} s left", left.div_ceil(1000))
    }
}
