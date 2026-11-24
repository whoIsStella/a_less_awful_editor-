use super::*;
use crate::session_store::{Session, SessionStore, SessionTab};
use std::{collections::HashMap, sync::mpsc};

type Checkpoint = (Session, mpsc::SyncSender<Result<(), String>>);

pub(super) struct SessionState {
    sender: Option<mpsc::Sender<Checkpoint>>,
    pub(super) enabled: bool,
    root: Option<PathBuf>,
    generation: u64,
    checkpoint_timer: Option<u64>,
    checkpoint_writer: Option<u64>,
    checkpoint_pending: bool,
    loading: bool,
    baseline: HashMap<PathBuf, String>,
    conflicts: Vec<Conflict>,
    review: Option<usize>,
    pub(super) active_pane: String,
    pub(super) terminal_focus: gpui::FocusHandle,
    focus_watched: std::collections::HashSet<gpui::EntityId>,
}

struct Conflict {
    path: PathBuf,
    base: String,
    draft: String,
    disk: String,
}

impl SessionState {
    pub fn new(cx: &mut Context<AppShell>) -> Self {
        Self {
            enabled: !cfg!(test),
            sender: None,
            root: None,
            generation: 0,
            checkpoint_timer: None,
            checkpoint_writer: None,
            checkpoint_pending: false,
            loading: false,
            baseline: HashMap::new(),
            conflicts: Vec::new(),
            review: None,
            active_pane: "editor".into(),
            terminal_focus: cx.focus_handle(),
            focus_watched: std::collections::HashSet::new(),
        }
    }
}

