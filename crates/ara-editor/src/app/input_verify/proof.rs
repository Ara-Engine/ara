use super::{Act, PREEDIT, UNICODE};
use crate::shell::Shell;
use ara_ui::{paint::Box2, source::stable_id, widgets::Widget};
use serde_json::{Value, json};

fn field<'a>(shell: &'a Shell, id: &str) -> Option<&'a ara_ui_text::TextInput> {
    match shell.ui.widgets.get(&stable_id(id)) {
        Some(Widget::Input(text)) => Some(text),
        _ => None,
    }
}
fn text<'a>(shell: &'a Shell, id: &str) -> Option<&'a str> {
    field(shell, id).map(ara_ui_text::TextInput::value)
}
fn host(shell: &Shell) -> Value {
    shell
        .toolkit_verify
        .as_ref()
        .map_or(Value::Null, |host| host.snapshot())
}

pub(super) fn snapshot(shell: &Shell) -> Value {
    let fields:Vec<_>=shell.ui.nodes.iter().filter(|node|node.element.id.starts_with("kit-")).filter_map(|node|field(shell,&node.element.id).map(|text|json!({"id":node.element.id,"committed":text.value(),"display":text.display(),"anchor":text.anchor(),"caret":text.caret(),"preedit":text.preedit_range()}))).collect();
    let bounds: Vec<_> = shell
        .ui
        .nodes
        .iter()
        .filter(|node| {
            node.element.id.starts_with("kit-")
                && (node.element.id.ends_with("-body") || node.element.tag == "popover")
        })
        .map(|node| json!({"id":node.element.id,"rect":node.rect,"clip":node.clip}))
        .collect();
    json!({"host":host(shell),"focus":shell.input.focused().and_then(|id|shell.ui.by_id(id)).map(|i|shell.ui.nodes[i].element.id.as_str()),"capture":shell.input.captured(),"focusGraph":shell.input.focus_graph(&shell.ui),"composing":shell.native_input.composing(),"modifiers":format!("{:?}",shell.native_input.modifiers()),"fields":fields,"bounds":bounds})
}

pub(super) fn frame_checks(act: Act, age: u32, shell: &Shell) -> Vec<Value> {
    let check = match (act, age) {
        (Act::Clipboard, 2) => Some((
            "Home collapses selection at the UTF-8 beginning",
            field(shell, "kit-text").is_some_and(|text| text.anchor() == 0 && text.caret() == 0),
        )),
        (Act::Clipboard, 3) => Some((
            "Shift-End extends from the original anchor",
            field(shell, "kit-text")
                .is_some_and(|text| text.anchor() == 0 && text.caret() == UNICODE.len()),
        )),
        (Act::Grapheme, 2) => Some((
            "two Shift-Right presses select A and one emoji",
            field(shell, "kit-text").is_some_and(|text| text.selection() == (0.."A🙂".len())),
        )),
        (Act::FocusLoss, 1) => Some((
            "the pending button genuinely held capture and right Shift",
            shell.input.captured() == Some(stable_id("kit-open-prompt"))
                && shell.native_input.modifiers().shift,
        )),
        (Act::FocusLoss, 2) => Some((
            "native focus-loss immediately clears capture without closing the scope",
            shell.input.captured().is_none() && scope_only(shell, "kit-"),
        )),
        _ => None,
    };
    check
        .into_iter()
        .map(|(label, ok)| json!({"check":label,"ok":ok,"frame":age}))
        .collect()
}

