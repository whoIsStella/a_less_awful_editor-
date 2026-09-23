//! UI-independent editor state.
//!
//! This crate must not depend on GPUI. Keeping the editing model separate from
//! rendering lets the UI evolve without taking the editor engine with it.

use ropey::Rope;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone)]
pub struct EditorBuffer {
    text: Rope,
    cursor: Position,
}

impl Default for EditorBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorBuffer {
    pub fn new() -> Self {
        Self {
            text: Rope::new(),
            cursor: Position::default(),
        }
    }

    pub fn with_text(text: &str) -> Self {
        Self {
            text: Rope::from_str(text),
            cursor: Position::default(),
        }
    }

    pub fn line_count(&self) -> usize {
        self.text.len_lines()
    }

    pub fn line_text(&self, line: usize) -> Option<String> {
        (line < self.text.len_lines()).then(|| self.text.line(line).to_string())
    }

    pub fn cursor(&self) -> Position {
        self.cursor
    }

    pub fn len_chars(&self) -> usize {
        self.text.len_chars()
    }

    pub fn is_empty(&self) -> bool {
        self.text.len_chars() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_starts_at_origin() {
        let buffer = EditorBuffer::with_text("fn main() {}\n");

        assert_eq!(buffer.cursor(), Position { line: 0, column: 0 });
        assert_eq!(buffer.line_text(0).as_deref(), Some("fn main() {}\n"));
    }

    #[test]
    fn editor_core_has_sane_empty_state() {
        let buffer = EditorBuffer::new();

        assert!(buffer.is_empty());
        assert_eq!(buffer.len_chars(), 0);
        assert_eq!(buffer.line_count(), 1);
    }
}
