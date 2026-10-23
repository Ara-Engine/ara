//! Token-budgeted summaries of store records, for agents (`ara_logs`): records are
//! grouped by code and severity, with the subjects they are about, the newest
//! message and location, and the best fix. Agents pay per token, so a summary of
//! 400 repeated warnings is one line, not 400.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::store::{Record, rank};
use crate::{Code, Fix, Severity, catalog};

/// Records grouped by code and severity, errors first.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Error records.
    pub errors: usize,
    /// Warning records.
    pub warnings: usize,
    /// Info records.
    pub info: usize,
    /// Occurrences across all records (repeats included).
    pub occurrences: u64,
    /// One group per code and severity, most severe and most frequent first.
    pub groups: Vec<Group>,
}

/// Every record of one code and severity.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// The code.
    pub code: Code,
    /// The severity.
    pub severity: Severity,
    /// The code's title from the catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<&'static str>,
    /// Distinct records.
    pub records: usize,
    /// Occurrences.
    pub count: u64,
    /// The subjects most often involved (`entity:e_gen001`), most frequent first.
    pub subjects: Vec<String>,
    /// Subjects left out of `subjects`.
    #[serde(skip_serializing_if = "is_zero")]
    pub more_subjects: usize,
    /// The newest record's message.
    pub latest: String,
    /// The newest record's sequence number.
    pub latest_seq: u64,
    /// The newest record's location, rendered.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub location: String,
    /// The first fix of the newest record that has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Groups `records`, keeping up to `top_subjects` subjects per group.
pub fn summarize(records: &[&Record], top_subjects: usize) -> Summary {
    let mut summary = Summary {
        errors: 0,
        warnings: 0,
        info: 0,
        occurrences: 0,
        groups: Vec::new(),
    };
    // Per (code, severity): its records and how often each subject occurs.
    type Bucket<'r> = (Vec<&'r Record>, BTreeMap<String, u32>);
    let mut groups: BTreeMap<(Code, u8), Bucket<'_>> = BTreeMap::new();
    for r in records {
        let d = &r.diagnostic;
        match d.severity {
            Severity::Error => summary.errors += 1,
            Severity::Warning => summary.warnings += 1,
            Severity::Info => summary.info += 1,
        }
        summary.occurrences += u64::from(r.count);
        let (list, subjects) = groups.entry((d.code, rank(d.severity))).or_default();
        list.push(r);
        for s in r.subjects() {
            *subjects.entry(s.to_string()).or_default() += r.count;
        }
    }
    for ((code, _), (mut list, subjects)) in groups {
        list.sort_by_key(|r| r.seq);
        let Some(newest) = list.last() else {
            continue;
        };
        let mut ranked: Vec<(String, u32)> = subjects.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let more = ranked.len().saturating_sub(top_subjects);
        summary.groups.push(Group {
            code,
            severity: newest.diagnostic.severity,
            title: catalog::get(code).map(|d| d.title),
            records: list.len(),
            count: list.iter().map(|r| u64::from(r.count)).sum(),
            subjects: ranked
                .into_iter()
                .take(top_subjects)
                .map(|(s, _)| s)
                .collect(),
            more_subjects: more,
            latest: newest.diagnostic.message.clone(),
            latest_seq: newest.seq,
            location: newest.diagnostic.location.to_string(),
            fix: list
                .iter()
                .rev()
                .find_map(|r| r.diagnostic.fixes.first().cloned()),
        });
    }
    summary.groups.sort_by(|a, b| {
        rank(b.severity)
            .cmp(&rank(a.severity))
            .then(b.count.cmp(&a.count))
            .then(b.latest_seq.cmp(&a.latest_seq))
    });
    summary
}

impl Summary {
    /// The headline: `2 errors · 1 warning · 3 info (9 occurrences)`.
    pub fn headline(&self) -> String {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        format!(
            "{} · {} · {} info ({} occurrences)",
            plural(self.errors, "error", "errors"),
            plural(self.warnings, "warning", "warnings"),
            self.info,
            self.occurrences
        )
    }

    /// The summary as text within about `budget` characters (at least the headline
    /// and one group). Groups that do not fit are counted on the last line.
    pub fn text(&self, budget: usize) -> String {
        let mut out = format!("{}\n", self.headline());
        let mut shown = 0;
        for g in &self.groups {
            let block = g.text();
            if shown > 0 && out.len() + block.len() > budget {
                break;
            }
            out.push_str(&block);
            shown += 1;
        }
        let left = self.groups.len() - shown;
        if left > 0 {
            out.push_str(&format!(
                "… {left} more group{} (filter by `codes` or raise `budget`)\n",
                if left == 1 { "" } else { "s" }
            ));
        }
        out
    }
}

impl Group {
    /// The group as a few lines of text.
    pub fn text(&self) -> String {
        let mut s = format!("{} {}", self.severity, self.code);
        if let Some(t) = self.title {
            s.push_str(&format!(" {t}"));
        }
        if self.count > 1 {
            s.push_str(&format!(" ×{}", self.count));
        }
        if !self.subjects.is_empty() {
            s.push_str(&format!(" [{}", self.subjects.join(" ")));
            if self.more_subjects > 0 {
                s.push_str(&format!(" +{} more", self.more_subjects));
            }
            s.push(']');
        }
        s.push_str(&format!("\n  {}\n", self.latest));
        if !self.location.is_empty() {
            s.push_str(&format!("  --> {}\n", self.location));
        }
        if let Some(fix) = &self.fix {
            s.push_str(&format!("  fix: {}\n", fix.title));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Meta;
    use crate::store::{Filter, Store};
    use crate::{Diagnostic, FixAction, codes};

    #[test]
    fn groups_by_code_with_subjects_and_the_newest_fix() {
        let mut s = Store::default();
        for e in ["e_gen001", "e_ylamp1", "e_gen001"] {
            s.push(
                Diagnostic::error(
                    codes::UNRESOLVED_REF,
                    format!("`{e}` mesh does not resolve"),
                )
                .record(e)
                .with_fix(
                    "Use `@meshes/generator.glb#ms_genr0001`",
                    FixAction::Set {
                        record: Some(e.into()),
                        path: "MeshRenderer.mesh".into(),
                        value: "@meshes/generator.glb#ms_genr0001".into(),
                    },
                ),
                Meta::default(),
            );
        }
        for i in 0..400 {
            s.push(
                Diagnostic::info(codes::REF_FILLED, format!("filled in ID {i}")),
                Meta::default().dedup("filled"),
            );
        }
        let records = s.query(&Filter::default());
        let summary = summarize(&records, 3);
        assert_eq!(
            (summary.errors, summary.info, summary.occurrences),
            (1 + 1, 1, 403)
        );
        let g = &summary.groups[0];
        assert_eq!((g.code, g.count, g.records), (codes::UNRESOLVED_REF, 3, 2));
        assert_eq!(g.subjects[0], "entity:e_gen001");
        assert!(g.fix.is_some());
        let text = summary.text(4000);
        assert!(
            text.starts_with("2 errors · 0 warnings · 1 info (403 occurrences)"),
            "{text}"
        );
        assert!(
            text.contains(
                "error ARA1302 Unresolved reference ×3 [entity:e_gen001 entity:e_ylamp1]"
            ),
            "{text}"
        );
        assert!(
            text.contains("info ARA1304"),
            "the 400 notes are one group:\n{text}"
        );
        let tight = summary.text(10);
        assert!(tight.contains("… 1 more group"), "{tight}");
    }
}
