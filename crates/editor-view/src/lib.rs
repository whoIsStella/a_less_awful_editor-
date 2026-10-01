//! Stateful native input and viewport rendering. Text/history remain in editor-core.
//! Filesystem operations are owned by the shell, never the input handler.

use std::ops::Range;

use ale_editor_core::{EditorBuffer, Movement, Position, TextSnapshot};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId,
    IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Render, ScrollWheelEvent, ShapedLine, Style, TextRun, UTF16Selection, Window,
    div, fill, point, prelude::*, px, relative, rgb, size,
};

const LINE_HEIGHT: f32 = 21.0;
const GUTTER: f32 = 56.0;

/// Emitted by compact command inputs when Enter is pressed outside composition.
pub struct Submit;
impl gpui::EventEmitter<Submit> for EditorView {}

pub struct EditorView {
    buffer: EditorBuffer,
    gutter: f32,
    focus: FocusHandle,
    marked: Option<Range<usize>>,
    rows: Vec<VisualLine>,
    bounds: Option<Bounds<Pixels>>,
    scroll_y: f32,
    scroll_x: f32,
    reveal_caret: bool,
    selecting: bool,
}

impl EditorView {
    pub fn new(text: &str, cx: &mut Context<Self>) -> Self {
        Self {
            buffer: EditorBuffer::with_text(text),
            gutter: GUTTER,
            focus: cx.focus_handle(),
            marked: None,
            rows: Vec::new(),
            bounds: None,
            scroll_y: 0.0,
            scroll_x: 0.0,
            reveal_caret: true,
            selecting: false,
        }
    }

    /// Reuse the native editor input path for compact workstation command fields.
    pub fn input(text: &str, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(text, cx);
        view.gutter = 0.0;
        view
    }

    pub fn text(&self) -> String {
        self.buffer
            .text_in_range(0..self.buffer.len_chars())
            .unwrap_or_default()
    }

