//! Bounded exception details from the original diagnostic, without copying its tree.
use super::*;

use crate::exception_stack::Stack;

impl Shell {
    pub(super) fn log_stack_details(
        &mut self,
        out: &mut String,
        id: &str,
        diagnostic: &ara_diag::Diagnostic,
    ) {
        let Some(value) = diagnostic
            .data
            .as_deref()
            .and_then(|data| data.get("exception"))
        else {
            return;
        };
        let stack = Stack::parse(value);
        out.push_str(&format!("<text id='{id}-stack-title' class='caption' style='flex-shrink:0' text='Exception stack' /><scroll id='{id}-stack-scroll' label='Exception stack' style='max-height:260px;min-height:0px;min-width:0px;flex-basis:auto;flex-shrink:1;gap:8px'>"));
        for (index, exception) in stack.nodes.iter().enumerate() {
            let prefix = if index == 0 { "" } else { "Caused by: " };
            out.push_str(&format!("<panel id='{id}-stack-cause-{index}' style='min-width:0px;flex-shrink:0;gap:4px;padding-left:{}px'><text id='{id}-stack-type-{index}' class='fg' style='white-space:normal;flex-shrink:0' text='{}{}' /><text id='{id}-stack-message-{index}' class='dialog-text' style='white-space:normal;flex-shrink:0' text='{}' />",exception.depth * 10, prefix, escape(exception.kind), escape(exception.message)));
            for frame in &exception.frames {
                let row = format!("{id}-stack-frame-{}", frame.index);
                out.push_str(&format!("<panel id='{row}' style='min-width:0px;flex-shrink:0;gap:2px'><text id='{row}-method' class='mono caption' style='white-space:normal;flex-shrink:0' text='{}' />",escape(frame.method)));
                if let Some(file) = frame.file {
                    let mut location = file.to_owned();
                    if let Some(line) = frame.line {
                        location.push_str(&format!(":{line}"));
                        if let Some(column) = frame.column {
                            location.push_str(&format!(":{column}"));
                        }
                    }
                    if frame.link {
                        let link = format!("{row}-source");
                        let mut target = ara_diag::Diagnostic::new(
                            diagnostic.severity,
                            diagnostic.code,
                            frame.method,
                        );
                        target.location.file = Some(file.to_owned());
                        target.location.line = frame.line;
                        target.location.column = frame.column;
                        self.bindings.insert(
                            ara_ui::source::stable_id(&link),
                            Binding::Diagnostic(target),
                        );
                        out.push_str(&format!("<button id='{link}' class='log-source' style='min-height:18px;flex-shrink:0' label='{}' description='Open {}' action='Dispatch'><text id='{link}-label' text='{}' /></button>",escape(&location),escape(&location),escape(&location)));
                    } else {
                        out.push_str(&format!("<text id='{row}-source-text' class='mono caption' style='white-space:normal;flex-shrink:0' text='{}' />",escape(&location)));
                    }
                }
                out.push_str("</panel>");
            }
            out.push_str("</panel>");
        }
        if stack.malformed {
            out.push_str(&format!("<text id='{id}-stack-malformed' class='caption' style='white-space:normal;flex-shrink:0' text='Some exception details are unavailable because their format is invalid or unsupported.' />"));
        }
        if stack.truncated {
            out.push_str(&format!("<text id='{id}-stack-truncated' class='caption' style='white-space:normal;flex-shrink:0' text='Exception details truncated. Additional frames or inner causes were omitted.' />"));
        }
        out.push_str("</scroll>");
    }
}

#[cfg(test)]
#[path = "stack/tests.rs"]
mod tests;
