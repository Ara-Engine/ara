//! Glint's generated UI: the markup (`game.ui`), the field bindings and the resources (font,
//! icons, the sound and music routes). Only the stylesheet (`game.css`) is authored by hand.
//!
//! The markup is generated because the UI cannot draw two things a HUD over a changing sky
//! needs, so each is built from plain elements here:
//! - an edge: every HUD text is two `text` nodes (a white twin offset behind a navy one), each
//!   with the same binding, since there is no `text-shadow` or stroke;
//! - clock digits: the running clock is one fixed-width cell per digit (the font's figures are
//!   not tabular), each bound to its own field; the other times use a `labels` map per field
//!   for zero padding, since there is no number format.
use super::sounds;
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

/// The same binding for an outlined text and its twin.
fn outlined(binding: Value) -> Vec<Value> {
    let mut twin = binding.clone();
    twin["node"] = json!(format!("{}-a", binding["node"].as_str().expect("a node")));
    vec![binding, twin]
}

/// An outlined text: a white twin drawn first, offset down and right (2 px under large
/// text, 1 px under small), then the navy or accent text over it.
fn outline(id: &str, class: &str, text: &str) -> String {
    let twin = if matches!(class, "tm" | "tc" | "ct") {
        "ola"
    } else {
        "ols"
    };
    format!(
        r#"<panel id="{id}-box" class="ol"><text id="{id}-a" class="{class} {twin}" text="{text}"/><text id="{id}" class="{class}" text="{text}"/></panel>"#
    )
}

/// One clock digit in a fixed-width cell, so the time does not shift as digits change (the
/// font's figures are not tabular).
fn digit(id: &str, class: &str, text: &str) -> String {
    let cell = if class == "tm" { "dg" } else { "dgs" };
    format!(
        r#"<panel id="{id}-cell" class="{cell}">{}</panel>"#,
        outline(id, class, text)
    )
}

fn key(id: &str, label: &str, wide: bool) -> String {
    let class = if wide { "key keywide" } else { "key" };
    format!(
        r#"<panel id="key-{id}" class="{class}"><text id="key-{id}-t" class="kt" text="{label}"/></panel>"#
    )
}

