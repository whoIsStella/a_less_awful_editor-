use super::*;
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use workspace_search::{Overlay, SearchOptions, SearchReport};

pub(super) struct SearchState {
    pub visible: bool,
    pub(super) query: Entity<EditorView>,
    pub(super) include: Entity<EditorView>,
    pub(super) exclude: Entity<EditorView>,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    generation: u64,
    page: usize,
    cancel: Option<Arc<AtomicBool>>,
    pub report: Option<SearchReport>,
    pub(super) input_revisions: [u64; 3],
    root: Option<PathBuf>,
}

fn same_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
}

impl SearchState {
    pub fn new(cx: &mut Context<AppShell>) -> Self {
        Self {
            visible: false,
            query: cx.new(|cx| EditorView::input("", cx)),
            include: cx.new(|cx| EditorView::input("", cx)),
            exclude: cx.new(|cx| EditorView::input("", cx)),
            case_sensitive: false,
            whole_word: false,
            regex: false,
            generation: 0,
            page: 0,
            cancel: None,
            report: None,
            input_revisions: [0; 3],
            root: None,
        }
    }
}

impl AppShell {
    pub(super) fn cancel_workspace_search(&mut self) {
        if let Some(cancel) = self.workspace_search.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.workspace_search.generation = self.workspace_search.generation.wrapping_add(1);
    }

