use crate::Result;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Clipboard seam implemented by the platform host; tests use an in-memory clipboard.
pub trait Clipboard {
    /// Read plain UTF-8 text.
    fn read(&mut self) -> Result<String>;
    /// Replace the plain-text clipboard contents.
    fn write(&mut self, text: &str) -> Result<()>;
}
/// Keyboard/OS IME messages use UTF-8 ranges; composition never changes committed content.
#[derive(Clone, Debug)]
pub enum EditEvent {
    /// Replace the current selection with single-line text.
    Insert(String),
    /// Remove a grapheme before the caret or the selection.
    Backspace,
    /// Remove the following grapheme or the selection.
    Delete,
    /// Move one logical grapheme, optionally extending the selection.
    Move {
        /// Forward versus backward logical movement.
        forward: bool,
        /// Keep the existing anchor.
        select: bool,
    },
    /// Select the entire committed value.
    SelectAll,
    /// Move to an explicit grapheme boundary.
    Select {
        /// Anchor byte offset.
        anchor: usize,
        /// Caret byte offset.
        caret: usize,
    },
    /// Begin an OS composition at the current selection.
    ImeStart,
    /// Replace the uncommitted composition and its selected byte range.
    ImePreedit {
        /// Uncommitted string.
        text: String,
        /// Selection within the preedit.
        selection: Range<usize>,
    },
    /// Commit composition, replacing the saved selection in one undo step.
    ImeCommit(String),
    /// Cancel without altering committed text.
    ImeCancel,
    /// Copy the selected committed text.
    Copy,
    /// Copy and remove selected text.
    Cut,
    /// Insert clipboard text after removing line breaks.
    Paste,
    /// Restore the previous edit.
    Undo,
    /// Reapply the last undone edit.
    Redo,
}
#[derive(Clone)]
struct Snapshot {
    value: String,
    anchor: usize,
    caret: usize,
}
/// Single-line editing state, owned by the retained field rather than a scripting object.
#[derive(Default)]
pub struct TextInput {
    value: String,
    anchor: usize,
    caret: usize,
    preedit: String,
    preedit_selection: Range<usize>,
    composition: Option<Range<usize>>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}
