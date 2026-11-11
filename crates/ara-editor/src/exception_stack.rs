//! Bounded exception details from a diagnostic's `data.exception` (the C# host's fault
//! payload: type, message, frames, inner exceptions), parsed without copying its tree. The
//! Console in either UI lists them, each frame with a source link where it names one.
use serde_json::Value;

/// Most frames kept, across the tree.
pub(crate) const MAX_FRAMES: usize = 64;
/// Deepest inner exception kept.
pub(crate) const MAX_DEPTH: usize = 8;
/// Most exceptions kept.
pub(crate) const MAX_NODES: usize = 16;
/// Longest text kept (bytes).
pub(crate) const MAX_STRING: usize = 8192;

/// One stack frame: its method, and its source position when it names one (`link`: a
/// project file that opens at it).
pub(crate) struct Frame<'a> {
    pub index: usize,
    pub method: &'a str,
    pub file: Option<&'a str>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub link: bool,
}

/// One exception in the tree: how deep (inner exceptions nest), its type, message, frames.
pub(crate) struct Exception<'a> {
    pub depth: usize,
    pub kind: &'a str,
    pub message: &'a str,
    pub frames: Vec<Frame<'a>>,
}

/// The exception tree, flattened outer first, within the bounds the host promises.
#[derive(Default)]
pub(crate) struct Stack<'a> {
    pub nodes: Vec<Exception<'a>>,
    /// Exceptions visited (bounded by [`MAX_NODES`]).
    pub visited: usize,
    /// Frames kept (bounded by [`MAX_FRAMES`]).
    pub frames: usize,
    pub truncated: bool,
    pub malformed: bool,
}

impl<'a> Stack<'a> {
    /// The exception tree in a diagnostic's `data.exception` (the C# host's payload).
    pub(crate) fn parse(value: &'a Value) -> Self {
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
