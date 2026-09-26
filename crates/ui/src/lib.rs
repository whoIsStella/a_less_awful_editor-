mod persistence;
pub mod prompts;

use ale_editor_core::TextSnapshot;
use ale_editor_view::EditorView;
use gpui::{
    Context, Div, Entity, Focusable, IntoElement, PathPromptOptions, PromptButton, PromptLevel,
    Render, Window, actions, div, prelude::*, px, rgb,
};
use persistence::{DiskVersion, Loaded, SaveError};
use std::path::PathBuf;

actions!(document, [Open, Save, SaveAs, Close, Quit]);

enum Intent {
    Open(Box<Loaded>),
    Close,
}

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
            Self::Output => {
                "Single-file editing. Open: Ctrl+O | Save: Ctrl+S | Save As: Ctrl+Shift+S"
            }
        }
    }
}

pub struct AppShell {
    editor: Entity<EditorView>,
    panel: Panel,
    path: Option<PathBuf>,
    disk: Option<DiskVersion>,
    busy: bool,
    pending: Option<Intent>,
    status: String,
}

impl AppShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| EditorView::new("", cx));
        editor.read(cx).focus_handle(cx).focus(window);
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        let shell = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = shell.update(cx, |this, cx| {
                this.request_replace(Intent::Close, window, cx)
            });
            false
        });
        Self {
            editor,
            panel: Panel::Terminal,
            path: None,
            disk: None,
            busy: false,
            pending: None,
            status: "Untitled - not saved to disk".into(),
        }
    }

    fn finish(&mut self, message: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = false;
        self.status = message.into();
        self.editor.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    fn cancel(&mut self, message: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.pending = None;
        self.finish(message, window, cx);
    }

    fn failure(&mut self, message: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = Panel::Output;
        self.cancel(message, window, cx);
    }

    fn request_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "Choose a UTF-8 file...".into();
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open file".into()),
        });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let path = match picker.await {
                Ok(Ok(Some(paths))) if paths.len() == 1 => paths.into_iter().next().unwrap(),
                Ok(Ok(_)) => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.cancel("Open cancelled", window, cx)
                    });
                    return;
                }
                result => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.failure(
                            format!("Could not open file dialog: {result:?}"),
                            window,
                            cx,
                        )
                    });
                    return;
                }
            };
            let result = cx
                .background_executor()
                .spawn(async move { persistence::load(&path) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(loaded) => this.request_replace(Intent::Open(Box::new(loaded)), window, cx),
                    Err(error) => this.failure(format!("Open failed: {error}"), window, cx),
                }
            });
        })
        .detach();
    }

    fn request_replace(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if !self.editor.read(cx).is_dirty() {
            self.apply_intent(intent, window, cx);
            return;
        }
        self.busy = true;
        let answer = window.prompt(
            PromptLevel::Warning,
            "Save changes before continuing?",
            Some("Unsaved changes will be lost if you discard them."),
            &[
                PromptButton::Ok("Save".into()),
                PromptButton::Other("Discard".into()),
                PromptButton::Cancel("Cancel".into()),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match answer {
                    Ok(0) => {
                        this.pending = Some(intent);
                        this.request_save(false, window, cx);
                    }
                    Ok(1) => this.apply_intent(intent, window, cx),
                    _ => this.cancel("Cancelled - document retained", window, cx),
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_intent(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        match intent {
            Intent::Close => cx.quit(),
            Intent::Open(loaded) => {
                let Loaded {
                    path,
                    buffer,
                    version,
                } = *loaded;
                self.path = Some(path);
                self.disk = Some(version);
                self.editor.update(cx, |editor, cx| editor.load(buffer, cx));
                self.finish("Opened", window, cx);
            }
        }
    }

    fn request_save(&mut self, save_as: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        if !save_as && let Some(path) = self.path.clone() {
            let snapshot = self.editor.update(cx, |editor, _| editor.save_snapshot());
            self.write_snapshot(path, snapshot, self.disk.clone(), window, cx);
            return;
        }
        let directory = self
            .path
            .as_ref()
            .and_then(|path| path.parent())
            .unwrap_or(std::path::Path::new("."));
        let name = self
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled.txt");
        let picker = cx.prompt_for_new_path(directory, Some(name));
        self.status = "Choose a save destination...".into();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = picker.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(Some(path))) => {
                        let snapshot = this.editor.update(cx, |editor, _| editor.save_snapshot());
                        // Always create-only first, even if a platform picker already asked.
                        this.write_snapshot(path, snapshot, None, window, cx);
                    }
                    Ok(Ok(None)) => this.cancel("Save cancelled - document retained", window, cx),
                    error => {
                        this.failure(format!("Could not open save dialog: {error:?}"), window, cx)
                    }
                }
            });
        })
        .detach();
    }

    fn write_snapshot(
        &mut self,
        path: PathBuf,
        snapshot: TextSnapshot,
        expected: Option<DiskVersion>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.status = "Saving snapshot...".into();
        cx.notify();
        let worker_path = path.clone();
        let worker_snapshot = snapshot.clone();
        let work = cx.background_executor().spawn(async move {
            persistence::save(&worker_path, &worker_snapshot, expected.as_ref())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(version) => {
                        this.path = Some(path);
                        this.disk = Some(version);
                        this.editor
                            .update(cx, |editor, cx| editor.mark_saved(&snapshot, cx));
                        let message = if this.editor.read(cx).is_dirty() {
                            "Snapshot saved; newer edits remain unsaved"
                        } else {
                            "Saved"
                        };
                        this.finish(message, window, cx);
                        if let Some(intent) = this.pending.take() {
                            match intent {
                                Intent::Open(loaded) => {
                                    // A save may have changed the very file being opened.
                                    // Refresh it, then recheck edits made during either I/O.
                                    this.reload_for_open(loaded.path, window, cx);
                                }
                                Intent::Close => this.request_replace(Intent::Close, window, cx),
                            }
                        }
                    }
                    Err(SaveError::Failed(error)) => {
                        this.failure(format!("Save failed: {error}"), window, cx)
                    }
                    Err(SaveError::Conflict(observed)) => {
                        this.confirm_overwrite(path, snapshot, observed, window, cx)
                    }
                }
            });
        })
        .detach();
    }

    fn reload_for_open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true;
        let work = cx
            .background_executor()
            .spawn(async move { persistence::load(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(loaded) => this.request_replace(Intent::Open(Box::new(loaded)), window, cx),
                    Err(error) => {
                        this.failure(format!("Open failed after save: {error}"), window, cx)
                    }
                }
            });
        })
        .detach();
    }

    fn confirm_overwrite(
        &mut self,
        path: PathBuf,
        snapshot: TextSnapshot,
        observed: Option<DiskVersion>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(PromptLevel::Warning, "Destination exists or changed outside the editor",
            Some(&format!("{}\nOverwrite with the captured buffer snapshot? Cancel keeps your document and the disk file.", path.display())),
            &[PromptButton::Cancel("Cancel".into()), PromptButton::Other("Overwrite".into())], cx);
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if matches!(answer, Ok(1)) {
                    this.write_snapshot(path, snapshot, observed, window, cx);
                } else {
                    this.cancel("Save cancelled - document retained", window, cx);
                }
            });
        })
        .detach();
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let position = self.editor.read(cx).cursor();
        let dirty = self.editor.read(cx).is_dirty();
        let path = self
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Untitled".into());
        let label = format!("{}{}", if dirty { "* " } else { "" }, path);
        window.set_window_title(&format!("A Less Awful Editor - {label}"));
        let tabs = [
            Panel::Terminal,
            Panel::Problems,
            Panel::Debug,
            Panel::Output,
        ]
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
            .on_action(cx.listener(|this, _: &Open, window, cx| this.request_open(window, cx)))
            .on_action(cx.listener(|this, _: &Save, window, cx| this.request_save(false, window, cx)))
            .on_action(cx.listener(|this, _: &SaveAs, window, cx| this.request_save(true, window, cx)))
            .on_action(cx.listener(|this, _: &Close, window, cx| this.request_replace(Intent::Close, window, cx)))
            .on_action(cx.listener(|this, _: &Quit, window, cx| this.request_replace(Intent::Close, window, cx)))
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
                    .child("A Less Awful Editor  /  Open Ctrl+O  /  Save Ctrl+S  /  Save As Ctrl+Shift+S"),
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
                                    .overflow_hidden()
                                    .child(label),
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
                            .id("panel-message")
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_3()
                            .text_sm()
                            .whitespace_normal()
                            .text_color(rgb(0x8d94a3))
                            .child(if self.panel == Panel::Output { self.status.clone() } else { self.panel.message().into() }),
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
                    .overflow_hidden()
                    .child(self.status.clone())
                    .child(format!(
                        "Ln {}, Col {}",
                        position.line + 1,
                        position.column + 1
                    )),
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{ClipboardItem, TestAppContext, VisualTestContext};

    fn contents(shell: &Entity<AppShell>, cx: &mut VisualTestContext) -> (Vec<u8>, bool) {
        cx.update(|_, cx| {
            shell.update(cx, |shell, cx| {
                shell.editor.update(cx, |editor, _| {
                    let mut bytes = Vec::new();
                    editor.save_snapshot().write_to(&mut bytes).unwrap();
                    (bytes, editor.is_dirty())
                })
            })
        })
    }

    #[gpui::test]
    fn save_as_cancel_failure_success_and_undo_dirty(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved.txt");
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("hello e\u{301}👩‍💻");
        let original = contents(&shell, cx);
        assert!(original.1);
        cx.dispatch_action(SaveAs);
        cx.simulate_new_path_selection(|_| None);
        cx.run_until_parked();
        assert_eq!(contents(&shell, cx), original);
        cx.dispatch_action(SaveAs);
        cx.simulate_new_path_selection(|_| Some(dir.path().join("missing/file")));
        cx.run_until_parked();
        assert_eq!(contents(&shell, cx), original);
        cx.dispatch_action(SaveAs);
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        cx.run_until_parked();
        assert_eq!(std::fs::read(&path).unwrap(), original.0);
        assert!(!contents(&shell, cx).1);
        cx.simulate_input("new");
        assert!(contents(&shell, cx).1);
        // The native simulator delivers each scalar as a separate input edit.
        cx.simulate_keystrokes("ctrl-z ctrl-z ctrl-z");
        assert!(!contents(&shell, cx).1);
        cx.simulate_keystrokes("ctrl-y ctrl-y ctrl-y");
        assert!(contents(&shell, cx).1);
    }

    #[gpui::test]
    fn close_and_replacement_cancel_save_discard(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let incoming = dir.path().join("incoming.txt");
        let outgoing = dir.path().join("outgoing.txt");
        std::fs::write(&incoming, "incoming\r\n").unwrap();
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("keep me");
        for action in [false, true] {
            if action {
                cx.dispatch_action(Quit);
            } else {
                cx.dispatch_action(Close);
            }
            assert!(cx.has_pending_prompt());
            cx.simulate_prompt_answer("Cancel");
            cx.run_until_parked();
            assert_eq!(contents(&shell, cx), (b"keep me".to_vec(), true));
        }
        let replace = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                shell.update(cx, |shell, cx| {
                    shell.request_replace(
                        Intent::Open(Box::new(persistence::load(&incoming).unwrap())),
                        window,
                        cx,
                    );
                })
            });
        };
        replace(cx);
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(contents(&shell, cx).0, b"keep me");
        replace(cx);
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        cx.simulate_new_path_selection(|_| None);
        cx.run_until_parked();
        assert_eq!(contents(&shell, cx).0, b"keep me");
        replace(cx);
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        cx.simulate_new_path_selection(|_| Some(outgoing.clone()));
        cx.run_until_parked();
        assert_eq!(std::fs::read(outgoing).unwrap(), b"keep me");
        assert_eq!(contents(&shell, cx), (b"incoming\r\n".to_vec(), false));
        cx.simulate_input("discard this");
        replace(cx);
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        assert_eq!(contents(&shell, cx), (b"incoming\r\n".to_vec(), false));
    }

    #[gpui::test]
    fn selection_multiline_clipboard_conflict_cancel_and_overwrite(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("replace me");
        cx.simulate_keystrokes("ctrl-a");
        cx.write_to_clipboard(ClipboardItem::new_string("e\u{301}👩‍💻\r\n東京\nlast".into()));
        cx.simulate_keystrokes("ctrl-v");
        let pasted = contents(&shell, cx).0;
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(contents(&shell, cx).0, b"replace me");
        cx.simulate_keystrokes("ctrl-y");
        assert_eq!(contents(&shell, cx).0, pasted);
        cx.dispatch_action(Save);
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        cx.run_until_parked();
        std::fs::write(&path, "external").unwrap();
        cx.simulate_input("new");
        let current = contents(&shell, cx);
        cx.dispatch_action(Save);
        cx.run_until_parked();
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(contents(&shell, cx), current);
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        cx.dispatch_action(Save);
        cx.run_until_parked();
        cx.simulate_prompt_answer("Overwrite");
        cx.run_until_parked();
        assert_eq!(std::fs::read(&path).unwrap(), current.0);
        assert!(!contents(&shell, cx).1);
    }
    #[gpui::test]
    fn saving_before_reopening_same_file_reads_the_new_saved_contents(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same.txt");
        std::fs::write(&path, "old").unwrap();
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.apply_intent(
                    Intent::Open(Box::new(persistence::load(&path).unwrap())),
                    window,
                    cx,
                );
            })
        });
        cx.simulate_input("new");
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.request_replace(
                    Intent::Open(Box::new(persistence::load(&path).unwrap())),
                    window,
                    cx,
                );
            })
        });
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        assert_eq!(contents(&shell, cx), (b"newold".to_vec(), false));
        assert_eq!(std::fs::read(path).unwrap(), b"newold");
    }

    #[gpui::test]
    fn edits_during_save_prevent_pending_close_and_require_another_decision(
        cx: &mut TestAppContext,
    ) {
        use gpui::EntityInputHandler;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.txt");
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("snapshot");
        // Queue I/O and deliver another native-input event before the executor
        // can complete the save. This deterministically exercises the ordering.
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.pending = Some(Intent::Close);
                shell.busy = true;
                let snapshot = shell.editor.update(cx, |editor, _| editor.save_snapshot());
                shell.write_snapshot(path.clone(), snapshot, None, window, cx);
                shell.editor.update(cx, |editor, cx| {
                    editor.replace_text_in_range(None, " newer", window, cx)
                });
            })
        });
        cx.run_until_parked();
        assert_eq!(std::fs::read(path).unwrap(), b"snapshot");
        assert_eq!(contents(&shell, cx), (b"snapshot newer".to_vec(), true));
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert!(contents(&shell, cx).1);
    }
    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn linux_confirmation_escape_cancels_and_restores_input_focus(cx: &mut TestAppContext) {
        cx.update(prompts::init);
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("keep");
        cx.dispatch_action(Quit);
        cx.simulate_keystrokes("escape");
        cx.simulate_input("x");
        assert_eq!(contents(&shell, cx), (b"keepx".to_vec(), true));
        cx.dispatch_action(Close);
        // The focused default is Cancel, so Enter must also preserve work.
        cx.simulate_keystrokes("enter");
        cx.simulate_input("y");
        assert_eq!(contents(&shell, cx), (b"keepxy".to_vec(), true));
    }
}