    pub fn go_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        self.finish_composition();
        self.buffer.move_to(self.buffer.line_start(line), false);
        self.changed(cx);
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }

    pub fn content_revision(&self) -> u64 {
        self.buffer.content_revision()
    }

    pub fn save_snapshot(&mut self) -> TextSnapshot {
        self.finish_composition();
        self.buffer.text_snapshot()
    }

    pub fn mark_saved(&mut self, snapshot: &TextSnapshot, cx: &mut Context<Self>) {
        self.buffer.mark_saved(snapshot);
        cx.notify();
    }

    pub fn load(&mut self, buffer: EditorBuffer, cx: &mut Context<Self>) {
        self.buffer = buffer;
        self.marked = None;
        self.rows.clear();
        self.scroll_y = 0.0;
        self.scroll_x = 0.0;
        self.selecting = false;
        self.changed(cx);
    }

    pub fn cursor(&self) -> Position {
        self.buffer.cursor()
    }

    fn finish_composition(&mut self) {
        self.marked = None;
        self.buffer.end_edit_group();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.reveal_caret = true;
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key: &str = event.keystroke.key.as_ref();
        let modifiers = event.keystroke.modifiers;
        let command = modifiers.secondary();
        if modifiers.alt {
            return;
        }
        if self.gutter == 0.0 && key == "enter" && self.marked.is_none() {
            cx.emit(Submit);
            cx.stop_propagation();
            return;
        }
        let movement = match (command, key) {
            (false, "left") => Some(Movement::Left),
            (false, "right") => Some(Movement::Right),
            (false, "up") => Some(Movement::Up),
            (false, "down") => Some(Movement::Down),
            (false, "home") => Some(Movement::LineStart),
            (false, "end") => Some(Movement::LineEnd),
            (true, "home") => Some(Movement::DocumentStart),
            (true, "end") => Some(Movement::DocumentEnd),
            _ => None,
        };
        if let Some(movement) = movement {
            self.finish_composition();
            self.buffer.move_cursor(movement, modifiers.shift);
        } else if command {
            match key {
                "a" => {
                    self.finish_composition();
                    self.buffer.select_all();
                }
                "c" | "x" => {
                    self.finish_composition();
                    if !self.buffer.selection().range().is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            self.buffer.selected_text(),
                        ));
                        if key == "x" {
                            self.buffer.insert("");
                        }
                    }
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.finish_composition();
                        self.buffer.insert(&text);
                    }
                }
                "z" => {
                    self.finish_composition();
                    if modifiers.shift {
                        self.buffer.redo();
                    } else {
                        self.buffer.undo();
                    }
                }
                "y" => {
                    self.finish_composition();
                    self.buffer.redo();
                }
                _ => return,
            }
        } else {
            match key {
                "backspace" | "delete" | "enter" | "tab" => {
                    self.finish_composition();
                    match key {
                        "backspace" => self.buffer.backspace(),
                        "delete" => self.buffer.delete_forward(),
                        "enter" => self.buffer.insert_newline(),
                        "tab" => self.buffer.insert("\t"),
                        _ => unreachable!(),
                    }
                }
                // Printable text is delivered by the native input handler below,
                // not reconstructed from physical keys (important for IMEs/layouts).
                _ => return,
            }
        }
        cx.stop_propagation();
        self.changed(cx);
    }

    fn offset_at(&self, position: Point<Pixels>) -> Option<usize> {
        let first = self.rows.first()?;
        let row_index = ((f32::from(position.y - first.origin.y) / LINE_HEIGHT)
            .floor()
            .max(0.0) as usize)
            .min(self.rows.len() - 1);
        let row = &self.rows[row_index];
        let byte = row.text.closest_index_for_x(position.x - row.origin.x);
        let column = row
            .byte_offsets
            .partition_point(|offset| *offset <= byte)
            .saturating_sub(1);
        Some(self.buffer.snap_boundary(row.start + column))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        self.finish_composition();
        if let Some(offset) = self.offset_at(event.position) {
            self.buffer.move_to(offset, event.modifiers.shift);
        }
        self.selecting = true;
        self.changed(cx);
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting
            && let Some(offset) = self.offset_at(event.position)
        {
            self.buffer.move_to(offset, true);
            self.changed(cx);
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(LINE_HEIGHT));
        self.scroll_y = (self.scroll_y - f32::from(delta.y)).max(0.0);
        self.scroll_x = (self.scroll_x - f32::from(delta.x)).max(0.0);
        self.reveal_caret = false;
        cx.notify();
        cx.stop_propagation();
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for EditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("scratch-editor")
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .font_family(if cfg!(target_os = "macos") {
                "Menlo"
            } else if cfg!(target_os = "windows") {
                "Consolas"
            } else {
                "DejaVu Sans Mono"
            })
            .text_size(px(13.0))
            .line_height(px(LINE_HEIGHT))
            .text_color(rgb(0xd4dce7))
            .bg(rgb(0x101216))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .child(EditorElement {
                editor: cx.entity(),
            })
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(range);
        *actual = Some(self.buffer.range_to_utf16(range.clone()));
        self.buffer.text_in_range(range)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let selection = self.buffer.selection();
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(selection.range()),
            reversed: selection.head < selection.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .clone()
            .map(|range| self.buffer.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition();
        self.changed(cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.buffer.range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.buffer.selection().range());
        self.buffer.replace_range(range, text);
        self.finish_composition();
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.buffer.range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.buffer.selection().range());
        let start = range.start;
        let start_utf16 = self.buffer.char_to_utf16(start);
        self.buffer.begin_edit_group();
        self.buffer.replace_range(range, text);
        self.marked = if text.is_empty() {
            None
        } else {
            Some(start..start + text.chars().count())
        };
        if let Some(selected) = selected {
            let length = text.encode_utf16().count();
            let selected = self.buffer.range_from_utf16(
                start_utf16 + selected.start.min(length)..start_utf16 + selected.end.min(length),
            );
            self.buffer.set_selection(selected.start, selected.end);
        }
        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.buffer.range_from_utf16(range);
        let row = self
            .rows
            .iter()
            .find(|row| range.start >= row.start && range.start <= row.end())?;
        let start = row.x_for_offset(range.start);
        let end = row.x_for_offset(range.end.min(row.end()));
        Some(Bounds::new(
            point(start, row.origin.y),
            size(px(f32::from(end - start).max(1.0)), px(LINE_HEIGHT)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.offset_at(point)
            .map(|offset| self.buffer.char_to_utf16(offset))
    }
}

struct VisualLine {
    start: usize,
    text: ShapedLine,
    number: ShapedLine,
    /// Display-byte position for each source scalar, including the end position.
    /// Tabs expand for painting without changing their underlying buffer content.
    byte_offsets: Vec<usize>,
    origin: Point<Pixels>,
}

impl VisualLine {
    fn end(&self) -> usize {
        self.start + self.byte_offsets.len() - 1
    }

    fn x_for_offset(&self, offset: usize) -> Pixels {
        let column = offset
            .saturating_sub(self.start)
            .min(self.byte_offsets.len() - 1);
        self.origin.x + self.text.x_for_index(self.byte_offsets[column])
    }
}

fn expand_tabs(text: &str) -> (String, Vec<usize>) {
    let mut expanded = String::new();
    let mut offsets = Vec::with_capacity(text.chars().count() + 1);
    let mut column = 0;
    for ch in text.chars() {
        offsets.push(expanded.len());
        if ch == '\t' {
            let spaces = 4 - column % 4;
            expanded.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            expanded.push(ch);
            column += 1;
        }
    }
    offsets.push(expanded.len());
    (expanded, offsets)
}

struct EditorElement {
    editor: Entity<EditorView>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Vec<VisualLine>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<VisualLine> {
        self.editor.update(cx, |editor, _| {
            let height = f32::from(bounds.size.height).max(1.0);
            let visible = (height / LINE_HEIGHT).ceil().max(1.0) as usize;
            let max_scroll = (editor.buffer.line_count() as f32 * LINE_HEIGHT - height).max(0.0);
            editor.scroll_y = editor.scroll_y.clamp(0.0, max_scroll);
            if editor
                .bounds
                .is_none_or(|previous| previous.size != bounds.size)
            {
                editor.reveal_caret = true;
            }
            let caret_y = editor.buffer.cursor().line as f32 * LINE_HEIGHT;
            if editor.reveal_caret {
                if caret_y < editor.scroll_y {
                    editor.scroll_y = caret_y;
                } else if caret_y + LINE_HEIGHT > editor.scroll_y + height {
                    editor.scroll_y = (caret_y + LINE_HEIGHT - height).clamp(0.0, max_scroll);
                }
            }
            let top = (editor.scroll_y / LINE_HEIGHT).floor() as usize;
            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let mut rows = Vec::new();
            for index in top..(top + visible + 1).min(editor.buffer.line_count()) {
                let content = editor.buffer.line_content(index).unwrap_or_default();
                let (display, byte_offsets) = expand_tabs(&content);
                let run = TextRun {
                    len: display.len(),
                    font: style.font(),
                    color: style.color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let text = window
                    .text_system()
                    .shape_line(display.into(), font_size, &[run], None);
                let label = format!("{:>5}", index + 1);
                let run = TextRun {
                    len: label.len(),
                    font: style.font(),
                    color: rgb(if index == editor.buffer.cursor().line {
                        0x7aa2f7
                    } else {
                        0x596579
                    })
                    .into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let number = window
                    .text_system()
                    .shape_line(label.into(), font_size, &[run], None);
                rows.push(VisualLine {
                    start: editor.buffer.line_start(index),
                    text,
                    number,
                    byte_offsets,
                    origin: point(
                        bounds.left() + px(editor.gutter),
                        bounds.top() + px(index as f32 * LINE_HEIGHT - editor.scroll_y),
                    ),
                });
            }
            if editor.reveal_caret {
                let head = editor.buffer.selection().head;
                if let Some(row) = rows
                    .iter()
                    .find(|row| head >= row.start && head <= row.end())
                {
                    let x = f32::from(row.x_for_offset(head) - row.origin.x);
                    let width = (f32::from(bounds.size.width) - editor.gutter - 8.0).max(1.0);
                    if x < editor.scroll_x {
                        editor.scroll_x = x;
                    }
                    if x > editor.scroll_x + width {
                        editor.scroll_x = x - width;
                    }
                }
            }
            let widest = rows
                .iter()
                .map(|row| f32::from(row.x_for_offset(row.end()) - row.origin.x))
                .fold(0.0_f32, f32::max);
            let width = (f32::from(bounds.size.width) - editor.gutter - 8.0).max(1.0);
            editor.scroll_x = editor.scroll_x.clamp(0.0, (widest - width).max(0.0));
            for row in &mut rows {
                row.origin.x -= px(editor.scroll_x);
            }
            editor.reveal_caret = false;
            editor.bounds = Some(bounds);
            rows
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        rows: &mut Vec<VisualLine>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus, selection, marked, gutter) = {
            let editor = self.editor.read(cx);
            (
                editor.focus.clone(),
                editor.buffer.selection(),
                editor.marked.clone(),
                editor.gutter,
            )
        };
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        let range = selection.range();
        let text_bounds = Bounds::new(
            point(bounds.left() + px(gutter), bounds.top()),
            size(
                px((f32::from(bounds.size.width) - gutter).max(0.0)),
                bounds.size.height,
            ),
        );
        window.with_content_mask(
            Some(ContentMask {
                bounds: text_bounds,
            }),
            |window| {
                for row in rows.iter() {
                    if gutter > 0.0
                        && range.is_empty()
                        && focus.is_focused(window)
                        && selection.head >= row.start
                        && selection.head <= row.end()
                    {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(text_bounds.left(), row.origin.y),
                                size(text_bounds.size.width, px(LINE_HEIGHT)),
                            ),
                            rgb(0x141a23),
                        ));
                    }
                    if range.start <= row.end() && range.end > row.start {
                        let x1 = row.x_for_offset(range.start.max(row.start));
                        let mut x2 = row.x_for_offset(range.end.min(row.end()));
                        if range.end > row.end() {
                            x2 += px(8.0);
                        }
                        window.paint_quad(fill(
                            Bounds::new(
                                point(x1, row.origin.y),
                                size(px(f32::from(x2 - x1).max(1.0)), px(LINE_HEIGHT)),
                            ),
                            rgb(0x273b59),
                        ));
                    }
                    if let Err(error) = row.text.paint(row.origin, px(LINE_HEIGHT), window, cx) {
                        eprintln!("editor text paint failed: {error}");
                    }
                    if let Some(marked) = &marked
                        && marked.start <= row.end()
                        && marked.end > row.start
                    {
                        let x1 = row.x_for_offset(marked.start.max(row.start));
                        let x2 = row.x_for_offset(marked.end.min(row.end()));
                        window.paint_quad(fill(
                            Bounds::new(
                                point(x1, row.origin.y + px(LINE_HEIGHT - 2.0)),
                                size(px(f32::from(x2 - x1).max(1.0)), px(1.0)),
                            ),
                            rgb(0x9abcf5),
                        ));
                    }
                    if focus.is_focused(window)
                        && selection.head >= row.start
                        && selection.head <= row.end()
                    {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(row.x_for_offset(selection.head), row.origin.y + px(2.0)),
                                size(px(2.0), px(LINE_HEIGHT - 4.0)),
                            ),
                            rgb(0x7aa2f7),
                        ));
                    }
                }
            },
        );
        let gutter_bounds = Bounds::new(bounds.origin, size(px(gutter), bounds.size.height));
        window.with_content_mask(
            Some(ContentMask {
                bounds: gutter_bounds,
            }),
            |window| {
                for row in rows.iter() {
                    if let Err(error) = row.number.paint(
                        point(bounds.left() + px(6.0), row.origin.y),
                        px(LINE_HEIGHT),
                        window,
                        cx,
                    ) {
                        eprintln!("editor gutter paint failed: {error}");
                    }
                }
            },
        );
        self.editor.update(cx, |editor, _| {
            editor.rows = std::mem::take(rows);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::expand_tabs;

    #[test]
    fn tab_mapping_preserves_source_character_positions() {
        let (text, offsets) = expand_tabs("a\t😀b");
        assert_eq!(text, "a   😀b");
        assert_eq!(offsets, vec![0, 1, 4, 8, 9]);
    }

    #[test]
    fn empty_line_has_a_valid_caret_position() {
        assert_eq!(expand_tabs(""), (String::new(), vec![0]));
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use gpui::{ScrollDelta, TestAppContext, TouchPhase};

    #[gpui::test]
    fn scrolling_beyond_200_lines_caret_reveal_and_resize(cx: &mut TestAppContext) {
        let (editor, cx) = cx.add_window_view(|window, cx| {
            let view = EditorView::new(&"line\r\n".repeat(500), cx);
            view.focus.focus(window);
            view
        });
        cx.simulate_keystrokes("ctrl-end");
        cx.update(|_, cx| {
            let view = editor.read(cx);
            assert_eq!(view.cursor().line, 500);
            assert!(view.scroll_y > 200.0 * LINE_HEIGHT);
            assert!(
                view.rows
                    .iter()
                    .any(|row| row.start == view.buffer.line_start(500))
            );
        });
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.scroll(
                    &ScrollWheelEvent {
                        position: point(px(100.0), px(100.0)),
                        delta: ScrollDelta::Pixels(point(px(0.0), px(5000.0))),
                        modifiers: Default::default(),
                        touch_phase: TouchPhase::Moved,
                    },
                    window,
                    cx,
                )
            })
        });
        cx.run_until_parked();
        cx.simulate_input("x");
        cx.simulate_resize(size(px(640.0), px(400.0)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = editor.read(cx);
            assert_eq!(
                view.cursor(),
                Position {
                    line: 500,
                    column: 1
                }
            );
            assert!(
                view.rows
                    .iter()
                    .any(|row| row.start == view.buffer.line_start(500))
            );
        });
    }

    #[gpui::test]
    fn native_composition_utf16_commit_undo_and_document_replacement(cx: &mut TestAppContext) {
        let (editor, cx) = cx.add_window_view(|window, cx| {
            let view = EditorView::new("", cx);
            view.focus.focus(window);
            view
        });
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
                assert_eq!(editor.marked_text_range(window, cx), Some(0..2));
                editor.replace_text_in_range(None, "你😀", window, cx);
                assert_eq!(
                    editor.selected_text_range(false, window, cx).unwrap().range,
                    3..3
                );
                assert!(editor.marked.is_none());
            })
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                assert!(editor.buffer.is_empty());
                editor.replace_and_mark_text_in_range(None, "preedit", None, window, cx);
                editor.load(EditorBuffer::with_text("new file"), cx);
                assert!(editor.marked.is_none());
                assert!(!editor.is_dirty());
                assert!(editor.focus.is_focused(window));
                assert!(!editor.buffer.undo());
            })
        });
    }
}
