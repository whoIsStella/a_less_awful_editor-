//! Small Linux fallback: GPUI 0.2.2's default clips long details and lacks
//! keyboard dismissal. Other platforms retain their native prompts.
use gpui::{
    App, AppContext, Context, EventEmitter, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyDownEvent, PromptButton, PromptResponse, Render, Window, div, prelude::*, px, rgb,
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
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .bg(gpui::rgba(0x101216b8))
            .child(
                div()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(Self::key))
                    .w(px(400.0))
                    .max_w_full()
                    .max_h_full()
                    .overflow_hidden()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .bg(rgb(0x1b2028))
                    .text_color(rgb(0xd4dce7))
                    .border_1()
                    .border_color(rgb(0x2a313c))
                    .rounded_lg()
                    .shadow_lg()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(16.0))
                            .line_height(px(22.0))
                            .font_weight(FontWeight::MEDIUM)
                            .whitespace_normal()
                            .child(self.message.clone()),
                    )
                    .when(!self.detail.is_empty(), |card| {
                        card.child(
                            div()
                                .id("confirmation-detail")
                                .min_h_0()
                                .max_h(px(140.0))
                                .overflow_y_scroll()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(rgb(0x8590a3))
                                .whitespace_normal()
                                .child(self.detail.clone()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .flex_shrink_0()
                            .justify_end()
                            .gap_2()
                            .pt_1()
                            .children(self.answers.iter().enumerate().map(|(index, answer)| {
                                let primary = matches!(answer, PromptButton::Ok(_));
                                let danger = matches!(answer, PromptButton::Other(_));
                                let selected = index == self.selected;
                                div()
                                    .id(index)
                                    .min_w(px(64.0))
                                    .min_h(px(30.0))
                                    .px_3()
                                    .py_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(12.0))
                                    .line_height(px(18.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .whitespace_normal()
                                    .cursor_pointer()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(match (selected, primary) {
                                        (true, true) => 0xd4dce7,
                                        (true, false) | (false, true) => 0x7aa2f7,
                                        (false, false) => 0x2a313c,
                                    }))
                                    .bg(rgb(if primary { 0x7aa2f7 } else { 0x1b2028 }))
                                    .text_color(rgb(if primary {
                                        0x101216
                                    } else if danger {
                                        0xe06c75
                                    } else if selected {
                                        0xd4dce7
                                    } else {
                                        0x8590a3
                                    }))
                                    .hover(move |style| {
                                        style
                                            .bg(rgb(if primary { 0x7aa2f7 } else { 0x2a313c }))
                                            .border_color(rgb(if danger {
                                                0xe06c75
                                            } else {
                                                0x7aa2f7
                                            }))
                                    })
                                    .child(answer.label().clone())
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(PromptResponse(index));
                                    }))
                            })),
                    ),
            )
    }
}
