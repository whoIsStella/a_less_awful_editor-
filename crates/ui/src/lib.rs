use ale_editor_core::EditorBuffer;
use ale_editor_view::editor_surface;
use gpui::{Context, Div, IntoElement, Render, Window, div, prelude::*, px, rgb};

pub struct AppShell {
    buffer: EditorBuffer,
}

impl AppShell {
    pub fn new() -> Self {
        Self {
            buffer: EditorBuffer::with_text(
                "fn main() {\n    println!(\"less awful\");\n}\n",
            ),
        }
    }
}

impl Default for AppShell {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for AppShell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x0e1014))
            .text_color(rgb(0xcfd5df))
            .child(top_bar())
            .child(
                div()
                    .flex()
                    .flex_1()
                    .child(file_tree())
                    .child(editor_surface(&self.buffer)),
            )
            .child(bottom_panel())
            .child(status_bar())
    }
}

fn top_bar() -> Div {
    div()
        .h(px(36.0))
        .flex()
        .items_center()
        .px_3()
        .border_b_1()
        .border_color(rgb(0x292d36))
        .bg(rgb(0x15181e))
        .text_sm()
        .child("a_less_awful_editor  /  src  /  main.rs")
}

fn file_tree() -> Div {
    div()
        .w(px(236.0))
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(rgb(0x292d36))
        .bg(rgb(0x13161b))
        .child(
            div()
                .h(px(34.0))
                .flex()
                .items_center()
                .px_3()
                .text_color(rgb(0x8d94a3))
                .text_sm()
                .child("FILES"),
        )
        .child(
            div()
                .px_3()
                .py_2()
                .text_sm()
                .child("▾  crates")
                .child(div().pl_4().child("▸  app"))
                .child(div().pl_4().child("▸  editor-core"))
                .child(div().pl_4().child("▸  editor-view"))
                .child(div().pl_4().child("▸  ui"))
                .child("   Cargo.toml"),
        )
}

fn bottom_panel() -> Div {
    div()
        .h(px(156.0))
        .flex()
        .flex_col()
        .border_t_1()
        .border_color(rgb(0x292d36))
        .bg(rgb(0x101217))
        .child(
            div()
                .h(px(32.0))
                .flex()
                .items_center()
                .px_3()
                .text_sm()
                .child("TERMINAL     PROBLEMS     DEBUG CONSOLE     OUTPUT"),
        )
        .child(
            div()
                .flex_1()
                .px_3()
                .py_2()
                .text_color(rgb(0x9aa2b1))
                .text_sm()
                .child("$ cargo run"),
        )
}

fn status_bar() -> Div {
    div()
        .h(px(24.0))
        .flex()
        .items_center()
        .px_3()
        .border_t_1()
        .border_color(rgb(0x292d36))
        .bg(rgb(0x171a20))
        .text_color(rgb(0x8d94a3))
        .text_sm()
        .child("main    0 errors    Rust    UTF-8    Ln 1, Col 1")
}
