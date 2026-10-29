//! UI-independent text, selection, navigation, and undo state.
//!
//! Offsets are Unicode scalar (Rust `char`) indices, never byte indices. Cursor
//! navigation/deletion uses grapheme boundaries. GPUI belongs in editor-view.

use std::{collections::VecDeque, ops::Range};

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

const HISTORY_LIMIT: usize = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    /// Unicode scalar column, not a pixel or tab-expanded column.
    pub column: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn range(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Movement {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}

#[derive(Debug, Clone)]
struct Snapshot {
    text: Rope,
    selection: Selection,
    revision: u64,
}

/// Cheap immutable rope snapshot. Serialization belongs on a background worker.
#[derive(Debug, Clone)]
pub struct TextSnapshot {
    text: Rope,
    revision: u64,
}

impl TextSnapshot {
    pub fn write_to(&self, writer: impl std::io::Write) -> std::io::Result<()> {
        self.text.write_to(writer)
    }
}

#[derive(Debug, Clone)]
pub struct EditorBuffer {
    text: Rope,
    selection: Selection,
    preferred_column: Option<usize>,
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
    group: Option<Snapshot>,
    revision: u64,
    next_revision: u64,
    saved_revision: u64,
    newline: &'static str,
}

impl Default for EditorBuffer {
    fn default() -> Self {
        Self::with_text("")
    }
}

impl EditorBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_text(text: &str) -> Self {
        Self {
            text: Rope::from_str(text),
            selection: Selection::default(),
            preferred_column: None,
            undo: VecDeque::new(),
            redo: Vec::new(),
            group: None,
            revision: 0,
            next_revision: 1,
            saved_revision: 0,
            newline: match text.find(['\r', '\n']) {
                Some(i) if text[i..].starts_with("\r\n") => "\r\n",
                Some(i) if text[i..].starts_with('\r') => "\r",
                _ => "\n",
            },
        }
    }

    pub fn text_snapshot(&self) -> TextSnapshot {
        TextSnapshot {
            text: self.text.clone(),
            revision: self.revision,
        }
    }

    /// Must be a snapshot from this document; a save may finish after newer edits.
    pub fn mark_saved(&mut self, snapshot: &TextSnapshot) {
        self.saved_revision = snapshot.revision;
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// Content-history identity within this buffer, restored by undo/redo.
    pub fn content_revision(&self) -> u64 {
        self.revision
    }

    /// Existing and pasted bytes are retained; Enter uses the first line ending.
    pub fn insert_newline(&mut self) {
        self.insert(self.newline);
    }

    pub fn line_count(&self) -> usize {
        self.text.len_lines()
    }

    pub fn line_text(&self, line: usize) -> Option<String> {
        self.text.get_line(line).map(|text| text.to_string())
    }

    pub fn line_content(&self, line: usize) -> Option<String> {
        self.line_text(line).map(|text| {
            text.trim_end_matches([
                '\r', '\n', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}',
            ])
            .to_owned()
        })
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.text.line_to_char(line.min(self.line_count() - 1))
    }

    pub fn position(&self, offset: usize) -> Position {
        let offset = offset.min(self.len_chars());
        let line = self.text.char_to_line(offset);
        Position {
            line,
            column: offset - self.line_start(line),
        }
    }

    pub fn cursor(&self) -> Position {
        self.position(self.selection.head)
    }

    pub fn len_chars(&self) -> usize {
        self.text.len_chars()
    }

    pub fn is_empty(&self) -> bool {
        self.len_chars() == 0
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn selected_text(&self) -> String {
        self.text.slice(self.selection.range()).to_string()
    }

    pub fn text_in_range(&self, range: Range<usize>) -> Option<String> {
        self.text.get_slice(range).map(|text| text.to_string())
    }

    pub fn set_selection(&mut self, anchor: usize, head: usize) {
        self.selection = Selection {
            anchor: anchor.min(self.len_chars()),
            head: head.min(self.len_chars()),
        };
        self.preferred_column = None;
    }

    pub fn move_to(&mut self, offset: usize, extend: bool) {
        let offset = self.snap_boundary(offset.min(self.len_chars()));
        let anchor = if extend {
            self.selection.anchor
        } else {
            offset
        };
        self.set_selection(anchor, offset);
    }

    pub fn select_all(&mut self) {
        self.set_selection(0, self.len_chars());
    }

    pub fn move_cursor(&mut self, movement: Movement, extend: bool) {
        let range = self.selection.range();
        let caret = self.selection.head;
        let position = self.cursor();
        let vertical = matches!(movement, Movement::Up | Movement::Down);
        let column = self.preferred_column.unwrap_or(position.column);
        let target = match movement {
            Movement::Left if !extend && !range.is_empty() => range.start,
            Movement::Right if !extend && !range.is_empty() => range.end,
            Movement::Left => self.previous_boundary(caret),
            Movement::Right => self.next_boundary(caret),
            Movement::Up | Movement::Down => {
                let line = if matches!(movement, Movement::Up) {
                    position.line.saturating_sub(1)
                } else {
                    (position.line + 1).min(self.line_count() - 1)
                };
                let length = self.line_content(line).unwrap_or_default().chars().count();
                self.line_start(line) + column.min(length)
            }
            Movement::LineStart => self.line_start(position.line),
            Movement::LineEnd => {
                self.line_start(position.line)
                    + self
                        .line_content(position.line)
                        .unwrap_or_default()
                        .chars()
                        .count()
            }
            Movement::DocumentStart => 0,
            Movement::DocumentEnd => self.len_chars(),
        };
        self.move_to(target, extend);
        if vertical {
            self.preferred_column = Some(column);
        }
    }

    /// Returns false for an invalid range, without changing text or history.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) -> bool {
        if range.start > range.end || range.end > self.len_chars() {
            return false;
        }
        if self.text.slice(range.clone()) != text {
            if self.group.is_none() {
                self.push_undo(self.snapshot());
                self.redo.clear();
            }
            self.text.remove(range.clone());
            self.text.insert(range.start, text);
            self.revision = self.next_revision;
            self.next_revision += 1;
        }
        let head = range.start + text.chars().count();
        self.set_selection(head, head);
        true
    }

    pub fn insert(&mut self, text: &str) {
        self.replace_range(self.selection.range(), text);
    }

    pub fn backspace(&mut self) {
        let mut range = self.selection.range();
        if range.is_empty() {
            range.start = self.previous_boundary(range.start);
        }
        self.replace_range(range, "");
    }

    pub fn delete_forward(&mut self) {
        let mut range = self.selection.range();
        if range.is_empty() {
            range.end = self.next_boundary(range.end);
        }
        self.replace_range(range, "");
    }

    /// Treat all native IME preedit updates as one undo operation.
    pub fn begin_edit_group(&mut self) {
        if self.group.is_none() {
            self.group = Some(self.snapshot());
        }
    }

    pub fn end_edit_group(&mut self) {
        if let Some(before) = self.group.take() {
            if before.text != self.text {
                self.push_undo(before);
                self.redo.clear();
            } else {
                self.revision = before.revision;
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        self.end_edit_group();
        if let Some(previous) = self.undo.pop_back() {
            self.redo.push(self.snapshot());
            self.restore(previous);
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        self.end_edit_group();
        if let Some(next) = self.redo.pop() {
            self.push_undo(self.snapshot());
            self.restore(next);
            true
        } else {
            false
        }
    }

    pub fn char_to_utf16(&self, offset: usize) -> usize {
        self.text.char_to_utf16_cu(offset.min(self.len_chars()))
    }

    pub fn utf16_to_char(&self, offset: usize) -> usize {
        self.text
            .utf16_cu_to_char(offset.min(self.text.len_utf16_cu()))
    }

    pub fn range_to_utf16(&self, range: Range<usize>) -> Range<usize> {
        self.char_to_utf16(range.start)..self.char_to_utf16(range.end)
    }

    /// Expand a nonempty UTF-16 range to whole Unicode scalars.
    pub fn range_from_utf16(&self, range: Range<usize>) -> Range<usize> {
        let start = self.utf16_to_char(range.start);
        if range.start >= range.end {
            return start..start;
        }
        let mut end = self.utf16_to_char(range.end);
        if end < self.len_chars() && self.char_to_utf16(end) < range.end {
            end += 1;
        }
        start..end.max(start)
    }

    pub fn snap_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.len_chars());
        let line = self.text.char_to_line(offset);
        let mut position = self.line_start(line);
        for grapheme in self.line_text(line).unwrap_or_default().graphemes(true) {
            let next = position + grapheme.chars().count();
            if next > offset {
                break;
            }
            position = next;
        }
        position
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        if offset == 0 {
            return 0;
        }
        let mut line = self.text.char_to_line(offset);
        if self.line_start(line) == offset {
            line = line.saturating_sub(1);
        }
        let mut position = self.line_start(line);
        for grapheme in self.line_text(line).unwrap_or_default().graphemes(true) {
            let next = position + grapheme.chars().count();
            if next >= offset {
                return position;
            }
            position = next;
        }
        position
    }

    fn next_boundary(&self, offset: usize) -> usize {
        let line = self.text.char_to_line(offset);
        let mut position = self.line_start(line);
        for grapheme in self.line_text(line).unwrap_or_default().graphemes(true) {
            position += grapheme.chars().count();
            if position > offset {
                return position;
            }
        }
        self.len_chars()
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            selection: self.selection,
            revision: self.revision,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.revision = snapshot.revision;
        self.text = snapshot.text;
        self.selection = snapshot.selection;
        self.preferred_column = None;
    }

    fn push_undo(&mut self, snapshot: Snapshot) {
        if self.undo.len() == HISTORY_LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(buffer: &EditorBuffer) -> String {
        buffer.text_in_range(0..buffer.len_chars()).unwrap()
    }

    #[test]
    fn buffer_starts_at_origin() {
        let buffer = EditorBuffer::with_text("fn main() {}\n");
        assert_eq!(buffer.cursor(), Position { line: 0, column: 0 });
        assert_eq!(buffer.line_text(0).as_deref(), Some("fn main() {}\n"));
    }

    #[test]
    fn editor_core_has_sane_empty_state() {
        let mut buffer = EditorBuffer::new();
        assert!(buffer.is_empty());
        assert_eq!(buffer.len_chars(), 0);
        assert_eq!(buffer.line_count(), 1);
        buffer.backspace();
        buffer.delete_forward();
        assert!(!buffer.undo());
    }

    #[test]
    fn typing_and_multiline_paste_round_trip() {
        let mut buffer = EditorBuffer::new();
        buffer.insert("hello");
        buffer.insert("\nworld 😀");
        assert_eq!(buffer.cursor(), Position { line: 1, column: 7 });
        assert!(buffer.undo());
        assert_eq!(text(&buffer), "hello");
        assert!(buffer.undo());
        assert!(buffer.is_empty());
        assert!(buffer.redo());
        assert!(buffer.redo());
        assert_eq!(text(&buffer), "hello\nworld 😀");
    }

    #[test]
    fn reversed_selection_replace_undo_restores_selection() {
        let mut buffer = EditorBuffer::with_text("abcdef");
        buffer.set_selection(5, 2);
        buffer.insert("X");
        assert_eq!(text(&buffer), "abXf");
        buffer.undo();
        assert_eq!(buffer.selection(), Selection { anchor: 5, head: 2 });
        assert_eq!(buffer.selected_text(), "cde");
        buffer.redo();
        assert_eq!(text(&buffer), "abXf");
    }

    #[test]
    fn new_edit_invalidates_redo() {
        let mut buffer = EditorBuffer::new();
        buffer.insert("a");
        buffer.undo();
        buffer.insert("b");
        assert!(!buffer.redo());
        assert_eq!(text(&buffer), "b");
    }

    #[test]
    fn grapheme_deletion_does_not_split_combining_text_or_emoji() {
        let mut buffer = EditorBuffer::with_text("e\u{301}👩‍💻");
        buffer.move_cursor(Movement::DocumentEnd, false);
        buffer.backspace();
        assert_eq!(text(&buffer), "e\u{301}");
        buffer.backspace();
        assert!(buffer.is_empty());
        buffer.undo();
        buffer.move_cursor(Movement::DocumentStart, false);
        buffer.delete_forward();
        assert!(buffer.is_empty());
    }

    #[test]
    fn grapheme_navigation_and_selection() {
        let mut buffer = EditorBuffer::with_text("e\u{301}😀z");
        buffer.move_cursor(Movement::Right, true);
        assert_eq!(buffer.selected_text(), "e\u{301}");
        buffer.move_cursor(Movement::Right, true);
        assert_eq!(buffer.selected_text(), "e\u{301}😀");
        buffer.move_cursor(Movement::Left, false);
        assert_eq!(buffer.selection().head, 0);
    }

    #[test]
    fn vertical_navigation_remembers_column_across_short_lines() {
        let mut buffer = EditorBuffer::with_text("abcdef\nx\nabcdef");
        buffer.move_to(5, false);
        buffer.move_cursor(Movement::Down, false);
        assert_eq!(buffer.cursor(), Position { line: 1, column: 1 });
        buffer.move_cursor(Movement::Down, false);
        assert_eq!(buffer.cursor(), Position { line: 2, column: 5 });
    }

    #[test]
    fn crlf_is_one_navigation_and_deletion_boundary() {
        let mut buffer = EditorBuffer::with_text("a\r\nb");
        buffer.move_to(3, false);
        buffer.move_cursor(Movement::Left, false);
        assert_eq!(buffer.selection().head, 1);
        buffer.delete_forward();
        assert_eq!(text(&buffer), "ab");
        buffer.undo();
        assert_eq!(text(&buffer), "a\r\nb");
    }

    #[test]
    fn utf16_ranges_do_not_split_surrogate_pairs() {
        let buffer = EditorBuffer::with_text("a😀b");
        assert_eq!(buffer.range_to_utf16(1..2), 1..3);
        assert_eq!(buffer.range_from_utf16(1..3), 1..2);
        assert_eq!(buffer.range_from_utf16(2..3), 1..2);
        assert_eq!(buffer.range_from_utf16(1..2), 1..2);
        assert_eq!(buffer.range_from_utf16(2..2), 1..1);
        assert_eq!(buffer.range_from_utf16(100..200), 3..3);
    }

    #[test]
    fn composition_is_one_undo_operation() {
        let mut buffer = EditorBuffer::with_text("prefix ");
        buffer.move_cursor(Movement::DocumentEnd, false);
        buffer.begin_edit_group();
        buffer.replace_range(7..7, "n");
        buffer.replace_range(7..8, "ni");
        buffer.replace_range(7..9, "你");
        buffer.end_edit_group();
        assert_eq!(text(&buffer), "prefix 你");
        buffer.undo();
        assert_eq!(text(&buffer), "prefix ");
        assert!(!buffer.undo());
        buffer.redo();
        assert_eq!(text(&buffer), "prefix 你");
    }

    #[test]
    fn invalid_edit_cannot_change_buffer_or_history() {
        let mut buffer = EditorBuffer::with_text("abc");
        assert!(!buffer.replace_range(0..4, "oops"));
        assert!(!buffer.replace_range(Range { start: 2, end: 1 }, "oops"));
        assert_eq!(text(&buffer), "abc");
        assert!(!buffer.undo());
    }

    #[test]
    fn edits_are_not_limited_to_first_two_hundred_lines() {
        let mut buffer = EditorBuffer::with_text(&"line\n".repeat(500));
        buffer.move_cursor(Movement::DocumentEnd, false);
        buffer.insert("last");
        assert_eq!(buffer.line_content(500).as_deref(), Some("last"));
        assert_eq!(buffer.cursor().line, 500);
    }
}

#[cfg(test)]
mod persistence_regressions {
    use super::*;

    #[test]
    fn undo_and_redo_track_the_saved_revision() {
        let mut buffer = EditorBuffer::with_text("first");
        buffer.select_all();
        buffer.insert("second");
        let saved = buffer.text_snapshot();
        buffer.mark_saved(&saved);
        assert!(!buffer.is_dirty());
        buffer.insert(" third");
        assert!(buffer.is_dirty());
        buffer.undo();
        assert!(!buffer.is_dirty());
        buffer.undo();
        assert!(buffer.is_dirty());
        buffer.redo();
        assert!(!buffer.is_dirty());
    }

    #[test]
    fn completing_an_older_save_does_not_clean_newer_edits() {
        let mut buffer = EditorBuffer::new();
        buffer.insert("snapshot");
        let saved = buffer.text_snapshot();
        buffer.insert(" plus newer edits");
        buffer.mark_saved(&saved);
        assert!(buffer.is_dirty());
        let mut bytes = Vec::new();
        saved.write_to(&mut bytes).unwrap();
        assert_eq!(bytes, b"snapshot");
        buffer.undo();
        assert!(!buffer.is_dirty());
    }

    #[test]
    fn enter_preserves_first_newline_style_and_paste_is_exact() {
        for newline in ["\n", "\r\n", "\r"] {
            let mut buffer = EditorBuffer::with_text(&format!("a{newline}b"));
            buffer.move_cursor(Movement::DocumentEnd, false);
            buffer.insert_newline();
            buffer.insert("e\u{301}👩‍💻\r\nx\ny");
            assert_eq!(
                buffer.text_in_range(0..buffer.len_chars()).unwrap(),
                format!("a{newline}b{newline}e\u{301}👩‍💻\r\nx\ny")
            );
        }
    }

    #[test]
    fn cancelled_composition_restores_clean_state() {
        let mut buffer = EditorBuffer::new();
        buffer.begin_edit_group();
        buffer.insert("preedit");
        buffer.replace_range(0..7, "");
        buffer.end_edit_group();
        assert!(!buffer.is_dirty());
        assert!(!buffer.undo());
    }
}