impl AppShell {
    pub(super) fn watch_editor_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.focus_watched.insert(self.editor.entity_id()) {
            return;
        }
        let focus = self.editor.read(cx).focus_handle(cx);
        cx.on_focus(&focus, window, |this, _, cx| {
            this.session.active_pane = "editor".into();
            this.schedule_session_checkpoint(cx);
        })
        .detach();
    }

    pub(super) fn begin_session(
        &mut self,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.session.enabled {
            return;
        }
        let canonical = root.canonicalize().ok();
        if canonical.is_some()
            && canonical == self.session.root
            && (self.session.loading || self.session.sender.is_some())
        {
            return;
        }
        let switched = self.session.root.is_some();
        self.checkpoint_session(cx);
        self.session.sender = None;
        self.session.root = Some(root.clone());
        self.session.generation += 1;
        self.session.loading = true;
        self.session.baseline.clear();
        self.session.conflicts.clear();
        self.session.review = None;
        let generation = self.session.generation;
        let work = cx.background_executor().spawn(async move {
            let store = SessionStore::open(&root)?;
            let saved = store.load()?;
            let canonical = root.canonicalize().map_err(|e| e.to_string())?;
            let disk = saved
                .as_ref()
                .map(|s| {
                    s.tabs
                        .iter()
                        .map(|tab| tab.path.as_ref().map(|path| persistence::load(path)))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok::<_, String>((store, canonical, saved, disk))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.session.generation != generation { return; }
                this.session.loading = false;
                match result {
                    Ok((store, canonical, saved, disk)) => {
                        let pristine = this.path.is_none() && this.documents.is_empty()
                            && !this.editor.read(cx).is_dirty() && this.editor.read(cx).text().is_empty()
                            && this.workbench.is_none();
                        if !pristine && (saved.is_some() || switched) {
                            this.status = "Open this folder in a new window to use its session. Current documents are retained.".into();
                            cx.notify();
                            return;
                        }
                        let (tx, rx) = mpsc::channel::<Checkpoint>();
                        std::thread::spawn(move || {
                            while let Ok((snapshot, reply)) = rx.recv() {
                                let _ = reply.send(store.save(&snapshot));
                            }
                        });
                        this.session.sender = Some(tx);
                        this.session.root = Some(canonical);
                        if let Some(saved) = saved { this.restore_session(saved, disk, window, cx); }
                        this.schedule_session_checkpoint(cx);
                    }
                    Err(error) => this.status = format!("Workspace session unavailable: {error}"),
                }
                cx.notify();
            });
        }).detach();
    }

    pub(super) fn schedule_session_checkpoint(&mut self, cx: &mut Context<Self>) {
        if self.session.loading || self.session.sender.is_none() {
            return;
        }
        let generation = self.session.generation;
        self.session.checkpoint_pending = true;
        if self.session.checkpoint_timer == Some(generation)
            || self.session.checkpoint_writer == Some(generation)
        {
            return;
        }
        self.session.checkpoint_timer = Some(generation);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.session.generation == generation {
                    this.session.checkpoint_timer = None;
                    this.checkpoint_session(cx);
                }
            });
        })
        .detach();
    }

    fn session_snapshot(&self, discard: bool, cx: &Context<Self>) -> Option<Session> {
        let root = self.session.root.as_ref()?;
        let mut tabs = Vec::new();
        let mut active = 0;
        for id in &self.document_order {
            let (editor, path, disk) = if *id == self.editor.entity_id() {
                active = tabs.len();
                (&self.editor, &self.path, &self.disk)
            } else if let Some(d) = self.documents.iter().find(|d| d.editor.entity_id() == *id) {
                (&d.editor, &d.path, &d.disk)
            } else {
                continue;
            };
            let view = editor.read(cx);
            let base = disk
                .as_ref()
                .and_then(|d| std::str::from_utf8(d.bytes()).ok())
                .map(str::to_owned)
                .or_else(|| {
                    path.as_ref()
                        .and_then(|p| self.session.baseline.get(p))
                        .cloned()
                });
            let selection = view.selection();
            let mut draft = view.text();
            let mut dirty = view.is_dirty();
            if discard
                && dirty
                && self
                    .quit_approvals
                    .contains(&(editor.entity_id(), view.content_revision()))
            {
                if let Some(original) = &base {
                    draft = original.clone();
                    dirty = false;
                } else {
                    continue;
                }
            }
            tabs.push(SessionTab {
                path: path.clone(),
                draft,
                base,
                anchor: selection.anchor,
                head: selection.head,
                dirty,
            });
        }
        let active = active.min(tabs.len().saturating_sub(1));
        Some(Session {
            workspace: root.clone(),
            tabs,
            active,
            explorer_visible: self.explorer_visible,
            terminal_visible: self.panels_visible && self.panel == Panel::Terminal,
            panel_visible: self.panels_visible,
            panel: if self.panel == Panel::Terminal {
                "terminal"
            } else {
                "output"
            }
            .into(),
            active_pane: self.session.active_pane.clone(),
        })
    }

    pub(super) fn checkpoint_session(&mut self, cx: &mut Context<Self>) {
        if self.session.sender.is_none() {
            return;
        }
        let Some(session) = self.session_snapshot(false, cx) else {
            return;
        };
        let Some(sender) = &self.session.sender else {
            return;
        };
        let (reply, response) = mpsc::sync_channel(1);
        if sender.send((session, reply)).is_err() {
            self.status = "Could not save workspace recovery session.".into();
            cx.notify();
            return;
        }
        let generation = self.session.generation;
        self.session.checkpoint_writer = Some(generation);
        self.session.checkpoint_pending = false;
        let work = cx
            .background_executor()
            .spawn(async move { response.recv() });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.session.generation != generation {
                    return;
                }
                this.session.checkpoint_writer = None;
                if this.session.checkpoint_pending {
                    this.schedule_session_checkpoint(cx);
                }
                if let Err(error) = result.unwrap_or_else(|_| Err("Recovery writer stopped".into()))
                {
                    this.status = format!("Workspace recovery save failed: {error}");
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn finish_session_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session_snapshot(true, cx) else {
            cx.quit();
            return;
        };
        let Some(sender) = &self.session.sender else {
            cx.quit();
            return;
        };
        self.session.generation = self.session.generation.wrapping_add(1);
        let (reply, response) = mpsc::sync_channel(1);
        if sender.send((session, reply)).is_err() {
            self.confirm_quit_without_recovery("Recovery writer stopped.".into(), window, cx);
            return;
        }
        self.busy = true;
        let revisions = std::iter::once(&self.editor)
            .chain(self.documents.iter().map(|doc| &doc.editor))
            .map(|editor| (editor.entity_id(), editor.read(cx).content_revision()))
            .collect::<Vec<_>>();
        let work = cx
            .background_executor()
            .spawn(async move { response.recv() });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(())) => {
                        let current = std::iter::once(&this.editor)
                            .chain(this.documents.iter().map(|doc| &doc.editor))
                            .map(|editor| (editor.entity_id(), editor.read(cx).content_revision()))
                            .collect::<Vec<_>>();
                        if current == revisions {
                            cx.quit();
                        } else {
                            this.status = "Documents changed while closing; quit cancelled.".into();
                            this.schedule_session_checkpoint(cx);
                            cx.notify();
                        }
                    }
                    other => this.confirm_quit_without_recovery(format!("{other:?}"), window, cx),
                }
            });
        })
        .detach();
    }

    fn confirm_quit_without_recovery(
        &mut self,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        let answer = window.prompt(PromptLevel::Warning, "Workspace session could not be saved",
            Some(&format!("{error}\nQuit anyway? The previous recovery session may still contain discarded drafts.")),
            &[PromptButton::Cancel("Cancel".into()), PromptButton::Other("Quit anyway".into())], cx);
        let revisions = std::iter::once(&self.editor)
            .chain(self.documents.iter().map(|doc| &doc.editor))
            .map(|editor| (editor.entity_id(), editor.read(cx).content_revision()))
            .collect::<Vec<_>>();
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.busy = false;
                let current = std::iter::once(&this.editor)
                    .chain(this.documents.iter().map(|doc| &doc.editor))
                    .map(|editor| (editor.entity_id(), editor.read(cx).content_revision()))
                    .collect::<Vec<_>>();
                if matches!(answer, Ok(1)) && current == revisions {
                    cx.quit();
                } else {
                    this.status = "Quit cancelled. Documents retained.".into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn restore_session(
        &mut self,
        saved: Session,
        disk: Vec<Option<Result<Loaded, String>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if saved.tabs.is_empty() {
            return;
        }
        self.document_order.clear();
        self.documents.clear();
        let mut restored = Vec::new();
        for (mut tab, loaded) in saved.tabs.into_iter().zip(disk) {
            if let (Some(path), Some(base)) = (&tab.path, &tab.base) {
                self.session.baseline.insert(path.clone(), base.clone());
            }
            let (current, load_error) = match loaded {
                Some(Ok(loaded)) => (Some(loaded), None),
                Some(Err(error)) => (None, Some(error)),
                None => (None, None),
            };
            let disk_text = current.as_ref().map(|v| {
                v.buffer
                    .text_in_range(0..v.buffer.len_chars())
                    .unwrap_or_default()
            });
            if !tab.dirty
                && let Some(text) = &disk_text
            {
                tab.draft = text.clone();
                tab.base = Some(text.clone());
                if let Some(path) = &tab.path {
                    self.session.baseline.insert(path.clone(), text.clone());
                }
            }
            let conflict = tab.path.is_some() && tab.base.as_deref() != disk_text.as_deref();
            if conflict {
                self.session.conflicts.push(Conflict {
                    path: tab.path.clone().unwrap(),
                    base: tab.base.clone().unwrap_or_default(),
                    draft: tab.draft.clone(),
                    disk: disk_text.clone().unwrap_or_else(|| {
                        format!(
                            "[File unavailable: {}]",
                            load_error.as_deref().unwrap_or("unknown error")
                        )
                    }),
                });
            }
            let version = current.and_then(|loaded| (!conflict).then_some(loaded.version));
            let dirty = tab.dirty || conflict;
            let editor = cx.new(|cx| EditorView::new("", cx));
            editor.update(cx, |e, cx| {
                e.load(
                    ale_editor_core::EditorBuffer::recovered(
                        &tab.draft, dirty, tab.anchor, tab.head,
                    ),
                    cx,
                )
            });
            cx.observe(&editor, |this, _, cx| {
                this.schedule_session_checkpoint(cx);
                cx.notify();
            })
            .detach();
            self.document_order.push(editor.entity_id());
            restored.push(documents::Document {
                editor,
                path: tab.path,
                disk: version,
                replacement_target: None,
            });
        }
        if restored.is_empty() {
            return;
        }
        let active = saved.active.min(restored.len() - 1);
        let document = restored.remove(active);
        self.editor = document.editor;
        self.path = document.path;
        self.disk = document.disk;
        self.replacement_target = None;
        self.documents = restored;
        self.watch_editor_focus(window, cx);
        self.explorer_visible = saved.explorer_visible;
        self.panels_visible = saved.panel_visible || saved.terminal_visible;
        self.panel = if saved.panel == "terminal" || saved.terminal_visible {
            Panel::Terminal
        } else {
            Panel::Output
        };
        self.show_editor(window, cx);
        if saved.active_pane == "terminal" && self.panels_visible && self.panel == Panel::Terminal {
            self.session.active_pane = "terminal".into();
            self.session.terminal_focus.focus(window);
        }
        self.status = if self.session.conflicts.is_empty() {
            "Workspace session restored.".into()
        } else {
            format!(
                "{} files changed on disk. Review recovered drafts.",
                self.session.conflicts.len()
            )
        };
    }

    pub(super) fn clear_recovery_conflict(&mut self, path: &std::path::Path) {
        self.session.conflicts.retain(|c| c.path != path);
        self.session.baseline.remove(path);
        self.session.review = None;
    }

    fn open_recovery_review(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(conflict) = self.session.conflicts.get(index) else {
            return;
        };
        let path = conflict.path.clone();
        let generation = self.session.generation;
        let work = cx.background_executor().spawn({
            let path = path.clone();
            async move { persistence::load(&path) }
        });
        cx.spawn(async move |this, cx| {
            let loaded = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.session.generation != generation {
                    return;
                }
                let draft = if this.path.as_ref() == Some(&path) {
                    Some(this.editor.read(cx).text())
                } else {
                    this.documents
                        .iter()
                        .find(|doc| doc.path.as_ref() == Some(&path))
                        .map(|doc| doc.editor.read(cx).text())
                };
                if let Some(conflict) = this
                    .session
                    .conflicts
                    .get_mut(index)
                    .filter(|item| item.path == path)
                {
                    conflict.disk = match loaded {
                        Ok(loaded) => loaded
                            .buffer
                            .text_in_range(0..loaded.buffer.len_chars())
                            .unwrap_or_default(),
                        Err(error) => format!("[File unavailable: {error}]"),
                    };
                    if let Some(draft) = draft {
                        conflict.draft = draft;
                    }
                    this.session.review = Some(index);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn recovery_view(&self, cx: &Context<Self>) -> Div {
        let Some(index) = self.session.review else {
            return div()
                .flex()
                .gap_2()
                .children(self.session.conflicts.iter().enumerate().map(|(i, c)| {
                    div()
                        .id(("recovery-review", i))
                        .cursor_pointer()
                        .px_2()
                        .child(format!("Review {}", c.path.display()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_recovery_review(i, cx);
                        }))
                }));
        };
        let c = &self.session.conflicts[index];
        div()
            .h(px(240.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(format!("Recovered conflict: {}", c.path.display()))
                    .child(
                        div()
                            .id("close-recovery-review")
                            .cursor_pointer()
                            .child("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.session.review = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div().flex().flex_1().min_h_0().children(
                    [
                        ("Original", &c.base),
                        ("Disk", &c.disk),
                        ("Recovered", &c.draft),
                    ]
                    .map(|(label, text)| {
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .child(label)
                            .child(
                                div()
                                    .id(label)
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_scroll()
                                    .whitespace_normal()
                                    .child(text.clone()),
                            )
                    }),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn restores_terminal_focus_without_starting_a_shell(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let saved = Session {
            workspace: dir.path().to_path_buf(),
            tabs: vec![SessionTab {
                path: None,
                draft: "draft".into(),
                base: None,
                anchor: 1,
                head: 3,
                dirty: true,
            }],
            active: 0,
            explorer_visible: true,
            terminal_visible: true,
            panel_visible: true,
            panel: "terminal".into(),
            active_pane: "terminal".into(),
        };
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.restore_session(saved, vec![None], window, cx);
                assert!(shell.panels_visible);
                assert!(shell.panel == Panel::Terminal);
                assert!(shell.terminal.is_none());
                assert!(shell.session.terminal_focus.is_focused(window));
                assert_eq!(shell.editor.read(cx).selection().anchor, 1);
                assert_eq!(shell.editor.read(cx).selection().head, 3);
            })
        });
    }

    #[gpui::test]
    fn restore_keeps_dirty_drafts_and_adopts_new_disk_for_clean_tabs(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        let clean = dir.path().join("clean");
        std::fs::write(&first, "external").unwrap();
        std::fs::write(&second, "original two").unwrap();
        std::fs::write(&clean, "new clean").unwrap();
        let tabs = vec![
            SessionTab {
                path: Some(first.clone()),
                draft: "my λ draft".into(),
                base: Some("original".into()),
                anchor: 3,
                head: 5,
                dirty: true,
            },
            SessionTab {
                path: Some(second.clone()),
                draft: "my other draft".into(),
                base: Some("original two".into()),
                anchor: 0,
                head: 2,
                dirty: true,
            },
            SessionTab {
                path: Some(clean.clone()),
                draft: "old clean".into(),
                base: Some("old clean".into()),
                anchor: 0,
                head: 0,
                dirty: false,
            },
        ];
        let saved = Session {
            workspace: dir.path().to_path_buf(),
            tabs,
            active: 0,
            explorer_visible: true,
            terminal_visible: false,
            panel_visible: true,
            panel: "output".into(),
            active_pane: "editor".into(),
        };
        let disks = [&first, &second, &clean]
            .iter()
            .map(|p| Some(persistence::load(p)))
            .collect();
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.session.root = Some(dir.path().to_path_buf());
                shell.restore_session(saved, disks, window, cx);
                assert_eq!(shell.editor.read(cx).text(), "my λ draft");
                assert!(shell.editor.read(cx).is_dirty());
                assert_eq!(shell.editor.read(cx).selection().anchor, 3);
                assert_eq!(shell.editor.read(cx).selection().head, 5);
                assert_eq!(shell.session.conflicts.len(), 1);
                assert_eq!(shell.session.conflicts[0].base, "original");
                assert_eq!(shell.session.conflicts[0].disk, "external");
                assert!(shell.disk.is_none());
                let other = shell
                    .documents
                    .iter()
                    .find(|d| d.path.as_ref() == Some(&second))
                    .unwrap();
                assert_eq!(other.editor.read(cx).text(), "my other draft");
                assert!(other.editor.read(cx).is_dirty());
                let clean_doc = shell
                    .documents
                    .iter()
                    .find(|d| d.path.as_ref() == Some(&clean))
                    .unwrap();
                assert_eq!(clean_doc.editor.read(cx).text(), "new clean");
                assert!(!clean_doc.editor.read(cx).is_dirty());
                assert!(shell.panels_visible);
                shell.quit_approvals.push((
                    shell.editor.entity_id(),
                    shell.editor.read(cx).content_revision(),
                ));
                let end = shell.session_snapshot(true, cx).unwrap();
                let discarded = end
                    .tabs
                    .iter()
                    .find(|d| d.path.as_ref() == Some(&first))
                    .unwrap();
                assert_eq!(discarded.draft, "original");
                assert!(!discarded.dirty);
            })
        });
    }
}