pub(super) fn checks(
    act: Act,
    shell: &Shell,
    clipboard: &str,
    stacked_focus: Option<u64>,
    initial_focus: Option<u64>,
) -> Vec<Value> {
    let state = host(shell);
    let mut checks = Vec::new();
    let mut check = |label: &str, ok: bool| checks.push(json!({"check":label,"ok":ok}));
    match act {
        Act::Unicode => {
            check(
                "committed Unicode appears exactly once",
                text(shell, "kit-text") == Some(UNICODE),
            );
            check(
                "host receives the same Unicode intent",
                state["text"] == UNICODE,
            );
        }
        Act::Clipboard => {
            check(
                "isolated clipboard receives all Unicode",
                clipboard == UNICODE,
            );
            check(
                "Home / Shift-End / Paste replace the whole selection",
                text(shell, "kit-text") == Some(UNICODE),
            );
            check(
                "paste collapses selection at UTF-8 end",
                field(shell, "kit-text").is_some_and(|text| {
                    text.anchor() == UNICODE.len() && text.caret() == UNICODE.len()
                }),
            );
        }
        Act::Grapheme => check(
            "Shift-Right crosses a whole emoji grapheme before Delete",
            text(shell, "kit-text") == Some("B"),
        ),
        Act::Preedit => {
            check(
                "preedit never mutates committed value",
                text(shell, "kit-text") == Some("Stable") && state["text"] == "Stable",
            );
            check(
                "Unicode preedit is displayed and tracked",
                shell.native_input.composing()
                    && field(shell, "kit-text").is_some_and(|text| {
                        text.display().contains(PREEDIT) && text.preedit_range().is_some()
                    }),
            );
        }
        Act::Cancel => {
            check(
                "Escape cancels composition and restores committed display",
                !shell.native_input.composing()
                    && field(shell, "kit-text")
                        .is_some_and(|text| text.value() == "Stable" && text.display() == "Stable"),
            );
            check(
                "composition Escape preserves the modal host",
                scope_only(shell, "kit-") && shell.input.focused() == Some(stable_id("kit-text")),
            );
        }
        Act::FocusLoss => {
            check(
                "lost capture cannot activate the pending prompt",
                shell.input.captured().is_none() && state["dialogCount"] == 0,
            );
            check(
                "lost modifiers make Tab forward",
                shell.native_input.modifiers() == Default::default()
                    && shell.input.focused() == Some(stable_id("kit-text")),
            );
            check(
                "focus loss retains the host focus scope",
                scope_only(shell, "kit-"),
            );
        }
        Act::SelectEnd => {
            check(
                "End addresses option 100 without committing it",
                state["selectOpen"] == true
                    && state["selectActive"] == 99
                    && state["selected"] == 0,
            );
            let option = shell.ui.find("kit-long-select-option-0-99");
            check(
                "active long option is fully revealed inside its real clip",
                option.is_some_and(|i| inside(shell.ui.nodes[i].rect, shell.ui.nodes[i].clip)),
            );
            check(
                "100-option list scrolls to the final row",
                shell
                    .ui
                    .find("kit-long-select-popover-options")
                    .and_then(|i| shell.ui.scroll_position(i))
                    .is_some_and(|p| p[1] > 0.),
            );
            check("localized select popup stays bounded", popup_bounded(shell));
        }
        Act::SelectSearch => {
            check(
                "native Unicode text ingress filters the shared Select",
                state["selectQuery"] == "Choice 097"
                    && state["selectActive"] == 97
                    && state["selected"] == 0,
            );
            check(
                "filtered long label remains inside popup width",
                popup_bounded(shell),
            );
        }
        Act::SelectAccept => {
            check(
                "Enter returns the typed option value",
                state["selected"] == 97 && state["selectOpen"] == false,
            );
            check(
                "Select restores its originating field focus",
                shell.input.focused() == Some(stable_id("kit-select")),
            );
        }
        Act::Prompt => {
            check(
                "prompt holds typed Unicode",
                text(shell, "kit-dialog-1-field") == Some("Prompt ✓"),
            );
            check(
                "repeated Tab remains trapped in the real prompt scope",
                state["dialogCount"] == 1 && scope_only(shell, "kit-dialog-1-"),
            );
            check(
                "Tab cycles all three prompt controls in document order",
                shell.input.focused() == Some(stable_id("kit-dialog-1-cancel")),
            );
            check(
                "long wrapping prompt fits the viewport",
                dialog_bounded(shell, "kit-dialog-1-body"),
            );
        }
        Act::Stack => {
            check(
                "a second shared modal traps all focus above the prompt",
                state["dialogCount"] == 2 && scope_only(shell, "kit-dialog-2-"),
            );
            check(
                "stacked long notes fit the viewport",
                dialog_bounded(shell, "kit-dialog-2-body"),
            );
            check(
                "stacking preserves the underlying typed prompt",
                text(shell, "kit-dialog-1-field") == Some("Prompt ✓"),
            );
        }
        Act::Unstack => {
            check(
                "Escape closes exactly one modal",
                state["dialogCount"] == 1 && scope_only(shell, "kit-dialog-1-"),
            );
            check(
                "underlying prompt restores its previous control focus",
                shell.input.focused() == stacked_focus,
            );
        }
        Act::PromptAccept => {
            check(
                "Enter completes prompt with the owned Unicode result",
                state["dialogCount"] == 0 && state["promptResult"] == "Saved prompt ✓",
            );
            check(
                "prompt completion returns focus to the invoking button",
                shell.input.focused() == Some(stable_id("kit-open-prompt")),
            );
        }
        Act::LongNote => {
            check(
                "standalone long note owns the scope",
                state["dialogCount"] == 1 && scope_only(shell, "kit-dialog-3-"),
            );
            check(
                "long note is viewport bounded and wraps",
                dialog_bounded(shell, "kit-dialog-3-body")
                    && shell
                        .ui
                        .find("kit-dialog-3-message")
                        .is_some_and(|i| shell.ui.nodes[i].rect.h > 100.),
            );
        }
        Act::NoteClose => {
            check(
                "Escape dismisses alert and restores the invoking button",
                state["dialogCount"] == 0
                    && shell.input.focused() == Some(stable_id("kit-open-note")),
            );
            check(
                "base host remains modal after dismissing all dialogs",
                scope_only(shell, "kit-"),
            );
        }
        Act::Cleanup => {
            check(
                "toolkit host and overlay nodes are removed",
                shell.toolkit_verify.is_none() && shell.ui.find("kit-host").is_none(),
            );
            check(
                "original editor focus is restored",
                shell.input.focused() == initial_focus,
            );
        }
    }
    checks
}

