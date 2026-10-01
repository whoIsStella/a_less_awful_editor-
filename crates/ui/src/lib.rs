mod decompiler;
mod persistence;
pub mod prompts;
mod systems;
mod systems_io;
mod systems_shell;

use ale_editor_core::TextSnapshot;
use ale_editor_view::EditorView;
use gpui::{
    Context, Div, Entity, Focusable, IntoElement, PathPromptOptions, PromptButton, PromptLevel,
    Render, Window, actions, div, prelude::*, px, rgb,
};
use persistence::{DiskVersion, Loaded, SaveError};
use std::path::PathBuf;
use systems::{Lens, Workbench};
use systems_shell::BinaryIntent;

actions!(
    document,
    [
        Open,
        Save,
        SaveAs,
        Close,
        Quit,
        InspectBinary,
        ShowEditor,
        ShowAssembly,
        ShowBytes,
        ShowOverview,
        TogglePanels
    ]
);

enum Intent {
    Open(Box<Loaded>),
    Source(Box<Loaded>, usize),
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
    workbench: Option<Entity<Workbench>>,
    systems_active: bool,
    panels_visible: bool,
    binary_pending: Option<BinaryIntent>,
    close_text_revision: Option<u64>,
    close_binary_revision: Option<(gpui::EntityId, u64)>,
}