impl TextInput {
    /// Create a single-line field, filtering newline/control characters.
    pub fn new(value: &str) -> Self {
        let value = single_line(value);
        let caret = value.len();
        Self {
            value,
            anchor: caret,
            caret,
            ..Self::default()
        }
    }
    /// Committed model-facing value.
    pub fn value(&self) -> &str {
        &self.value
    }
    /// Selected committed UTF-8 range.
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }
    /// Current logical caret offset.
    pub fn caret(&self) -> usize {
        self.caret
    }
    /// Fixed selection anchor, independent of selection direction.
    pub fn anchor(&self) -> usize {
        self.anchor
    }
    /// Display value, with preedit replacing the saved committed selection.
    pub fn display(&self) -> String {
        if let Some(r) = &self.composition {
            format!(
                "{}{}{}",
                &self.value[..r.start],
                self.preedit,
                &self.value[r.end..]
            )
        } else {
            self.value.clone()
        }
    }
    /// Underlined composition range in display text, if composition is active.
    pub fn preedit_range(&self) -> Option<Range<usize>> {
        self.composition
            .as_ref()
            .map(|r| r.start..r.start + self.preedit.len())
    }
    /// Caret in display-text coordinates, including OS preedit selection.
    pub fn display_caret(&self) -> usize {
        self.composition
            .as_ref()
            .map_or(self.caret, |r| r.start + self.preedit_selection.end)
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            value: self.value.clone(),
            anchor: self.anchor,
            caret: self.caret,
        }
    }
    fn restore(&mut self, s: Snapshot) {
        self.value = s.value;
        self.anchor = s.anchor;
        self.caret = s.caret;
        self.cancel();
    }
    fn cancel(&mut self) {
        self.composition = None;
        self.preedit.clear();
        self.preedit_selection = 0..0;
    }
    fn replace(&mut self, range: Range<usize>, text: &str) {
        self.undo.push(self.snapshot());
        self.redo.clear();
        let text = single_line(text);
        self.value.replace_range(range.clone(), &text);
        self.caret = range.start + text.len();
        self.anchor = self.caret;
        self.cancel();
    }
    /// Apply an input message. Returns whether the visual value/selection changed.
    pub fn event(&mut self, event: EditEvent, clipboard: &mut dyn Clipboard) -> Result<bool> {
        let before = (
            self.value.clone(),
            self.anchor,
            self.caret,
            self.preedit.clone(),
            self.composition.clone(),
        );
        match event {
            EditEvent::ImeStart => {
                self.cancel();
                self.composition = Some(self.selection());
            }
            EditEvent::ImePreedit { text, selection } => {
                if self.composition.is_none()
                    || selection.start > selection.end
                    || !text.is_char_boundary(selection.start)
                    || !text.is_char_boundary(selection.end)
                    || text.chars().any(char::is_control)
                {
                    return Err("ARA7031: invalid IME range or composition lifecycle".into());
                }
                self.preedit = text;
                self.preedit_selection = selection;
            }
            EditEvent::ImeCommit(text) => {
                let r = self
                    .composition
                    .clone()
                    .ok_or("ARA7031: commit without active composition")?;
                self.replace(r, &text);
            }
            EditEvent::ImeCancel => self.cancel(),
            EditEvent::Insert(text) => self.replace(self.selection(), &text),
            EditEvent::Paste => {
                let text = clipboard.read()?;
                self.replace(self.selection(), &text);
            }
            EditEvent::Copy => clipboard.write(&self.value[self.selection()])?,
            EditEvent::Cut => {
                let r = self.selection();
                clipboard.write(&self.value[r.clone()])?;
                if !r.is_empty() {
                    self.replace(r, "");
                }
            }
            EditEvent::SelectAll => {
                self.cancel();
                self.anchor = 0;
                self.caret = self.value.len();
            }
            EditEvent::Select { anchor, caret } => {
                if !boundary(&self.value, anchor) || !boundary(&self.value, caret) {
                    return Err("ARA7032: selection splits a grapheme".into());
                }
                self.cancel();
                self.anchor = anchor;
                self.caret = caret;
            }
            EditEvent::Move { forward, select } => {
                self.cancel();
                let range = self.selection();
                self.caret = if !select && !range.is_empty() {
                    if forward { range.end } else { range.start }
                } else {
                    neighbor(&self.value, self.caret, forward)
                };
                if !select {
                    self.anchor = self.caret;
                }
            }
            EditEvent::Backspace | EditEvent::Delete => {
                let mut r = self.selection();
                if r.is_empty() {
                    if matches!(event, EditEvent::Backspace) {
                        r.start = neighbor(&self.value, r.start, false);
                    } else {
                        r.end = neighbor(&self.value, r.end, true);
                    }
                }
                if !r.is_empty() {
                    self.replace(r, "");
                }
            }
            EditEvent::Undo => {
                if let Some(s) = self.undo.pop() {
                    self.redo.push(self.snapshot());
                    self.restore(s);
                }
            }
            EditEvent::Redo => {
                if let Some(s) = self.redo.pop() {
                    self.undo.push(self.snapshot());
                    self.restore(s);
                }
            }
        }
        Ok(before
            != (
                self.value.clone(),
                self.anchor,
                self.caret,
                self.preedit.clone(),
                self.composition.clone(),
            ))
    }
}
fn single_line(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}
fn boundary(s: &str, i: usize) -> bool {
    i == s.len() || s.grapheme_indices(true).any(|(j, _)| j == i)
}
fn neighbor(s: &str, i: usize, forward: bool) -> usize {
    if forward {
        s.grapheme_indices(true)
            .map(|(j, _)| j)
            .find(|j| *j > i)
            .unwrap_or(s.len())
    } else {
        s.grapheme_indices(true)
            .map(|(j, _)| j)
            .take_while(|j| *j < i)
            .last()
            .unwrap_or(0)
    }
}
