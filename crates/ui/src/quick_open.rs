use super::*;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Default, PartialEq)]
struct FileIndex {
    files: Vec<PathBuf>,
    skipped: usize,
    truncated: bool,
}

impl FileIndex {
    fn scan(root: &Path, cancel: &AtomicBool) -> Result<Option<Self>, String> {
        let mut index = Self::default();
        for entry in workspace_search::workspace_walk(root)?.build() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            match entry {
                Ok(entry) if entry.file_type().is_some_and(|t| t.is_file()) => {
                    if let Ok(relative) = entry.path().strip_prefix(root) {
                        if index.files.len() == 50_000 {
                            index.truncated = true;
                            break;
                        }
                        index.files.push(relative.to_owned());
                    }
                }
                Ok(_) => {}
                Err(_) => index.skipped += 1,
            }
        }
        index.files.sort_unstable();
        Ok(Some(index))
    }

    fn matches(&self, query: String) -> impl Iterator<Item = &PathBuf> {
        let query = query.to_lowercase();
        self.files
            .iter()
            .filter(move |p| p.to_string_lossy().to_lowercase().contains(&query))
    }
}

pub(super) struct FilePicker {
    input: Entity<EditorView>,
    cancel: Option<Arc<AtomicBool>>,
    index: FileIndex,
    message: String,
}
impl FilePicker {
    pub(super) fn new(window: &mut Window, cx: &mut Context<AppShell>) -> Self {
        let input = cx.new(|cx| EditorView::input("", cx));
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(
            &input,
            window,
            |shell, _, _: &ale_editor_view::Submit, window, cx| {
                let first = shell
                    .quick_open
                    .index
                    .matches(shell.quick_open.input.read(cx).text())
                    .next()
                    .cloned();
                if let Some(path) = first {
                    shell.pick_file(path, window, cx);
                }
            },
        )
        .detach();
        Self {
            input,
            cancel: None,
            index: FileIndex::default(),
            message: String::new(),
        }
    }
    pub(super) fn visible(&self) -> bool {
        self.cancel.is_some()
    }
    pub(super) fn close(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.index = FileIndex::default();
    }
}
impl Drop for FilePicker {
    fn drop(&mut self) {
        self.close();
    }
}
impl AppShell {
    pub(super) fn show_file_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.explorer_tree.root.clone() else {
            self.status = "Open a folder to use Quick Open.".into();
            cx.notify();
            return;
        };
        self.quick_open.close();
        self.quick_open.message = "Indexing files…".into();
        self.quick_open.input.update(cx, |field, cx| {
            field.edit_command(ale_editor_view::EditCommand::SelectAll, cx)
        });
        let cancel = Arc::new(AtomicBool::new(false));
        self.quick_open.cancel = Some(cancel.clone());
        self.quick_open
            .input
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.spawn(async move |this, cx| {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let path = root.clone();
                let flag = cancel.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { FileIndex::scan(&path, &flag) })
                    .await;
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if this
                    .update(cx, |shell, cx| {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        match result {
                            Ok(Some(index)) => {
                                let mut message = format!("{} files", index.files.len());
                                if index.truncated {
                                    message.push_str(" · index limit reached");
                                }
                                if index.skipped > 0 {
                                    message.push_str(&format!(
                                        " · {} unreadable entries",
                                        index.skipped
                                    ));
                                }
                                if index != shell.quick_open.index
                                    || message != shell.quick_open.message
                                {
                                    shell.quick_open.index = index;
                                    shell.quick_open.message = message;
                                    cx.notify();
                                }
                            }
                            Err(error) => {
                                shell.quick_open.message = error;
                                cx.notify();
                            }
                            Ok(None) => {}
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        })
        .detach();
        cx.notify();
    }

    fn pick_file(&mut self, relative: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !self.quick_open.visible() || self.busy {
            return;
        }
        if let Some(root) = &self.explorer_tree.root {
            let path = root.join(relative);
            self.quick_open.close();
            if self.path.as_ref() == Some(&path) {
                self.show_editor(window, cx);
            } else if let Some(index) = self
                .documents
                .iter()
                .position(|d| d.path.as_ref() == Some(&path))
            {
                self.activate_document(index, window, cx);
            } else {
                self.open_text_path(path, window, cx);
            }
            cx.notify();
        }
    }

    pub(super) fn file_picker_view(&self, cx: &Context<Self>) -> Div {
        let mut matches = self
            .quick_open
            .index
            .matches(self.quick_open.input.read(cx).text());
        let rows = matches
            .by_ref()
            .take(100)
            .enumerate()
            .map(|(i, path)| {
                let path = path.clone();
                div()
                    .id(("quick-file", i))
                    .cursor_pointer()
                    .px_2()
                    .py_1()
                    .hover(|style| style.bg(rgb(0x30436a)))
                    .child(path.display().to_string())
                    .on_click(cx.listener(move |shell, _, window, cx| {
                        shell.pick_file(path.clone(), window, cx)
                    }))
            })
            .collect::<Vec<_>>();
        div()
            .w(px(330.))
            .max_w(gpui::relative(0.5))
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .bg(rgb(0x15191f))
            .on_key_down(
                cx.listener(|shell, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        shell.quick_open.close();
                        shell.show_editor(window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(
                div().flex().justify_between().child("Quick Open").child(
                    div()
                        .id("close-file-picker")
                        .cursor_pointer()
                        .child("×")
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.quick_open.close();
                            shell.show_editor(window, cx);
                            cx.notify();
                        })),
                ),
            )
            .child(
                div()
                    .h(px(26.))
                    .bg(rgb(0x202630))
                    .child(self.quick_open.input.clone()),
            )
            .child(self.quick_open.message.clone())
            .when(
                rows.is_empty() && !self.quick_open.input.read(cx).text().is_empty(),
                |view| view.child("No matching paths"),
            )
            .when(matches.next().is_some(), |view| {
                view.child("Showing 100 matches. Type to narrow.")
            })
            .child(
                div()
                    .id("quick-files")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn picker_refreshes_while_open_and_discards_previous_project(cx: &mut gpui::TestAppContext) {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir(first.path().join("nested")).unwrap();
        let before = first.path().join("nested/before.txt");
        let after = first.path().join("nested/after.txt");
        std::fs::write(&before, "saved").unwrap();
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                s.select_folder(first.path().into(), window, cx);
                s.show_file_picker(window, cx);
            })
        });
        cx.run_until_parked();
        cx.simulate_input("nested/");
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).quick_open.index.files,
                [PathBuf::from("nested/before.txt")]
            )
        });
        std::fs::rename(before, &after).unwrap();
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).quick_open.index.files,
                [PathBuf::from("nested/after.txt")]
            )
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                assert_eq!(s.path.as_ref(), Some(&after));
                s.show_file_picker(window, cx);
                s.select_folder(second.path().into(), window, cx);
                s.show_file_picker(window, cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert!(shell.read(cx).quick_open.index.files.is_empty()));
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| assert!(!shell.read(cx).quick_open.visible()));
    }

    #[test]
    fn refresh_finds_nested_paths_and_tracks_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join(".ignore"), "*.tmp\n").unwrap();
        std::fs::write(root.join("src/deep/file.tmp"), "").unwrap();
        let first = root.join("src/deep/file.rs");
        let second = root.join("src/deep/new.rs");
        let flag = AtomicBool::new(false);
        let scan = || FileIndex::scan(root, &flag).unwrap().unwrap();
        assert_eq!(scan().matches("deep/".into()).count(), 0);
        std::fs::write(&first, "").unwrap();
        assert_eq!(scan().matches("DEEP/F".into()).count(), 1);
        std::fs::rename(&first, &second).unwrap();
        assert_eq!(scan().matches("deep/f".into()).count(), 0);
        assert_eq!(scan().matches("deep/new".into()).count(), 1);
        std::fs::remove_file(second).unwrap();
        assert_eq!(scan().matches("deep/".into()).count(), 0);
        flag.store(true, Ordering::Relaxed);
        assert!(FileIndex::scan(root, &flag).unwrap().is_none());
    }
}
