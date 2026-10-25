//! The self-contained `report.html`: one table, a pass/fail column per gate.
use serde_json::Value;

use super::rows::{DEFINITIONS, Row, Status, Summary};

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn pill(status: Status) -> &'static str {
    match status {
        Status::Pass => "<span class='pill pass'>pass</span>",
        Status::Fail => "<span class='pill fail'>fail</span>",
        Status::NotMeasured => "<span class='pill none'>not measured</span>",
        Status::Reported => "<span class='pill info'>reported</span>",
    }
}

fn number(value: f64) -> String {
    if value.abs() >= 100. || value.fract() == 0. {
        format!("{value:.0}")
    } else if value.abs() >= 0.1 {
        format!("{value:.2}")
    } else {
        format!("{value:.4}")
    }
}

const STYLE: &str = "
:root { color-scheme: dark; --bg:#111317; --panel:#181b21; --line:#262a33; --fg:#e6e8ec; --muted:#8b93a1; }
* { box-sizing: border-box; }
body { margin: 0; padding: 32px 40px 64px; background: var(--bg); color: var(--fg); font: 14px/1.45 system-ui, -apple-system, 'Segoe UI', sans-serif; }
h1 { font-size: 22px; margin: 0 0 4px; } h2 { font-size: 15px; margin: 32px 0 8px; color: var(--muted); font-weight: 600; }
.meta { color: var(--muted); margin-bottom: 20px; } .meta code { color: var(--fg); }
.cards { display: flex; gap: 12px; flex-wrap: wrap; margin-bottom: 8px; }
.card { background: var(--panel); border: 1px solid var(--line); border-radius: 10px; padding: 12px 16px; min-width: 220px; }
.card .gate { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: .06em; }
.card .word { font-size: 22px; font-weight: 700; margin: 2px 0; }
.word.PASS { color: #4ade80; } .word.FAIL { color: #f87171; } .word.INCOMPLETE { color: #fbbf24; }
table { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--line); border-radius: 10px; overflow: hidden; }
th, td { padding: 6px 10px; border-bottom: 1px solid var(--line); text-align: left; vertical-align: top; }
th { color: var(--muted); font-weight: 600; font-size: 12px; background: #1c2027; position: sticky; top: 0; }
td.num { text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }
tr.section td { background: #1f242c; font-weight: 600; }
td.note { color: var(--muted); font-size: 12px; max-width: 520px; }
td.note a { color: var(--muted); }
.pill { display: inline-block; padding: 1px 8px; border-radius: 999px; font-size: 12px; font-weight: 600; white-space: nowrap; }
.pass { background: #14532d; color: #bbf7d0; } .fail { background: #7f1d1d; color: #fecaca; }
.none { background: #374151; color: #d1d5db; } .info { background: #1e3a5f; color: #bfdbfe; }
.limit { color: var(--muted); font-size: 12px; }
dl { display: grid; grid-template-columns: 160px 1fr; gap: 6px 16px; color: var(--muted); font-size: 13px; }
dt { color: var(--fg); font-weight: 600; }
";

/// The whole page, from the report and its rows.
pub(super) fn page(report: &Value, rows: &[Row], summary: &Summary) -> String {
    let gates: Vec<&str> = summary.gates.keys().copied().collect();
    let run = &report["run"];
    let mut out = format!(
        "<!doctype html><html lang='en'><head><meta charset='utf-8'><title>Ara D26 scoreboard {}</title><style>{STYLE}</style></head><body>",
        escape(run["createdAt"].as_str().unwrap_or(""))
    );
    out.push_str("<h1>Ara D26 scoreboard</h1>");
    out.push_str(&format!(
        "<div class='meta'><code>{}</code>{} · {} profile · {} · {} frames per window, warmup {} · {} · {} UTC</div>",
        escape(&run["sourceHead"].as_str().unwrap_or("").chars().take(10).collect::<String>()),
        if run["sourceDirty"] == true { " (dirty)" } else { "" },
        escape(run["profile"].as_str().unwrap_or("")),
        escape(run["resolution"].as_str().unwrap_or("")),
        run["frames"],
        run["warmup"],
        escape(report["device"]["name"].as_str().unwrap_or("GPU not reported")),
        escape(run["createdAt"].as_str().unwrap_or("")),
    ));
    out.push_str("<div class='cards'>");
    for (gate, counts) in &summary.gates {
        let word = Summary::word(counts);
        out.push_str(&format!(
            "<div class='card'><div class='gate'>{gate}</div><div class='word {word}'>{word}</div><div>{} pass · {} fail · {} not measured</div></div>",
            counts.pass, counts.fail, counts.not_measured
        ));
    }
    out.push_str("</div>");
    if let Some(skipped) = report["skipped"].as_object().filter(|s| !s.is_empty()) {
        for (section, reason) in skipped {
            out.push_str(&format!(
                "<div class='meta'>{} section not measured: {}</div>",
                escape(section),
                escape(reason.as_str().unwrap_or(""))
            ));
        }
    }
    out.push_str("<table><thead><tr><th>Subject</th><th>Metric</th><th>Value</th>");
    for gate in &gates {
        out.push_str(&format!("<th>{gate}</th>"));
    }
    out.push_str("<th>Reported</th><th>Note</th></tr></thead><tbody>");
    let columns = 5 + gates.len();
    let mut section = "";
    for row in rows {
        if row.section != section {
            section = row.section;
            out.push_str(&format!(
                "<tr class='section'><td colspan='{columns}'>{}</td></tr>",
                escape(section)
            ));
        }
        let value = row.value.map_or("—".into(), |v| {
            format!(
                "{}{} {}",
                if row.bound { "≤ " } else { "" },
                number(v),
                row.unit
            )
        });
        out.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td class='num'>{}</td>",
            escape(&row.subject),
            escape(&row.metric),
            escape(&value)
        ));
        for gate in &gates {
            match row.gates.iter().find(|g| g.gate == *gate) {
                Some(judged) => out.push_str(&format!(
                    "<td>{} <span class='limit'>≤ {} {}</span></td>",
                    pill(judged.status),
                    number(judged.limit),
                    escape(row.unit)
                )),
                None => out.push_str("<td class='limit'>—</td>"),
            }
        }
        let source = row.source.as_ref().map_or(String::new(), |source| {
            format!(
                " · <a href='file:///{}'>source</a>",
                escape(&source.replace('\\', "/"))
            )
        });
        // Gated rows read per gate; the column says whether an ungated row was measured.
        out.push_str(&format!(
            "<td>{}</td><td class='note'>{}{source}</td></tr>",
            if row.gates.is_empty() {
                pill(row.status)
            } else {
                ""
            },
            escape(&row.note)
        ));
    }
    out.push_str("</tbody></table><h2>Definitions</h2><dl>");
    for (term, text) in DEFINITIONS {
        out.push_str(&format!(
            "<dt>{}</dt><dd>{}</dd>",
            escape(term),
            escape(text)
        ));
    }
    out.push_str("</dl></body></html>");
    out
}
