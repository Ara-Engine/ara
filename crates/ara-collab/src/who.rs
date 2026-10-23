//! The token-efficient "who is doing what" summary agents read before planning
//! (`collab.who`, MCP `ara_session {"action": "who"}`): one line per participant,
//! then the claims they must avoid, then branch sessions.

use std::fmt::Write;

use ara_ops::Store;

use crate::branch::BranchStatus;
use crate::claims::remaining;
use crate::hub::Hub;
use crate::participant::{Kind, ParticipantId};

fn ago(now: u64, at: u64) -> String {
    let s = now.saturating_sub(at) / 1000;
    if s < 60 {
        format!("{s} s ago")
    } else {
        format!("{} min ago", s / 60)
    }
}

/// The summary for `me` (marked "you"; `None` for an observer).
pub fn summary(hub: &Hub, store: &Store, me: Option<&ParticipantId>, now: u64) -> String {
    let mut out = String::new();
    let people: Vec<_> = hub.roster().all().collect();
    let others = people.iter().filter(|p| Some(&p.id) != me).count();
    let _ = writeln!(
        out,
        "{} here at revision {}{}",
        people.len(),
        store.revision(),
        if others == 0 { " (nobody else)" } else { "" }
    );
    for p in &people {
        let kind = match p.kind {
            Kind::Human => "human",
            Kind::Agent => "agent",
        };
        let you = if Some(&p.id) == me { ", you" } else { "" };
        let mut line = format!("- {} ({kind}{you})", p.name);
        let mut parts = Vec::new();
        if let Some(pr) = hub.presence().get(&p.id) {
            if let Some(doc) = &pr.doc {
                parts.push(format!("in {doc}"));
            }
            if !pr.selection.is_empty() {
                parts.push(format!("selected {}", pr.selection.join(", ")));
            }
            if let Some(f) = &pr.following {
                parts.push(format!(
                    "following {}",
                    hub.roster().name(&ParticipantId(f.clone()))
                ));
            }
            if let Some(a) = &pr.activity {
                let why = a
                    .why
                    .as_deref()
                    .map(|w| format!(" \"{w}\""))
                    .unwrap_or_default();
                let when =
                    a.at.map(|t| format!(" {}", ago(now, t)))
                        .unwrap_or_default();
                parts.push(format!("doing {}{why}{when}", a.what));
            }
        }
        if !parts.is_empty() {
            line.push_str(": ");
            line.push_str(&parts.join("; "));
        }
        let _ = writeln!(out, "{line}");
    }
    let claims = hub.claims();
    if !claims.is_empty() {
        let _ = writeln!(
            out,
            "Claims (agents' edits there are rejected; humans are never blocked):"
        );
        for c in claims {
            let mine = if Some(&c.holder) == me {
                " (yours)"
            } else {
                ""
            };
            let why = c
                .why
                .as_deref()
                .map(|w| format!(" \"{w}\""))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "- {}: {}{mine}{why}, {}",
                c.label,
                c.name,
                remaining(c, now)
            );
        }
    }
    let live: Vec<_> = hub
        .branches()
        .filter(|b| matches!(b.status, BranchStatus::Open | BranchStatus::Submitted))
        .collect();
    if !live.is_empty() {
        let _ = writeln!(out, "Branch sessions:");
        for b in live {
            let wait = if b.blocked_by.is_empty() {
                String::new()
            } else {
                format!(", waiting on {}", b.blocked_by.join(", "))
            };
            let _ = writeln!(out, "- {} by {}: {}{wait}", b.id, b.name, b.status.name());
        }
    }
    out
}
