//! Claims and soft locks: a participant claims subjects for a turn, with an expiry
//! that heartbeats extend.
//!
//! - **Humans:** claims are advisory. A human's edit is never blocked, and a human
//!   may claim over someone else's claim (the overlap is reported).
//! - **Agents:** claims are enforced. [`ClaimGuard`] (an `ara_ops::Guard`) rejects
//!   an agent transaction touching a subject someone else claims, with `ARA2500`
//!   naming the claimant and what to do instead. An agent asking for a subject
//!   someone else holds gets `ARA2501`.

use ara_diag::{Diagnostic, FixAction};
use ara_doc::{IdGen, Op};
use ara_ops::{Author, Guard, TxnMeta};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::codes;
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
    /// Claims every subject in `wanted` for `holder`, all or nothing. Re-claiming a
    /// subject one already holds refreshes it. Agents fail with `ARA2501` on subjects
    /// others hold; humans get them anyway, with the overlaps listed.
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
        if holder.kind == Kind::Agent && !overlaps.is_empty() {
            return Err(overlaps
                .iter()
                .map(|c| held(c, now, "you cannot claim it while they hold it"))
                .collect());
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

    /// The enforcement snapshot to install on the store, as of `now`.
    pub fn guard(&self, now: u64) -> ClaimGuard {
        ClaimGuard {
            claims: self.list.clone(),
            now,
        }
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

fn who(c: &Claim) -> String {
    let kind = match c.kind {
        Kind::Human => "human",
        Kind::Agent => "agent",
    };
    let why = c
        .why
        .as_deref()
        .map(|w| format!(" for \"{w}\""))
        .unwrap_or_default();
    format!("{} ({kind}){why}", c.name)
}

/// `ARA2501`: an agent asked for a subject `c` covers.
fn held(c: &Claim, now: u64, tail: &str) -> Diagnostic {
    locate(
        Diagnostic::error(
            codes::CLAIM_HELD,
            format!(
                "`{}` is claimed by {} ({}); {tail}",
                c.label,
                who(c),
                remaining(c, now)
            ),
        ),
        c,
    )
}

/// `ARA2500`: an agent edit touched what `c` covers. Names the claimant and offers
/// the ways forward.
pub fn claimed(c: &Claim, now: u64) -> Diagnostic {
    let message = format!(
        "`{}` is claimed by {} ({}), and agents cannot edit claimed subjects, so nothing in this batch was applied. Leave it out and apply the rest of your changes now; for the claimed part only, wait for the claim to end, ask {} to release it, or queue it in a branch session (`branch.open`, `ara_apply` with `branch`, then `submit`), which merges once the claim ends",
        c.label,
        who(c),
        remaining(c, now),
        c.name
    );
    locate(Diagnostic::error(codes::CLAIMED, message), c)
        .with_fix(
            "See who is working on what (ara_session who)",
            FixAction::Call {
                method: "collab.who".into(),
                params: json!({}),
            },
        )
        .with_fix(
            "Queue just the claimed part in a branch session (ara_session branch)",
            FixAction::Call {
                method: "branch.open".into(),
                params: json!({ "why": "<this change>" }),
            },
        )
}

fn locate(d: Diagnostic, c: &Claim) -> Diagnostic {
    let d = d.data(json!({ "claim": c }));
    match &c.subject {
        Subject::Doc { doc } => d.doc(doc.as_str()),
        Subject::Entity { doc, entity } => d.doc(doc.as_str()).record(entity.as_str()),
        Subject::Field { doc, entity, path } => d
            .doc(doc.as_str())
            .record(entity.as_str())
            .path(path.to_string()),
        Subject::Asset { path } => d.file(path.trim_start_matches('@')),
    }
}

/// Enforces claims for agents at commit time: a snapshot of the claims, installed on
/// the store with `Store::set_guard` whenever they change.
///
/// A transaction is the claimant's own when the store's origin is the claim's
/// session; with no origin, when the authors match. Humans are never blocked.
#[derive(Debug, Clone)]
pub struct ClaimGuard {
    claims: Vec<Claim>,
    now: u64,
}

impl Guard for ClaimGuard {
    fn check(&self, meta: &TxnMeta, origin: Option<&str>, ops: &[Op]) -> Vec<Diagnostic> {
        if !matches!(meta.author, Author::Agent(_)) {
            return Vec::new();
        }
        let own = |c: &Claim| match origin {
            Some(o) => c.holder.0 == o,
            None => c.author == meta.author,
        };
        self.claims
            .iter()
            .filter(|c| !own(c) && c.expires > self.now)
            .filter(|c| ops.iter().any(|op| c.subject.touched_by(op)))
            .map(|c| claimed(c, self.now))
            .collect()
    }
}