impl AppShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| EditorView::new("", cx));
        editor.read(cx).focus_handle(cx).focus(window);
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        let shell = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            // GPUI 0.2.2's X11 close callback still holds the platform state
            // borrow. Quit (and prompts) must run after that callback unwinds;
            // defer() would flush inside the same update and is too early.
            let shell = shell.clone();
            window
                .spawn(cx, async move |cx| {
                    let _ = shell.update_in(cx, |this, window, cx| {
                        this.request_replace(Intent::Close, window, cx)
                    });
                })
                .detach();
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
            workbench: None,
            systems_active: false,
            panels_visible: false,
            binary_pending: None,
            close_text_revision: None,
            close_binary_revision: None,
        }
    }

    fn finish(&mut self, message: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = false;
        self.status = message.into();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn cancel(&mut self, message: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.pending = None;
        self.binary_pending = None;
        self.close_text_revision = None;
        self.close_binary_revision = None;
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
        let close_approved = matches!(intent, Intent::Close)
            && self.close_text_revision == Some(self.editor.read(cx).content_revision());
        if !matches!(intent, Intent::Close) {
            self.close_text_revision = None;
            self.close_binary_revision = None;
        }
        if !self.editor.read(cx).is_dirty() || close_approved {
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
            Intent::Close => {
                self.close_text_revision = Some(self.editor.read(cx).content_revision());
                self.request_binary_replace(BinaryIntent::Close, window, cx);
            }
            Intent::Open(_) | Intent::Source(_, _) => {
                let (loaded, line) = match intent {
                    Intent::Open(loaded) => (loaded, None),
                    Intent::Source(loaded, line) => (loaded, Some(line)),
                    Intent::Close => unreachable!(),
                };
                let Loaded {
                    path,
                    buffer,
                    version,
                } = *loaded;
                self.path = Some(path);
                self.disk = Some(version);
                self.editor.update(cx, |editor, cx| editor.load(buffer, cx));
                if let Some(line) = line {
                    self.editor
                        .update(cx, |editor, cx| editor.go_to_line(line, cx));
                }
                self.systems_active = false;
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
                                    this.reload_for_open(loaded.path, None, window, cx);
                                }
                                Intent::Source(loaded, line) => {
                                    this.reload_for_open(loaded.path, Some(line), window, cx)
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

    fn reload_for_open(
        &mut self,
        path: PathBuf,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        let work = cx
            .background_executor()
            .spawn(async move { persistence::load(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(loaded) => this.request_replace(
                        match line {
                            Some(line) => Intent::Source(Box::new(loaded), line),
                            None => Intent::Open(Box::new(loaded)),
                        },
                        window,
                        cx,
                    ),
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
        let text_dirty = self.editor.read(cx).is_dirty();
        let text_name = self
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        let binary = self.workbench.as_ref().map(|workbench| {
            let workbench = workbench.read(cx);
            let name = workbench
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| workbench.path.display().to_string());
            (name, workbench.is_dirty(), workbench.offset)
        });
        let (active_name, active_dirty, location) = if self.systems_active {
            let (name, dirty, offset) = binary.as_ref().expect("active systems view");
            (name.clone(), *dirty, format!("File offset {offset:#x}"))
        } else {
            (
                text_name.clone(),
                text_dirty,
                format!("Ln {}, Col {}", position.line + 1, position.column + 1),
            )
        };
        window.set_window_title(&format!(
            "A Less Awful Editor - {}{}",
            active_name,
            if active_dirty { " *" } else { "" }
        ));
        // Keep room for the binary command bar, an expanded Details section,
        // and several data rows when the optional placeholder panel is open.
        let panel_height = if self.systems_active {
            (window.viewport_size().height - px(420.0)).clamp(px(28.0), px(104.0))
        } else {
            px(104.0)
        };
        let tabs = [
            Panel::Terminal,
            Panel::Problems,
            Panel::Debug,
            Panel::Output,
        ]
        .into_iter()
        .map(|panel| {
            let active = panel == self.panel;
            let label = match panel {
                Panel::Terminal => "Terminal",
                Panel::Problems => "Problems",
                Panel::Debug => "Debug",
                Panel::Output => "Output",
            };
            div()
                .id(panel.label())
                .px_3()
                .h_full()
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(px(11.0))
                .text_color(rgb(if active { 0xd4dce7 } else { 0x8590a3 }))
                .border_b_1()
                .border_color(rgb(if active { 0x7aa2f7 } else { 0x15191f }))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.panel = panel;
                    cx.notify();
                }))
        });
        div()
            .on_action(cx.listener(|this, _: &Open, window, cx| this.request_open(window, cx)))
            .on_action(
                cx.listener(|this, _: &Save, window, cx| this.save_active(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SaveAs, window, cx| this.save_active(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &InspectBinary, window, cx| this.request_inspect(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowEditor, window, cx| this.show_editor(window, cx)))
            .on_action(cx.listener(|this, _: &ShowAssembly, window, cx| {
                this.show_systems(Lens::Assembly, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowBytes, window, cx| {
                this.show_systems(Lens::Bytes, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowOverview, window, cx| {
                this.show_systems(Lens::Overview, window, cx)
            }))
            .on_action(cx.listener(|this, _: &TogglePanels, _, cx| {
                this.panels_visible = !this.panels_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Close, window, cx| {
                this.request_replace(Intent::Close, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                this.request_replace(Intent::Close, window, cx)
            }))
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .font_family("Inter")
            .text_size(px(12.0))
            .bg(rgb(0x101216))
            .text_color(rgb(0xd4dce7))
            .child(
                div()
                    .h(px(28.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px_3()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(0x2a313c))
                    .bg(rgb(0x15191f))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.0))
                            .text_color(rgb(0x8590a3))
                            .child("A Less Awful Editor"),
                    )
                    .child(
                        systems::button("Open").on_click(
                            cx.listener(|this, _, window, cx| this.request_open(window, cx)),
                        ),
                    )
                    .child(systems::button("Inspect").on_click(
                        cx.listener(|this, _, window, cx| this.request_inspect(window, cx)),
                    ))
                    .child(systems::button("Save").on_click(
                        cx.listener(|this, _, window, cx| this.save_active(false, window, cx)),
                    ))
                    .child(
                        systems::button("Panels")
                            .when(self.panels_visible, |button| {
                                button.bg(rgb(0x1b2028)).text_color(rgb(0xd4dce7))
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panels_visible = !this.panels_visible;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .h(px(30.0))
                    .flex_shrink_0()
                    .flex()
                    .min_w_0()
                    .overflow_hidden()
                    .border_b_1()
                    .border_color(rgb(0x2a313c))
                    .bg(rgb(0x15191f))
                    .child(
                        div()
                            .id("text-document-tab")
                            .h_full()
                            .min_w_0()
                            .max_w(px(240.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(rgb(0x2a313c))
                            .text_color(rgb(0x8590a3))
                            .when(!self.systems_active, |tab| {
                                tab.bg(rgb(0x1b2028))
                                    .border_color(rgb(0x7aa2f7))
                                    .text_color(rgb(0xd4dce7))
                            })
                            .child(div().min_w_0().truncate().child(text_name))
                            .when(text_dirty, |tab| {
                                tab.child(
                                    div()
                                        .size(px(5.0))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .bg(rgb(0x7aa2f7)),
                                )
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.show_editor(window, cx)),
                            ),
                    )
                    .when_some(binary, |bar, (name, dirty, _)| {
                        bar.child(
                            div()
                                .id("binary-document-tab")
                                .h_full()
                                .min_w_0()
                                .max_w(px(240.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .border_b_1()
                                .border_color(rgb(0x2a313c))
                                .text_color(rgb(0x8590a3))
                                .when(self.systems_active, |tab| {
                                    tab.bg(rgb(0x1b2028))
                                        .border_color(rgb(0x7aa2f7))
                                        .text_color(rgb(0xd4dce7))
                                })
                                .child(div().min_w_0().truncate().child(name))
                                .when(dirty, |tab| {
                                    tab.child(
                                        div()
                                            .size(px(5.0))
                                            .flex_shrink_0()
                                            .rounded_full()
                                            .bg(rgb(0x7aa2f7)),
                                    )
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.resume_systems(window, cx)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.panels_visible && !self.systems_active, |row| {
                        row.child(file_placeholder())
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .when(!self.systems_active, |column| {
                                column.when_some(self.path.as_ref(), |column, path| {
                                    column.child(
                                        div()
                                            .h(px(24.0))
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .px_3()
                                            .bg(rgb(0x101216))
                                            .text_size(px(11.0))
                                            .text_color(rgb(0x8590a3))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .child(path.display().to_string()),
                                            ),
                                    )
                                })
                            })
                            .child(div().flex_1().min_h_0().child(if self.systems_active {
                                self.workbench
                                    .as_ref()
                                    .expect("active systems view")
                                    .clone()
                                    .into_any_element()
                            } else {
                                self.editor.clone().into_any_element()
                            })),
                    ),
            )
            .when(self.panels_visible, |root| {
                root.child(
                    div()
                        .h(panel_height)
                        .flex_shrink_0()
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .border_t_1()
                        .border_color(rgb(0x2a313c))
                        .bg(rgb(0x15191f))
                        .child(div().h(px(28.0)).flex_shrink_0().flex().children(tabs))
                        .child(
                            div()
                                .id("panel-message")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .px_3()
                                .py_2()
                                .text_size(px(11.0))
                                .whitespace_normal()
                                .text_color(rgb(0x8590a3))
                                .child(if self.panel == Panel::Output {
                                    self.status.clone()
                                } else {
                                    self.panel.message().into()
                                }),
                        ),
                )
            })
            .child(
                div()
                    .h(px(22.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .border_t_1()
                    .border_color(rgb(0x2a313c))
                    .bg(rgb(0x15191f))
                    .text_color(rgb(0x8590a3))
                    .text_size(px(11.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.status.clone()),
                    )
                    .child(div().max_w(px(220.0)).min_w_0().truncate().child(location)),
            )
    }
}

fn file_placeholder() -> Div {
    div()
        .w(px(170.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(rgb(0x2a313c))
        .bg(rgb(0x15191f))
        .child(
            div()
                .h(px(28.0))
                .flex()
                .items_center()
                .px_3()
                .text_size(px(11.0))
                .text_color(rgb(0xd4dce7))
                .child("Files"),
        )
        .child(
            div()
                .px_3()
                .py_2()
                .text_size(px(11.0))
                .text_color(rgb(0x8590a3))
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
