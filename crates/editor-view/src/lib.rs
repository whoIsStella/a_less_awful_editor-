//! GPUI rendering for the editor model.
//!
//! This crate is intentionally the boundary between the UI framework and
//! `ale-editor-core`.

use ale_editor_core::EditorBuffer;
use gpui::{Div, div, prelude::*, px, rgb};

pub fn editor_surface(buffer: &EditorBuffer) -> Div {
    let lines = (0..buffer.line_count().min(200))
        .filter_map(|line| buffer.line_text(line).map(|text| (line, text)))
        .map(|(line, text)| {
            div()
                .flex()
                .h(px(22.0))
                .items_center()
                .child(
                    div()
                        .w(px(54.0))
                        .text_color(rgb(0x666b78))
                        .child(format!("{:>4}", line + 1)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(rgb(0xd8dee9))
                        .child(text.trim_end_matches('\n').to_owned()),
                )
        });

    div()
        .flex()
        .flex_col()
        .flex_1()
        .bg(rgb(0x111318))
        .child(
            div()
                .h(px(34.0))
                .flex()
                .items_center()
                .px_3()
                .border_b_1()
                .border_color(rgb(0x292d36))
                .bg(rgb(0x171a20))
                .child("main.rs"),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .p_3()
                .children(lines),
        )
}
