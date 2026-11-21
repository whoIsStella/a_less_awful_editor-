//! Open several text files in the same window without replacing open buffers.
use super::*;

impl AppShell {
    pub fn open_text_targets(
        &mut self,
        paths: Vec<PathBuf>,
        line: usize,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            self.status = "Wait for the current file operation before opening files.".into();
            cx.notify();
            return;
        }
        let cwd = match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(error) => {
                self.status = format!("Cannot resolve paths: {error}");
                cx.notify();
                return;
            }
        };
        let paths: Vec<_> = paths
            .into_iter()
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                }
            })
            .collect();
        let already_open: std::collections::HashSet<_> = self
            .documents
            .iter()
            .filter_map(|doc| doc.path.clone())
            .chain(self.path.iter().cloned())
            .collect();
        self.busy = true;
        self.status = "Opening files…".into();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let mut failures = Vec::new();
            let mut opened = 0;
            for path in paths {
                let loaded = if already_open.contains(&path) {
                    None
                } else {
                    let load_path = path.clone();
                    Some(
                        cx.background_executor()
                            .spawn(async move { persistence::load(&load_path) })
                            .await,
                    )
                };
                let result = this.update_in(cx, |this, window, cx| {
                    this.busy = false;
                    let mut failed = false;
                    match loaded {
                        Some(Ok(loaded)) => this.open_document(loaded, None, window, cx),
                        Some(Err(error)) => {
                            failures.push(format!("{}: {error}", path.display()));
                            failed = true;
                        }
                        None if this.path.as_ref() == Some(&path) => this.show_editor(window, cx),
                        None => {
                            if let Some(index) = this
                                .documents
                                .iter()
                                .position(|d| d.path.as_ref() == Some(&path))
                            {
                                this.activate_document(index, window, cx);
                            } else {
                                failures.push(format!("{}: tab is no longer open", path.display()));
                                failed = true;
                            }
                        }
                    }
                    if !failed && this.path.as_ref() == Some(&path) {
                        this.editor.update(cx, |editor, cx| {
                            editor.go_to_location(
                                line.saturating_sub(1),
                                column.saturating_sub(1),
                                cx,
                            )
                        });
                        opened += 1;
                    } else if !failed {
                        failures.push(format!("{}: unable to open another tab", path.display()));
                    }
                    this.busy = true;
                });
                if result.is_err() {
                    return;
                }
            }
            let _ = this.update_in(cx, |this, _, cx| {
                this.busy = false;
                this.status = if failures.is_empty() {
                    format!("Opened {opened} files.")
                } else {
                    format!("Opened {opened} files. {}", failures.join("; "))
                };
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn multiple_files_keep_dirty_tab_and_report_missing_paths(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("one.c");
        let second = dir.path().join("two λ.c");
        let missing = dir.path().join("missing.c");
        std::fs::write(&first, "one\nλx\n").unwrap();
        std::fs::write(&second, "two\nλxyz\n").unwrap();
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let mut shell = AppShell::new(window, cx);
            shell.open_target(OpenTarget::Text(first.clone()), window, cx);
            shell
        });
        cx.run_until_parked();
        cx.simulate_input("unsaved ");
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.open_text_targets(
                    vec![first.clone(), missing.clone(), second.clone()],
                    2,
                    3,
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let shell = shell.read(cx);
            assert_eq!(shell.path.as_ref(), Some(&second));
            assert_eq!(
                shell.editor.read(cx).cursor(),
                ale_editor_core::Position { line: 1, column: 2 }
            );
            let retained = shell
                .documents
                .iter()
                .find(|doc| doc.path.as_ref() == Some(&first))
                .unwrap();
            assert!(retained.editor.read(cx).is_dirty());
            assert!(retained.editor.read(cx).text().starts_with("unsaved "));
            assert!(shell.status.contains("missing.c"));
        });
    }
}

#[cfg(unix)]
impl AppShell {
    pub fn accept_open_requests(
        &mut self,
        receiver: std::sync::mpsc::Receiver<(Vec<PathBuf>, usize, usize)>,
        guard: Box<dyn std::any::Any>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let mut pending = std::collections::VecDeque::new();
            let guard = guard;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let _ = &guard;
                while pending.len() < 32 {
                    match receiver.try_recv() {
                        Ok(request) => pending.push_back(request),
                        Err(_) => break,
                    }
                }
                let Ok(()) = this.update_in(cx, |this, window, cx| {
                    if !this.busy
                        && let Some((paths, line, column)) = pending.pop_front()
                    {
                        this.open_text_targets(paths, line, column, window, cx);
                    }
                }) else {
                    break;
                };
            }
        })
        .detach();
    }
}
