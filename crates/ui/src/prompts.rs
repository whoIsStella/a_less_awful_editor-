//! Small Linux fallback: GPUI 0.2.2's default clips long details and lacks
//! keyboard dismissal. Other platforms retain their native prompts.
use gpui::{
    App, AppContext, Context, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    PromptButton, PromptResponse, Render, Window, div, prelude::*, px, rgb,
};

pub fn init(cx: &mut App) {
    if !cfg!(target_os = "linux") {
        return;
    }
    cx.set_prompt_builder(|_, message, detail, answers, handle, window, cx| {
        let view = cx.new(|cx| Confirmation {
            message: message.into(),
            detail: detail.unwrap_or_default().into(),
            answers: answers.to_vec(),
            selected: answers
                .iter()
                .position(|a| matches!(a, PromptButton::Cancel(_)))
                .unwrap_or(0),
            focus: cx.focus_handle(),
        });
        handle.with_view(view, window, cx)
    });
}

struct Confirmation {
    message: String,
    detail: String,
    answers: Vec<PromptButton>,
    selected: usize,
    focus: FocusHandle,
}

impl EventEmitter<PromptResponse> for Confirmation {}
impl Focusable for Confirmation {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Confirmation {
    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => {
                if let Some(index) = self
                    .answers
                    .iter()
                    .position(|a| matches!(a, PromptButton::Cancel(_)))
                {
                    cx.emit(PromptResponse(index));
                }
            }
            "enter" => cx.emit(PromptResponse(self.selected)),
            "tab" | "down" | "up" => {
                let backwards = event.keystroke.key == "up" || event.keystroke.modifiers.shift;
                self.selected = (self.selected
                    + if backwards { self.answers.len() - 1 } else { 1 })
                    % self.answers.len();
                cx.notify();
            }
            _ => {}
        }
        cx.stop_propagation();
    }
}
impl Render for Confirmation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000088))
            .child(
                div()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(Self::key))
                    .w(px(520.0))
                    .max_w_full()
                    .max_h_full()
                    .overflow_hidden()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .bg(rgb(0x1b2029))
                    .text_color(rgb(0xd8dee9))
                    .border_1()
                    .border_color(rgb(0x56657c))
                    .rounded_lg()
                    .child(div().whitespace_normal().child(self.message.clone()))
                    .child(
                        div()
                            .id("confirmation-detail")
                            .max_h(px(140.0))
                            .overflow_y_scroll()
                            .text_sm()
                            .whitespace_normal()
                            .child(self.detail.clone()),
                    )
                    .children(self.answers.iter().enumerate().map(|(index, answer)| {
                        div()
                            .id(index)
                            .px_3()
                            .py_2()
                            .text_sm()
                            .cursor_pointer()
                            .border_1()
                            .border_color(rgb(if index == self.selected {
                                0x90b4ed
                            } else {
                                0x39414f
                            }))
                            .child(answer.label().clone())
                            .on_click(
                                cx.listener(move |_, _, _, cx| cx.emit(PromptResponse(index))),
                            )
                    })),
            )
    }
}