/// A medal time as `m:ss`.
fn clock(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// The whole UI tree.
fn markup(medals: [u32; 3]) -> String {
    let counter = |id: &str, icon: &str, text: &str| {
        format!(
            r#"<panel id="{id}-row" class="counter row"><image class="ci" id="{id}-icon" src="{icon}"/>{}</panel>"#,
            outline(id, "ct", text)
        )
    };
    let split = |n: u32| {
        format!(
            r#"<panel id="split-{n}" class="split row"><image id="split-{n}-icon" class="si" src="flag"/><text class="st" id="split-{n}-label" text="GATE {n}"/><panel id="split-{n}-time" class="row"><text class="st" id="s{n}-min" text="0:"/><text class="st" id="s{n}-sec" text="00."/><text class="st" id="s{n}-cs" text="00"/></panel></panel>"#
        )
    };
    // One medal's time on the results; `tier-<id>-mark` shows under the one that was earned.
    let tier = |id: &str, seconds: u32| {
        format!(
            r#"<panel id="tier-{id}" class="tier"><panel id="tier-{id}-row" class="row tierrow"><image id="tier-{id}-icon" class="ti" src="medal_{id}"/><text id="tier-{id}-time" class="tt" text="{}"/></panel><panel id="tier-{id}-mark" class="tiermark"/></panel>"#,
            clock(seconds)
        )
    };
    let medal = |id: &str, name: &str, seconds: u32| {
        format!(
            r#"<panel id="lobby-{id}" class="medal row"><image id="lobby-{id}-icon" class="mi" src="medal_{id}"/><text id="lobby-{id}-name" class="medal-name" text="{name}"/><text id="lobby-{id}-time" class="medal-time" text="{}"/></panel>"#,
            clock(seconds)
        )
    };
    [
        r#"<!-- ara:asset ui_glint000000001 -->"#.to_owned(),
        r#"<ui id="glint">"#.to_owned(),
        r#"<panel id="hud">"#.to_owned(),
        // The clock: the course name, the time, the next medal's time, the best time.
        format!(
            r#"<panel id="clock">{}<panel id="time" class="row">{}</panel><panel id="target" class="row">{}</panel><panel id="best" class="row">{}{}{}{}</panel></panel>"#,
            outline("tag", "tag", "SKYWARD"),
            [
                digit("t-min", "tm", "0"),
                outline("t-colon", "tm", ":"),
                digit("t-s1", "tm", "0"),
                digit("t-s2", "tm", "0"),
                outline("t-dot", "tm", "."),
                digit("t-c1", "tc", "0"),
                digit("t-c2", "tc", "0"),
            ]
            .concat(),
            outline("goal-time", "tg", "GOLD"),
            outline("best-label", "tb", "BEST"),
            outline("b-min", "tb", "0:"),
            outline("b-sec", "tb", "00."),
            outline("b-cs", "tb", "00"),
        ),
        format!(
            r#"<panel id="counters">{}{}{}</panel>"#,
            counter("orbs", "orb", "0 / 0"),
            counter("gates", "flag", "0 / 3"),
            counter("falls", "fall", "0"),
        ),
        // Key glyphs: shown until the first input, and again after a long idle.
        format!(
            r#"<panel id="keys" class="row">{}{}{}{}{}{}{}{}{}</panel>"#,
            key("w", "W", false),
            key("a", "A", false),
            key("s", "S", false),
            key("d", "D", false),
            outline("key-roll", "keylabel", "ROLL"),
            key("space", "SPACE", true),
            outline("key-jump", "keylabel", "JUMP"),
            key("e", "E", false),
            outline("key-retry", "keylabel", "RETRY"),
        ),
        r#"<panel id="fade-full" class="fade full"/><panel id="fade-half" class="fade half"/><panel id="fade-light" class="fade light"/>"#.to_owned(),
        // The results, on the navy side panel the title and course pages use: the medal and
        // the time are the headline; under them the three medal times with the earned one
        // marked, the splits, and two equal actions. The rest of the screen is the course
        // in a white frame (the camera rises and turns back over it once the run is done).
        format!(
            r#"<panel id="win" class="screen"><panel id="win-side" class="side"/><panel id="win-frame"/><panel id="win-content"><panel id="medal-box"><image class="wm" id="medal-gold" src="medal_gold"/><image class="wm" id="medal-silver" src="medal_silver"/><image class="wm" id="medal-bronze" src="medal_bronze"/></panel><text id="win-medal" text="GOLD"/><panel id="win-time" class="row"><text id="w-min" text="0:"/><text id="w-sec" text="00."/><text id="w-cs" text="00"/></panel><text id="win-title" text="COURSE CLEAR"/><panel id="tiers" class="row">{}{}{}</panel><panel id="splits">{}{}{}<panel id="split-orbs" class="split row"><image id="split-orbs-icon" class="si" src="orb"/><text class="st" id="orb-label" text="ORBS"/><text class="st" id="win-orbs" text="0 / 0"/></panel><panel id="split-falls" class="split row"><image id="split-falls-icon" class="si" src="fall"/><text class="st" id="fall-label" text="FALLS"/><text class="st" id="win-falls" text="0"/></panel></panel><panel id="again" class="row"><panel id="again-roll" class="action row">{}<text id="again-label" class="actionlabel" text="ROLL AGAIN"/></panel><button id="win-leave" action="leave" text="TITLE"/></panel></panel></panel>"#,
            tier("gold", medals[0]),
            tier("silver", medals[1]),
            tier("bronze", medals[2]),
            split(1),
            split(2),
            split(3),
            key("again", "E", false),
        ),
        r#"</panel>"#.to_owned(),
        r#"<panel id="main-menu" class="screen"><panel id="menu-content"><text id="eyebrow" text="A ROLLING COURSE ABOVE THE CLOUDS"/><panel id="logo-box"><text id="logo-shadow" class="logo" text="GLINT"/><text id="logo" class="logo" text="GLINT"/></panel><panel id="title-rule" class="row"><panel id="rule-a"/><panel id="rule-b"/><panel id="rule-c"/></panel><panel id="menu-buttons"><button id="solo" action="solo" text="ROLL"/><button id="quit" action="quit" text="QUIT"/></panel></panel></panel>"#.to_owned(),
        format!(
            r#"<panel id="lobby" class="screen"><panel id="lobby-side" class="side"/><panel id="lobby-content"><text id="lobby-eyebrow" text="COURSE 01"/><text id="lobby-title" text="SKYWARD"/><panel id="medals">{}{}{}</panel><panel id="lobby-buttons"><button id="ready" action="ready" text="START"/><button id="lobby-leave" action="leave" text="BACK"/></panel></panel></panel>"#,
            medal("gold", "GOLD", medals[0]),
            medal("silver", "SILVER", medals[1]),
            medal("bronze", "BRONZE", medals[2]),
        ),
        r#"<panel id="loading" class="screen solid"><text id="loading-title" text="LOADING"/><text id="loading-progress" text=""/></panel>"#.to_owned(),
        r#"<panel id="pause" class="scrim"><panel id="pause-content"><text id="pause-title" text="PAUSED"/><button id="resume" action="resume" text="RESUME"/><button id="pause-leave" action="leave" text="LEAVE RUN"/></panel></panel>"#.to_owned(),
        r#"</ui>"#.to_owned(),
    ]
    .join("\n")
        + "\n"
}

/// Writes `game.ui`, `bindings.json` and `resources.json` into `dir`.
pub fn write(dir: &Path, orbs: usize, gates: usize, medals: [u32; 3]) -> Result<()> {
    std::fs::write(dir.join("game.ui"), markup(medals))?;
    let mut b = Vec::new();
    // The next medal still in reach, by `GlintRun.Target` (3 gold .. 0 none).
    let mut target = text("goal-time", "target", "{value}");
    target["labels"] = json!({
        "3": format!("GOLD {}", clock(medals[0])),
        "2": format!("SILVER {}", clock(medals[1])),
        "1": format!("BRONZE {}", clock(medals[2])),
        "0": "NO MEDAL",
    });
    for binding in [
        text("t-min", "min", "{value}"),
        text("t-s1", "secTens", "{value}"),
        text("t-s2", "secOnes", "{value}"),
        text("t-c1", "csTens", "{value}"),
        text("t-c2", "csOnes", "{value}"),
        target,
        text("b-min", "bestMin", "{value}:"),
        padded("b-sec", "bestSec", 60, "."),
        padded("b-cs", "bestCs", 100, ""),
        text("orbs", "orbs", &format!("{{value}} / {orbs}")),
        text("gates", "reached", &format!("{{value}} / {gates}")),
        text("falls", "falls", "{value}"),
    ] {
        b.extend(outlined(binding));
    }
    let mut medal = text("win-medal", "medal", "{value}");
    medal["labels"] = json!({"3": "GOLD", "2": "SILVER", "1": "BRONZE", "0": "NO MEDAL"});
    b.extend([
        shown("clock", "showHud"),
        shown("counters", "showHud"),
        shown("best", "hasBest"),
        shown("keys", "showKeys"),
        shown("fade-full", "fadeFull"),
        shown("fade-half", "fadeHalf"),
        shown("fade-light", "fadeLight"),
        shown("win", "finished"),
        shown("medal-gold", "isGold"),
        shown("medal-silver", "isSilver"),
        shown("medal-bronze", "isBronze"),
        shown("tier-gold-mark", "isGold"),
        shown("tier-silver-mark", "isSilver"),
        shown("tier-bronze-mark", "isBronze"),
        medal,
        text("w-min", "min", "{value}:"),
        padded("w-sec", "sec", 60, "."),
        padded("w-cs", "cs", 100, ""),
        text("win-orbs", "orbs", &format!("{{value}} / {orbs}")),
        text("win-falls", "falls", "{value}"),
    ]);
    for n in 1..=3 {
        b.push(text(&format!("s{n}-min"), &format!("s{n}Min"), "{value}:"));
        b.push(padded(&format!("s{n}-sec"), &format!("s{n}Sec"), 60, "."));
        b.push(padded(&format!("s{n}-cs"), &format!("s{n}Cs"), 100, ""));
    }
    std::fs::write(
        dir.join("bindings.json"),
        ara_json::to_canonical_string(&Value::Array(b)),
    )?;
    let sound = |component: &str, field: &str, name: &str, gain: f32| {
        let id = sounds::CLIPS
            .iter()
            .find(|(n, _)| *n == name)
            .expect("a synthesised clip")
            .1;
        json!({"component": component, "field": field, "clip": format!("@audio/{name}.wav#{id}"), "gain": gain})
    };
    let resources = json!({
        // Counters the scripts raise; each plays its clip once per change, at the entity that
        // carries the counter (so they sit on the ball, the orb, the arch, the cannon, the camera).
        "sounds": [
            sound("Ball", "jumpTick", "jump", 0.6),
            sound("Ball", "landTick", "land", 0.8),
            sound("Pickup", "takenTick", "orb", 0.7),
            sound("Checkpoint", "reachedTick", "gate", 0.9),
            sound("Cannon", "loadTick", "load", 0.9),
            sound("Cannon", "fireTick", "fire", 1.2),
            sound("RunFeedback", "fallTick", "fall", 0.7),
            sound("RunFeedback", "finishTick", "finish", 0.9),
        ],
        "music": [{"entity": "e_run", "component": "GlintRun", "field": "intensity", "target": "e_music"}],
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
