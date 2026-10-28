//! Bounded exception details from the original diagnostic, without copying its tree.
use super::*;

const MAX_FRAMES: usize = 64;
const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 16;
const MAX_STRING: usize = 8192;

struct Frame<'a> {
    index: usize,
    method: &'a str,
    file: Option<&'a str>,
    line: Option<u32>,
    column: Option<u32>,
    link: bool,
}

struct Exception<'a> {
    depth: usize,
    kind: &'a str,
    message: &'a str,
    frames: Vec<Frame<'a>>,
}

#[derive(Default)]
struct Stack<'a> {
    nodes: Vec<Exception<'a>>,
    visited: usize,
    frames: usize,
    truncated: bool,
    malformed: bool,
}

impl<'a> Stack<'a> {
    fn parse(value: &'a Value) -> Self {
        let mut stack = Self::default();
        stack.visit(value, 0);
        stack
    }

    fn text(&mut self, value: Option<&'a Value>, fallback: &'static str) -> &'a str {
        let Some(text) = value.and_then(Value::as_str) else {
            self.malformed = true;
            return fallback;
        };
        let (text, cut) = bounded(text);
        self.truncated |= cut;
        text
    }

    fn coordinate(&mut self, value: Option<&Value>) -> (Option<u32>, bool) {
        match value {
            Some(Value::Null) => (None, true),
            Some(value) => match value.as_u64().and_then(|n| u32::try_from(n).ok()) {
                Some(n) if n > 0 => (Some(n), true),
                _ => {
                    self.malformed = true;
                    (None, false)
                }
            },
            None => {
                self.malformed = true;
                (None, false)
            }
        }
    }

    fn visit(&mut self, value: &'a Value, depth: usize) {
        if depth >= MAX_DEPTH || self.visited >= MAX_NODES {
            self.truncated = true;
            return;
        }
        self.visited += 1;
        let Some(node) = value
            .as_object()
            .filter(|node| node.get("schema").and_then(Value::as_u64) == Some(1))
        else {
            self.malformed = true;
            return;
        };
        let kind = self.text(node.get("type"), "Exception type unavailable");
        let message = self.text(node.get("message"), "Exception message unavailable");
        match node.get("truncated").and_then(Value::as_bool) {
            Some(truncated) => self.truncated |= truncated,
            None => self.malformed = true,
        }
        let mut frames = Vec::new();
        if let Some(rows) = node.get("frames").and_then(Value::as_array) {
            let available = MAX_FRAMES - self.frames;
            self.truncated |= rows.len() > available;
            for row in rows.iter().take(available) {
                let Some(row) = row.as_object() else {
                    self.malformed = true;
                    continue;
                };
                let method = self.text(row.get("method"), "Frame method unavailable");
                let (file, complete) = match row.get("file") {
                    Some(Value::Null) => (None, true),
                    Some(Value::String(file)) => {
                        let (file, cut) = bounded(file);
                        self.truncated |= cut;
                        (Some(file), !cut)
                    }
                    _ => {
                        self.malformed = true;
                        (None, false)
                    }
                };
                let (line, valid_line) = self.coordinate(row.get("line"));
                let (column, valid_column) = self.coordinate(row.get("column"));
                let link = complete && valid_line && valid_column && file.is_some_and(source_file);
                frames.push(Frame {
                    index: self.frames,
                    method,
                    file,
                    line,
                    column,
                    link,
                });
                self.frames += 1;
            }
        } else {
            self.malformed = true;
        }
        self.nodes.push(Exception {
            depth,
            kind,
            message,
            frames,
        });
        if let Some(inners) = node.get("inners").and_then(Value::as_array) {
            for inner in inners {
                if self.visited >= MAX_NODES || depth + 1 >= MAX_DEPTH {
                    self.truncated = true;
                    break;
                }
                self.visit(inner, depth + 1);
            }
        } else {
            self.malformed = true;
        }
    }
}

fn bounded(text: &str) -> (&str, bool) {
    if text.len() <= MAX_STRING {
        return (text, false);
    }
    let mut end = MAX_STRING;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// No filesystem reads while generating a panel. The existing IDE opener checks
/// project containment and resolves the exact provided path when it is clicked.
fn source_file(file: &str) -> bool {
    !file.trim().is_empty()
        && !file.chars().any(char::is_control)
        && !file.contains("://")
        && !(file.starts_with('<') && file.ends_with('>'))
}

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
