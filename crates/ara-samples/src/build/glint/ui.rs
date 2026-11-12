//! Glint's generated UI data: the field bindings (clock digits need zero-padded labels, which
//! the UI has no number format for) and the resources (font, icons). The markup and the
//! stylesheet are authored by hand in `content/ui`.
use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

/// `labels` mapping 0..count to zero-padded two-digit strings with `suffix`.
fn digits(count: u32, suffix: &str) -> Value {
    let mut map = serde_json::Map::new();
    for n in 0..count {
        map.insert(n.to_string(), json!(format!("{n:02}{suffix}")));
    }
    Value::Object(map)
}

fn text(node: &str, field: &str, template: &str) -> Value {
    json!({"node": node, "entity": "e_run", "component": "GlintRun", "field": field, "template": template})
}

fn shown(node: &str, field: &str) -> Value {
    json!({"node": node, "entity": "e_run", "component": "GlintRun", "field": field, "visibility": true})
}

fn padded(node: &str, field: &str, count: u32, suffix: &str) -> Value {
    let mut b = text(node, field, "{value}");
    b["labels"] = digits(count, suffix);
    b
}

/// Writes `bindings.json` and `resources.json` into `dir`.
pub fn write(dir: &Path, orbs: usize, gates: usize) -> Result<()> {
    let mut b = vec![
        text("t-min", "min", "{value}:"),
        padded("t-sec", "sec", 60, "."),
        padded("t-cs", "cs", 100, ""),
        shown("best", "hasBest"),
        text("b-min", "bestMin", "{value}:"),
        padded("b-sec", "bestSec", 60, "."),
        padded("b-cs", "bestCs", 100, ""),
        text("orbs", "orbs", &format!("{{value}} / {orbs}")),
        text("gates", "reached", &format!("{{value}} / {gates}")),
        text("falls", "falls", "{value}"),
        shown("keys", "isReady"),
        shown("fade-full", "fadeFull"),
        shown("fade-half", "fadeHalf"),
        shown("fade-light", "fadeLight"),
        shown("win", "finished"),
        shown("medal-gold", "isGold"),
        shown("medal-silver", "isSilver"),
        shown("medal-bronze", "isBronze"),
        text("w-min", "min", "{value}:"),
        padded("w-sec", "sec", 60, "."),
        padded("w-cs", "cs", 100, ""),
        text("win-orbs", "orbs", &format!("{{value}} / {orbs}")),
        text("win-falls", "falls", "{value}"),
    ];
    for n in 1..=3 {
        b.push(text(&format!("s{n}-min"), &format!("s{n}Min"), "{value}:"));
        b.push(padded(&format!("s{n}-sec"), &format!("s{n}Sec"), 60, "."));
        b.push(padded(&format!("s{n}-cs"), &format!("s{n}Cs"), 100, ""));
    }
    std::fs::write(
        dir.join("bindings.json"),
        ara_json::to_canonical_string(&Value::Array(b)),
    )?;
    let resources = json!({
        "fonts": ["third_party/outpost_ui/Font/Kenney Future.ttf"],
        "images": {
            "orb": "ui/icons/orb.png", "flag": "ui/icons/flag.png", "fall": "ui/icons/fall.png",
            "medal_gold": "ui/icons/medal_gold.png", "medal_silver": "ui/icons/medal_silver.png", "medal_bronze": "ui/icons/medal_bronze.png",
        },
    });
    std::fs::write(
        dir.join("resources.json"),
        ara_json::to_canonical_string(&resources),
    )?;
    Ok(())
}