fn scope_only(shell: &Shell, prefix: &str) -> bool {
    let graph = shell.input.focus_graph(&shell.ui);
    graph.as_array().is_some_and(|nodes| {
        !nodes.is_empty()
            && nodes
                .iter()
                .all(|node| node["id"].as_str().is_some_and(|id| id.starts_with(prefix)))
    })
}
fn inside(rect: Box2, clip: Box2) -> bool {
    [
        rect.x, rect.y, rect.w, rect.h, clip.x, clip.y, clip.w, clip.h,
    ]
    .iter()
    .all(|v| v.is_finite())
        && rect.w > 0.
        && rect.h > 0.
        && rect.x >= clip.x - 0.5
        && rect.y >= clip.y - 0.5
        && rect.x + rect.w <= clip.x + clip.w + 0.5
        && rect.y + rect.h <= clip.y + clip.h + 0.5
}
fn popup_bounded(shell: &Shell) -> bool {
    shell.ui.find("kit-long-select-popover").is_some_and(|i| {
        let node = &shell.ui.nodes[i];
        node.rect.w <= ara_ui::popover::MAX_WIDTH + 0.5
            && inside(
                node.rect,
                Box2 {
                    x: 0.,
                    y: 0.,
                    w: shell.ui.size[0],
                    h: shell.ui.size[1],
                },
            )
    })
}
fn dialog_bounded(shell: &Shell, id: &str) -> bool {
    shell.ui.find(id).is_some_and(|i| {
        inside(
            shell.ui.nodes[i].rect,
            Box2 {
                x: 0.,
                y: 0.,
                w: shell.ui.size[0],
                h: shell.ui.size[1],
            },
        )
    })
}
