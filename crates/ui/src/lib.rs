use ale_editor_view::EditorView;
use gpui::{Context, Div, Entity, Focusable, IntoElement, Render, Window, div, prelude::*, px, rgb};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Terminal,
    Problems,
    Debug,
    Output,
}

impl Panel {
    fn label(self) -> &'static str {
        match self {
            Self::Terminal => "TERMINAL",
            Self::Problems => "PROBLEMS",
            Self::Debug => "DEBUG CONSOLE",
            Self::Output => "OUTPUT",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal is not implemented yet. This is not a shell prompt.",
            Self::Problems => "Language diagnostics are not connected yet.",
            Self::Debug => "No debug adapter is connected. Debugging is not implemented yet.",
            Self::Output => "Scratch editor only. Project files are not opened or saved by this build.",
        }
    }
}

pub struct AppShell {
    editor: Entity<EditorView>,
    panel: Panel,
}

impl AppShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            EditorView::new("// Scratch buffer: changes are not saved.\n\nfn main() {\n    println!(\"less awful\");\n}\n", cx)
        });
        editor.read(cx).focus_handle(cx).focus(window);
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        Self { editor, panel: Panel::Terminal }
    }
}

impl Render for AppShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let position = self.editor.read(cx).cursor();
        let tabs = [Panel::Terminal, Panel::Problems, Panel::Debug, Panel::Output]
            .into_iter()
            .map(|panel| {
                let active = panel == self.panel;
                div()
                    .id(panel.label())
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_color(rgb(if active { 0xd8dee9 } else { 0x747e90 }))
                    .border_b_1()
                    .border_color(rgb(if active { 0x90b4ed } else { 0x101217 }))
                    .child(panel.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.panel = panel;
                        cx.notify();
                    }))
            });
        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x0e1014))
            .text_color(rgb(0xcfd5df))
            .child(
                div()
                    .h(px(36.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(0x292d36))
                    .bg(rgb(0x15181e))
                    .text_sm()
                    .child("A Less Awful Editor  /  Scratch"),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(file_placeholder())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .child(
                                div()
                                    .h(px(34.0))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .px_3()
                                    .border_b_1()
                                    .border_color(rgb(0x292d36))
                                    .bg(rgb(0x171a20))
                                    .text_sm()
                                    .child("Scratch — in memory only; not saved"),
                            )
                            .child(div().flex_1().min_h_0().child(self.editor.clone())),
                    ),
            )
            .child(
                div()
                    .h(px(132.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(rgb(0x292d36))
                    .bg(rgb(0x101217))
                    .child(div().h(px(32.0)).flex().text_sm().children(tabs))
                    .child(
                        div()
                            .p_3()
                            .text_sm()
                            .text_color(rgb(0x8d94a3))
                            .child(self.panel.message()),
                    ),
            )
            .child(
                div()
                    .h(px(24.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .border_t_1()
                    .border_color(rgb(0x292d36))
                    .bg(rgb(0x171a20))
                    .text_color(rgb(0x8d94a3))
                    .text_sm()
                    .child("SCRATCH  ·  Not saved to disk")
                    .child(format!("Ln {}, Col {}", position.line + 1, position.column + 1)),
            )
    }
}

fn file_placeholder() -> Div {
    div()
        .w(px(236.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(rgb(0x292d36))
        .bg(rgb(0x13161b))
        .child(div().px_3().py_2().text_sm().child("FILES"))
        .child(
            div()
                .p_3()
                .text_sm()
                .text_color(rgb(0x747e90))
                .child("File navigation is not implemented yet."),
        )
}
