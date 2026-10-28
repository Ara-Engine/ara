use super::{Act, fixture};
use crate::shell::Shell;
use ara_ui::{paint::Box2, source::stable_id};
use serde_json::{Value, json};
use std::path::Path;

fn text<'a>(shell: &'a Shell, id: &str) -> Option<&'a str> {
    shell
        .shown_node(id)
        .and_then(|i| shell.ui.nodes[i].text.as_ref())
        .map(|text| text.value.as_str())
}
pub(super) fn checked(shell: &Shell, id: &str) -> bool {
    shell
        .shown_node(id)
        .and_then(|i| shell.ui.nodes[i].element.attrs.get("class"))
        .is_some_and(|classes| {
            classes
                .split_whitespace()
                .any(|class| matches!(class, "selected" | "segment-on"))
        })
}
pub(super) fn menu_command(shell: &Shell, command: &str) -> Option<String> {
    shell
        .command_bindings()
        .into_iter()
        .find(|(id, binding)| {
            id.starts_with("m0-") && *binding == command && shell.shown_node(id).is_some()
        })
        .map(|(id, _)| id)
}

fn rows(shell: &Shell, panel: &str) -> Vec<Value> {
    let prefix = if panel == "console" {
        "log-"
    } else {
        "problem-"
    };
    shell.ui.nodes.iter().filter_map(|node|{
        let base=node.element.id.strip_prefix(prefix)?.strip_suffix("-text")?;
        let message=text(shell,&node.element.id)?;
        if !message.starts_with(fixture::PREFIX) {return None;}
        let id=format!("{prefix}{base}");
        let source=if panel=="console" {format!("{id}-source-label")} else {format!("{id}-source")};
        Some(json!({"id":id,"message":message,"source":text(shell,&source),"count":text(shell,&format!("{id}-count")),"rect":node.rect,"clip":node.clip}))
    }).collect()
}
pub(super) fn snapshot(shell: &Shell) -> Value {
    let controls: Vec<_> = [
        "console-collapse",
        "console-clear-play",
        "console-errors",
        "console-warnings",
        "console-info",
    ]
    .into_iter()
    .map(|id| json!({"id":id,"checked":checked(shell,id),"shown":shell.shown_node(id).is_some()}))
    .collect();
    let dialog:Vec<_>=shell.ui.nodes.iter().filter(|node|node.element.id.starts_with("dialog-")).filter_map(|node|text(shell,&node.element.id).map(|value|json!({"id":node.element.id,"text":value,"rect":node.rect,"clip":node.clip}))).collect();
    json!({"console":rows(shell,"console"),"problems":rows(shell,"problems"),"controls":controls,"dialog":dialog,"status":shell.status_note,"focus":shell.input.focused().and_then(|id|shell.ui.by_id(id)).map(|i|shell.ui.nodes[i].element.id.as_str()),"scope":shell.input.focus_graph(&shell.ui),"consoleSearch":text(shell,"search-console"),"problemsSearch":text(shell,"search-problems"),"menuClearOnPlay":menu_command(shell,"console.clear_on_play")})
}