    pub(super) fn show_workspace_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace_search.visible = true;
        self.workspace_search
            .query
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.notify();
    }

    pub(super) fn run_workspace_search(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.explorer_tree.root.clone() else {
            self.status = "Open a folder to search the workspace.".into();
            cx.notify();
            return;
        };
        let options = SearchOptions {
            query: self.workspace_search.query.read(cx).text(),
            include: self.workspace_search.include.read(cx).text(),
            exclude: self.workspace_search.exclude.read(cx).text(),
            case_sensitive: self.workspace_search.case_sensitive,
            whole_word: self.workspace_search.whole_word,
            regex: self.workspace_search.regex,
        };
        if options.query.is_empty() {
            self.status = "Enter text to search.".into();
            cx.notify();
            return;
        }
        self.cancel_workspace_search();
        let mut overlays = Vec::new();
        for (path, editor) in std::iter::once((self.path.as_ref(), &self.editor)).chain(
            self.documents
                .iter()
                .map(|document| (document.path.as_ref(), &document.editor)),
        ) {
            if let Some(path) = path {
                overlays.push(Overlay {
                    path: path.clone(),
                    text: editor.read(cx).text(),
                });
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.workspace_search.cancel = Some(cancel.clone());
        self.workspace_search.root = Some(root.clone());
        self.workspace_search.report = None;
        self.workspace_search.page = 0;
        let generation = self.workspace_search.generation;
        self.status = "Searching workspace…".into();
        let work = cx
            .background_executor()
            .spawn(async move { workspace_search::search(&root, overlays, options, &cancel) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.workspace_search.generation != generation {
                    return;
                }
                this.workspace_search.cancel = None;
                match result {
                    Ok(report) => {
                        this.status = format!(
                            "{} matches{}{}",
                            report.total,
                            if report.truncated {
                                " (results truncated)"
                            } else {
                                ""
                            },
                            if report.warnings.is_empty() {
                                String::new()
                            } else {
                                format!(" — {}", report.warnings.join("; "))
                            }
                        );
                        this.workspace_search.report = Some(report);
                    }
                    Err(error) => this.status = format!("Search failed: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_search_result(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.workspace_search.root != self.explorer_tree.root {
            return;
        }
        let Some(result) = self
            .workspace_search
            .report
            .as_ref()
            .and_then(|r| r.matches.get(index))
        else {
            return;
        };
        let path = result.path.clone();
        let range = result.range.clone();
        let digest = result.digest;
        if self
            .path
            .as_ref()
            .is_some_and(|open| same_path(open, &path))
        {
            if <[u8; 32]>::from(Sha256::digest(self.editor.read(cx).text().as_bytes())) != digest {
                self.status = "File changed since search. Search again.".into();
            } else {
                self.show_editor(window, cx);
                self.editor
                    .update(cx, |editor, cx| editor.select_range(range, cx));
            }
            cx.notify();
            return;
        }
        if let Some(index) = self
            .documents
            .iter()
            .position(|d| d.path.as_ref().is_some_and(|open| same_path(open, &path)))
        {
            if <[u8; 32]>::from(Sha256::digest(
                self.documents[index].editor.read(cx).text().as_bytes(),
            )) != digest
            {
                self.status = "File changed since search. Search again.".into();
                cx.notify();
                return;
            }
            self.activate_document(index, window, cx);
            self.editor
                .update(cx, |editor, cx| editor.select_range(range, cx));
            return;
        }
        let generation = self.workspace_search.generation;
        self.busy = true;
        let work = cx
            .background_executor()
            .spawn(async move { persistence::load(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                if this.workspace_search.generation != generation
                    || this.workspace_search.root != this.explorer_tree.root
                {
                    return;
                }
                match result {
                    Ok(loaded) => {
                        let path = loaded.path.clone();
                        let text = loaded.buffer.text_snapshot();
                        let mut bytes = Vec::new();
                        if text.write_to(&mut bytes).is_err()
                            || <[u8; 32]>::from(Sha256::digest(&bytes)) != digest
                        {
                            this.status = "File changed since search. Search again.".into();
                            cx.notify();
                            return;
                        }
                        this.open_document(loaded, None, window, cx);
                        if this.path.as_ref() == Some(&path) {
                            this.editor
                                .update(cx, |editor, cx| editor.select_range(range, cx));
                        }
                    }
                    Err(error) => this.status = format!("Open failed: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn workspace_search_view(&self, cx: &Context<Self>) -> Div {
        let toggle = |label: &'static str, active: bool| {
            div()
                .px_2()
                .py_1()
                .cursor_pointer()
                .bg(rgb(if active { 0x30436a } else { 0x202630 }))
                .child(label)
        };
        let input = |label: &'static str, field: Entity<EditorView>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(label)
                .child(div().h(px(26.0)).bg(rgb(0x202630)).child(field))
        };
        let rows = self
            .workspace_search
            .report
            .as_ref()
            .map(|report| {
                report
                    .matches
                    .iter()
                    .enumerate()
                    .skip(self.workspace_search.page * 100)
                    .take(100)
                    .map(|(index, result)| {
                        let path = result
                            .path
                            .strip_prefix(
                                self.workspace_search
                                    .root
                                    .as_deref()
                                    .unwrap_or(result.path.as_path()),
                            )
                            .unwrap_or(&result.path);
                        div()
                            .id(("search-result", index))
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0x30436a)))
                            .child(format!(
                                "{}:{}:{}",
                                path.display(),
                                result.line + 1,
                                result.column + 1
                            ))
                            .child(
                                div()
                                    .truncate()
                                    .text_color(rgb(0x8590a3))
                                    .child(result.preview.clone()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_search_result(index, window, cx)
                            }))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        div()
            .w(px(330.0))
            .max_w(gpui::relative(0.5))
            .min_w_0()
            .flex_shrink()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .bg(rgb(0x15191f))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child("Workspace Search")
                    .child(
                        div()
                            .id("close-search")
                            .cursor_pointer()
                            .child("×")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_workspace_search();
                                this.workspace_search.visible = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(input("Search", self.workspace_search.query.clone()))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        toggle("Case", self.workspace_search.case_sensitive)
                            .id("case")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace_search.case_sensitive =
                                    !this.workspace_search.case_sensitive;
                                this.cancel_workspace_search();
                                this.workspace_search.report = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        toggle("Word", self.workspace_search.whole_word)
                            .id("word")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace_search.whole_word =
                                    !this.workspace_search.whole_word;
                                this.cancel_workspace_search();
                                this.workspace_search.report = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        toggle("Regex", self.workspace_search.regex)
                            .id("regex")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace_search.regex = !this.workspace_search.regex;
                                this.cancel_workspace_search();
                                this.workspace_search.report = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(input(
                "Include globs",
                self.workspace_search.include.clone(),
            ))
            .child(input(
                "Exclude globs",
                self.workspace_search.exclude.clone(),
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        toggle("Search", false)
                            .id("run-search")
                            .on_click(cx.listener(|this, _, _, cx| this.run_workspace_search(cx))),
                    )
                    .child(
                        toggle("Cancel", false)
                            .id("cancel-search")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_workspace_search();
                                this.status = "Search cancelled.".into();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        toggle("Previous", false)
                            .id("previous-search-page")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace_search.page =
                                    this.workspace_search.page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(
                        toggle("Next", false)
                            .id("next-search-page")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.workspace_search.report.as_ref().is_some_and(|report| {
                                    (this.workspace_search.page + 1) * 100 < report.matches.len()
                                }) {
                                    this.workspace_search.page += 1;
                                }
                                cx.notify();
                            })),
                    )
                    .child(format!("Page {}", self.workspace_search.page + 1)),
            )
            .child(div().text_size(px(11.0)).child(self.status.clone()))
            .child(
                div()
                    .id("workspace-results")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .text_size(px(12.0))
                    .children(rows),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn search_selects_unicode_in_dirty_tab_and_rejects_stale_disk(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        let third = dir.path().join("third.txt");
        std::fs::write(&first, "λ old").unwrap();
        std::fs::write(&second, "λ needle").unwrap();
        std::fs::write(&third, "λ needle").unwrap();
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.select_folder(dir.path().to_path_buf(), window, cx);
                shell.open_document(persistence::load(&first).unwrap(), None, window, cx);
            })
        });
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("λ needle dirty");
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.open_document(persistence::load(&second).unwrap(), None, window, cx);
                shell.show_workspace_search(window, cx);
            })
        });
        cx.simulate_input("needle");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                let report = shell.workspace_search.report.as_ref().expect(&shell.status);
                assert_eq!(report.total, 3);
                let index = report.matches.iter().position(|m| m.path == first).unwrap();
                shell.open_search_result(index, window, cx);
                assert_eq!(shell.path.as_ref(), Some(&first));
                assert_eq!(shell.editor.read(cx).selection().range(), 2..8);
                assert!(shell.editor.read(cx).is_dirty());
                let index = shell
                    .workspace_search
                    .report
                    .as_ref()
                    .unwrap()
                    .matches
                    .iter()
                    .position(|m| m.path == second)
                    .unwrap();
                // Once opened, a document's retained snapshot is authoritative.
                shell.open_search_result(index, window, cx);
                assert_eq!(shell.editor.read(cx).selection().range(), 2..8);
            })
        });
        std::fs::write(&third, "changed since search").unwrap();
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                let index = shell
                    .workspace_search
                    .report
                    .as_ref()
                    .unwrap()
                    .matches
                    .iter()
                    .position(|m| m.path == third)
                    .unwrap();
                shell.open_search_result(index, window, cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert!(shell.read(cx).status.contains("changed since search")));
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.show_workspace_search(window, cx);
                shell.run_workspace_search(cx);
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                let index = shell
                    .workspace_search
                    .report
                    .as_ref()
                    .unwrap()
                    .matches
                    .iter()
                    .position(|m| m.path == first)
                    .unwrap();
                shell
                    .documents
                    .iter()
                    .find(|d| d.path.as_ref() == Some(&first))
                    .unwrap()
                    .editor
                    .update(cx, |editor, cx| {
                        editor.edit_command(ale_editor_view::EditCommand::Undo, cx)
                    });
                shell.open_search_result(index, window, cx);
                assert!(shell.status.contains("changed since search"));
            })
        });
    }
}
