//! Systems document lifecycle. Text and binary snapshots have independent ownership.
use super::*;
use ale_systems_core::{BinaryImage, SourceLocation};
use systems::WorkbenchEvent;

pub(super) enum BinaryIntent {
    Open(PathBuf, Box<BinaryImage>),
    Close,
}

impl AppShell {
    pub(super) fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.systems_active
            && let Some(workbench) = &self.workbench
        {
            workbench.read(cx).focus_handle(cx).focus(window);
        } else {
            self.editor.read(cx).focus_handle(cx).focus(window);
        }
    }

    pub(super) fn show_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.systems_active = false;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(super) fn show_systems(&mut self, lens: Lens, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_systems(Some(lens), window, cx);
    }

    pub(super) fn resume_systems(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_systems(None, window, cx);
    }

    fn activate_systems(
        &mut self,
        lens: Option<Lens>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workbench) = &self.workbench else {
            self.status =
                "Inspect a binary first (Ctrl+Shift+O). Your text buffer is retained.".into();
            cx.notify();
            return;
        };
        if !self.systems_active {
            let line = self.editor.read(cx).cursor().line;
            let offset = self
                .path
                .as_ref()
                .and_then(|path| workbench.read(cx).location_for_source(path, line));
            if let Some(offset) = offset {
                workbench.update(cx, |workbench, cx| workbench.go(offset, cx));
                self.status = "Mapped through build debug information. Source edits are not rebuilt automatically.".into();
            } else {
                self.status =
                    "No build mapping for this source line; retained the binary location.".into();
            }
        }
        self.systems_active = true;
        if let Some(lens) = lens {
            workbench.update(cx, |workbench, cx| workbench.show(lens, window, cx));
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    pub(super) fn save_active(
        &mut self,
        save_as: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.systems_active {
            self.request_export(window, cx);
        } else {
            self.request_save(save_as, window, cx);
        }
    }

    pub(super) fn request_inspect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "Choose a binary to inspect. It will not be executed.".into();
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Inspect binary".into()),
        });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let path = match picker.await {
                Ok(Ok(Some(paths))) if paths.len() == 1 => paths.into_iter().next().unwrap(),
                Ok(Ok(_)) => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.cancel("Inspection cancelled; documents retained", window, cx)
                    });
                    return;
                }
                error => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.failure(format!("Binary dialog failed: {error:?}"), window, cx)
                    });
                    return;
                }
            };
            let worker_path = path.clone();
            let work = cx.background_executor().spawn(async move {
                systems_io::read_binary(&worker_path).and_then(BinaryImage::parse)
            });
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(image) => this.request_binary_replace(
                        BinaryIntent::Open(path, Box::new(image)),
                        window,
                        cx,
                    ),
                    Err(error) => this.failure(format!("Inspection failed: {error}"), window, cx),
                }
            });
        })
        .detach();
    }

    pub(super) fn request_binary_replace(
        &mut self,
        intent: BinaryIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        if !matches!(intent, BinaryIntent::Close) {
            self.close_text_revision = None;
            self.close_binary_revision = None;
        }
        if let Some(workbench) = &self.workbench {
            if workbench.read(cx).is_busy() {
                self.close_text_revision = None;
                self.close_binary_revision = None;
                self.status =
                    "Wait for the current binary operation, then try again. Documents retained."
                        .into();
                cx.notify();
                return;
            }
            let close_approved = matches!(intent, BinaryIntent::Close)
                && self.close_binary_revision
                    == Some((workbench.entity_id(), workbench.read(cx).snapshot().1));
            if workbench.read(cx).is_dirty() && !close_approved {
                self.busy = true;
                let answer = window.prompt(PromptLevel::Warning, "Export binary patches before continuing?",
                    Some("Patches exist only in memory. Export writes a new file and keeps the original untouched."),
                    &[PromptButton::Ok("Export copy".into()), PromptButton::Other("Discard patches".into()), PromptButton::Cancel("Cancel".into())], cx);
                cx.spawn_in(window, async move |this, cx| {
                    let answer = answer.await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.busy = false;
                        match answer {
                            Ok(0) => {
                                this.binary_pending = Some(intent);
                                this.request_export(window, cx);
                            }
                            Ok(1) => {
                                if matches!(intent, BinaryIntent::Close) {
                                    this.close_binary_revision =
                                        this.workbench.as_ref().map(|workbench| {
                                            (workbench.entity_id(), workbench.read(cx).snapshot().1)
                                        });
                                    this.request_replace(Intent::Close, window, cx);
                                } else {
                                    this.apply_binary_intent(intent, window, cx);
                                }
                            }
                            _ => this.cancel("Cancelled; text and binary retained", window, cx),
                        }
                    });
                })
                .detach();
                return;
            }
        }
        self.apply_binary_intent(intent, window, cx);
    }

    fn apply_binary_intent(
        &mut self,
        intent: BinaryIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match intent {
            BinaryIntent::Close => cx.quit(),
            BinaryIntent::Open(path, image) => {
                let workbench = cx.new(|cx| Workbench::new(path, *image, cx));
                cx.observe(&workbench, |_, _, cx| cx.notify()).detach();
                cx.subscribe_in(
                    &workbench,
                    window,
                    |this, _, event, window, cx| match event {
                        WorkbenchEvent::Source(location) => {
                            this.request_source(location.clone(), window, cx)
                        }
                        WorkbenchEvent::Export => this.request_export(window, cx),
                    },
                )
                .detach();
                self.workbench = Some(workbench);
                self.systems_active = true;
                self.panels_visible = false;
                self.finish(
                    "Binary loaded. Text buffer retained; original binary is never overwritten.",
                    window,
                    cx,
                );
            }
        }
    }

    fn request_source(
        &mut self,
        location: SourceLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let path = PathBuf::from(location.path);
        let line = location.line.saturating_sub(1) as usize;
        if self.path.as_ref() == Some(&path) {
            self.editor
                .update(cx, |editor, cx| editor.go_to_line(line, cx));
            self.show_editor(window, cx);
            self.status =
                "Source location from build debug information; current source may have changed."
                    .into();
            return;
        }
        self.busy = true;
        self.status = "Opening source referenced by build debug information...".into();
        let work = cx
            .background_executor()
            .spawn(async move { persistence::load(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(loaded) => {
                        this.request_replace(Intent::Source(Box::new(loaded), line), window, cx)
                    }
                    Err(error) => this.failure(
                        format!("Source unavailable: {error}. Binary retained."),
                        window,
                        cx,
                    ),
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn request_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(workbench) = self.workbench.clone() else {
            return;
        };
        if workbench.read(cx).is_busy() {
            self.status = "Wait for the binary operation before exporting.".into();
            cx.notify();
            return;
        }
        self.busy = true;
        let path = &workbench.read(cx).path;
        let name = format!(
            "{}.patched",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        let picker = cx.prompt_for_new_path(
            path.parent().unwrap_or(std::path::Path::new(".")),
            Some(&name),
        );
        self.status = "Export to a NEW file; existing destinations are refused.".into();
        cx.spawn_in(window, async move |this, cx| {
            let result = picker.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(Ok(Some(path))) => this.export_snapshot(path, window, cx),
                Ok(Ok(None)) => {
                    this.cancel("Export cancelled; binary patches retained", window, cx)
                }
                error => this.failure(format!("Export dialog failed: {error:?}"), window, cx),
            });
        })
        .detach();
        cx.notify();
    }

    fn export_snapshot(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbench) = self.workbench.clone() else {
            self.cancel("No binary to export", window, cx);
            return;
        };
        let (image, revision) = workbench.read(cx).snapshot();
        self.status = "Exporting captured binary snapshot...".into();
        let work = cx
            .background_executor()
            .spawn(async move { systems_io::export_copy(&path, image.bytes()) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        workbench.update(cx, |workbench, cx| workbench.mark_exported(revision, cx));
                        this.finish(
                            "Binary snapshot exported to a new file. Original input unchanged.",
                            window,
                            cx,
                        );
                        if let Some(intent) = this.binary_pending.take() {
                            match intent {
                                // Recheck BOTH buffers after asynchronous export.
                                BinaryIntent::Close => {
                                    this.request_replace(Intent::Close, window, cx)
                                }
                                intent => this.request_binary_replace(intent, window, cx),
                            }
                        }
                    }
                    Err(error) => this.failure(
                        format!("Export failed: {error}. Patches retained."),
                        window,
                        cx,
                    ),
                }
            });
        })
        .detach();
        cx.notify();
    }
}