pub(super) fn checks(
    act: Act,
    shell: &Shell,
    records: &[Value],
    clipboard: &str,
    root: &Path,
) -> Vec<Value> {
    let console = rows(shell, "console");
    let problems = rows(shell, "problems");
    let source = fixture::seq(records, fixture::SAME, fixture::FILE_A, 12).ok();
    let repair = fixture::seq(records, fixture::REPAIR, fixture::FILE_B, 20).ok();
    let same = |row: &Value| row["message"] == fixture::SAME;
    let mut checks = Vec::new();
    let mut check = |label: &str, ok: bool| checks.push(json!({"check":label,"ok":ok}));
    let count = records
        .iter()
        .filter_map(|record| record["count"].as_u64())
        .sum::<u64>();
    match act {
        Act::ClearInitial => check(
            "native Clear is reachable in the maximized Console",
            shell.shown_node("console-clear").is_some() && console.is_empty(),
        ),
        Act::Seed => {
            check(
                "real bus stores seven identities and nine occurrences",
                records.len() == 7 && count == 9,
            );
            check(
                "every seeded identity reaches the real Console",
                console.len() == 7,
            );
        }
        Act::Filter | Act::SearchAll => {
            check(
                "native search text reaches the Console model",
                text(shell, "search-console") == Some("J7 "),
            );
            check(
                "all seven fixture identities remain visible",
                console.len() == 7,
            );
        }
        Act::Collapse => {
            check(
                "Collapse is enabled through its toolbar command",
                checked(shell, "console-collapse"),
            );
            check(
                "same-code messages keep seven distinct identities",
                console.len() == 7
                    && records.iter().all(|r| {
                        r["diagnostic"]["code"] == ara_diag::codes::INVALID_PARAMS.to_string()
                    }),
            );
            check(
                "same message keeps distinct source files and lines",
                console.iter().filter(|row| same(row)).count() == 3
                    && [
                        format!("{}:12", fixture::FILE_A),
                        format!("{}:13", fixture::FILE_A),
                        format!("{}:12", fixture::FILE_B),
                    ]
                    .iter()
                    .all(|source| {
                        console
                            .iter()
                            .any(|row| row["source"].as_str() == Some(source.as_str()) && same(row))
                    }),
            );
            check(
                "repeated diagnostic shows its occurrence count",
                source.is_some_and(|seq| text(shell, &format!("log-{seq}-count")) == Some("×3")),
            );
        }
        Act::ErrorsOff => check(
            "hiding errors preserves warnings and info",
            !checked(shell, "console-errors")
                && console.len() == 3
                && console
                    .iter()
                    .all(|row| !same(row) && row["message"] != "J7 different failure"),
        ),
        Act::WarningsOff => check(
            "hiding warnings leaves only the info message",
            !checked(shell, "console-warnings")
                && console.len() == 1
                && console[0]["message"] == "J7 info only",
        ),
        Act::InfoOff => check(
            "disabling all severities hides every fixture row",
            ["console-errors", "console-warnings", "console-info"]
                .iter()
                .all(|id| !checked(shell, id))
                && console.is_empty(),
        ),
        Act::Restore => check(
            "restoring severity toggles restores all identities",
            ["console-errors", "console-warnings", "console-info"]
                .iter()
                .all(|id| checked(shell, id))
                && console.len() == 7,
        ),
        Act::Search => check(
            "message search isolates the warning",
            console.len() == 1 && console[0]["message"] == "J7 warning only",
        ),
        Act::SearchLocations => check(
            "same-message search retains all source locations",
            console.len() == 3 && console.iter().all(same),
        ),
        Act::Source | Act::ProblemSource => {
            let expected = format!("{}:12:4", root.join(fixture::FILE_A).display());
            check(
                "source click targets the exact project file, line and column",
                shell.status_note.as_deref() == Some(expected.as_str())
                    && text(shell, "status-saved") == Some(expected.as_str()),
            );
            check(
                "source label exposes the project path and line",
                if matches!(act, Act::Source) {
                    source.is_some_and(|seq| {
                        text(shell, &format!("log-{seq}-source-label"))
                            == Some(format!("{}:12", fixture::FILE_A).as_str())
                    })
                } else {
                    source.is_some_and(|seq| {
                        text(shell, &format!("problem-{seq}-source"))
                            == Some(format!("{}:12", fixture::FILE_A).as_str())
                    })
                },
            );
        }
        Act::ClearPlayOn => check(
            "toolbar enables Clear on Play",
            checked(shell, "console-clear-play"),
        ),
        Act::Context => {
            let command = menu_command(shell, "console.clear_on_play");
            check(
                "keyboard context menu exposes Clear on Play",
                command.is_some(),
            );
            check(
                "context menu reflects the checked toolbar state",
                command.is_some_and(|id| {
                    shell.ui.find(&format!("{id}-c-icon")).is_some_and(|i| {
                        shell.ui.nodes[i]
                            .element
                            .attrs
                            .get("src")
                            .map(String::as_str)
                            == Some("icon-check")
                    })
                }),
            );
            check(
                "Console context menu stays viewport bounded",
                bounded(shell, "menu-pop-0"),
            );
        }
        Act::ClearPlayOff => check(
            "the context command toggles the same Clear on Play state",
            !checked(shell, "console-clear-play") && shell.menu.is_none(),
        ),
        Act::Clear => {
            check("Clear hides old Console messages", console.is_empty());
            check(
                "Clear preserves diagnostics on the bus",
                records.len() == 7 && count == 9,
            );
        }
        Act::NewMessage => {
            check(
                "new messages remain reachable after Clear",
                console.len() == 1 && console[0]["message"] == "J7 message after clear",
            );
            check(
                "bus retains old records alongside the new message",
                records.len() == 8 && count == 10,
            );
        }
        Act::Problems => check(
            "Problems retains same-code failures at three locations",
            problems.len() == 3 && problems.iter().all(same),
        ),
        Act::Why => {
            let doc = ara_diag::codes::INVALID_PARAMS.doc();
            check(
                "Why opens the catalog explanation on the correct diagnostic",
                source.zip(doc).is_some_and(|(seq, doc)| {
                    text(shell, &format!("problem-{seq}-title")) == Some(doc.title)
                        && text(shell, &format!("problem-{seq}-explain")) == Some(doc.explanation)
                }),
            );
        }
        Act::RepairSearch => check(
            "repair fix is reachable from the actual diagnostic",
            problems.len() == 1
                && problems[0]["message"] == fixture::REPAIR
                && repair
                    .is_some_and(|seq| shell.shown_node(&format!("problem-{seq}-fix-0")).is_some()),
        ),
        Act::RepairReview => {
            check(
                "repair opens a reviewable command with exact argument quoting",
                text(shell, "dialog-code") == Some(fixture::command())
                    && text(shell, "dialog-title") == Some("Review engine repair command"),
            );
            check(
                "pinned project close prerequisite is explicit",
                text(shell, "dialog-line-0").is_some_and(|line| {
                    line.contains("Save your work") && line.contains("close this pinned project")
                }),
            );
            check(
                "Copy and Close controls are reachable inside a bounded modal",
                shell.shown_node("dialog-copy").is_some()
                    && shell.shown_node("dialog-close").is_some()
                    && bounded(shell, "dialog-body"),
            );
            check(
                "review leaves the isolated clipboard unchanged",
                clipboard == "J7 isolated clipboard before Copy",
            );
        }
        Act::RepairCopy => {
            check(
                "Copy returns the exact command to the isolated clipboard",
                clipboard == fixture::command(),
            );
            check(
                "copy preserves the review dialog",
                shell.dialog.is_some() && text(shell, "dialog-code") == Some(fixture::command()),
            );
        }
        Act::RepairClose => {
            check(
                "Escape dismisses the review modal",
                shell.dialog.is_none() && shell.ui.find("dialog").is_none(),
            );
            check(
                "dismissal restores the diagnostic fix focus",
                repair.is_some_and(|seq| {
                    shell.input.focused() == Some(stable_id(&format!("problem-{seq}-fix-0")))
                }),
            );
        }
        Act::Cleanup => check(
            "diagnostic journey leaves no modal or context menu",
            shell.dialog.is_none() && shell.menu.is_none(),
        ),
    }
    checks
}

fn bounded(shell: &Shell, id: &str) -> bool {
    shell.shown_node(id).is_some_and(|i| {
        let Box2 { x, y, w, h } = shell.ui.nodes[i].rect;
        [x, y, w, h].iter().all(|v| v.is_finite())
            && w > 0.
            && h > 0.
            && x >= -0.5
            && y >= -0.5
            && x + w <= shell.ui.size[0] + 0.5
            && y + h <= shell.ui.size[1] + 0.5
    })
}
