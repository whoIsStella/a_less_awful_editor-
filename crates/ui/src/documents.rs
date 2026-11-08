//! Open text buffers retain their own editor, history, disk version and recovered-C target.
use super::*;

const MAX_DOCUMENTS: usize = 32;
pub(super) struct Document {
    pub(super) editor: Entity<EditorView>,
    pub(super) path: Option<PathBuf>,
    pub(super) disk: Option<DiskVersion>,
    pub(super) replacement_target: Option<ReplacementTarget>,
}
impl AppShell {
    pub(super) fn exchange_document(&mut self, document: Document) -> Document {
        Document {
            editor: std::mem::replace(&mut self.editor, document.editor),
            path: std::mem::replace(&mut self.path, document.path),
            disk: std::mem::replace(&mut self.disk, document.disk),
            replacement_target: std::mem::replace(
                &mut self.replacement_target,
                document.replacement_target,
            ),
        }
    }
    pub(super) fn fresh_document(&mut self, cx: &mut Context<Self>) -> bool {
        if self.busy {
            return false;
        }
        let reusable = self.path.is_none()
            && self.replacement_target.is_none()
            && !self.editor.read(cx).is_dirty()
            && self.editor.read(cx).text().is_empty();
        if reusable {
            self.dismiss_transient(cx);
            return true;
        }
        if self.documents.len() + 1 >= MAX_DOCUMENTS {
            self.status = format!(
                "At most {MAX_DOCUMENTS} text documents can be open. Close a document first."
            );
            cx.notify();
            return false;
        }
        let editor = cx.new(|cx| EditorView::new("", cx));
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        self.document_order.push(editor.entity_id());
        let old = self.exchange_document(Document {
            editor,
            path: None,
            disk: None,
            replacement_target: None,
        });
        self.documents.push(old);
        if let Some(w) = &self.workbench {
            w.update(cx, |w, _| w.invalidate_compilation());
        }
        self.close_text_revision = None;
        self.dismiss_transient(cx);
        true
    }
    pub(super) fn activate_document(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy || index >= self.documents.len() {
            return;
        }
        let next = self.documents.remove(index);
        let old = self.exchange_document(next);
        self.documents.insert(index, old);
        self.close_text_revision = self
            .quit_approvals
            .iter()
            .find(|(id, _)| *id == self.editor.entity_id())
            .map(|(_, revision)| *revision);
        if let Some(w) = &self.workbench {
            w.update(cx, |w, _| w.invalidate_compilation());
        }
        self.dismiss_transient(cx);
        self.show_editor(window, cx);
    }
    pub(super) fn open_document(
        &mut self,
        loaded: Loaded,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        if self.path.as_ref() == Some(&loaded.path) {
            if let Some(line) = line {
                self.editor.update(cx, |e, cx| e.go_to_line(line, cx));
            }
            self.show_editor(window, cx);
            return;
        }
        if let Some(index) = self
            .documents
            .iter()
            .position(|d| d.path.as_ref() == Some(&loaded.path))
        {
            self.activate_document(index, window, cx);
            if let Some(line) = line {
                self.editor.update(cx, |e, cx| e.go_to_line(line, cx));
            }
            return;
        }
        if self.fresh_document(cx) {
            let intent = match line {
                Some(line) => Intent::Source(Box::new(loaded), line),
                None => Intent::Open(Box::new(loaded)),
            };
            self.apply_intent(intent, window, cx);
        }
    }
    pub(super) fn open_recovered_document(
        &mut self,
        text: String,
        target: ReplacementTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.fresh_document(cx) {
            self.apply_intent(Intent::RecoveredC(text, target), window, cx);
        }
    }
    pub(super) fn close_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.systems_active {
            self.request_binary_replace(BinaryIntent::CloseDocument, window, cx);
        } else {
            self.request_replace(Intent::CloseDocument, window, cx);
        }
    }
    pub(super) fn discard_active_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.documents.pop().unwrap_or_else(|| {
            let editor = cx.new(|cx| EditorView::new("", cx));
            cx.observe(&editor, |_, _, cx| cx.notify()).detach();
            Document {
                editor,
                path: None,
                disk: None,
                replacement_target: None,
            }
        });
        let old = self.exchange_document(next);
        self.document_order
            .retain(|id| *id != old.editor.entity_id());
        if !self.document_order.contains(&self.editor.entity_id()) {
            self.document_order.push(self.editor.entity_id());
        }
        self.quit_approvals
            .retain(|(id, _)| *id != old.editor.entity_id());
        self.close_text_revision = None;
        if let Some(w) = &self.workbench {
            w.update(cx, |w, _| w.invalidate_compilation());
        }
        self.dismiss_transient(cx);
        self.systems_active = false;
        self.finish("Document closed", window, cx);
    }
    pub(super) fn continue_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let revision = self.editor.read(cx).content_revision();
        self.close_text_revision = Some(revision);
        self.quit_approvals
            .retain(|(id, _)| *id != self.editor.entity_id());
        self.quit_approvals
            .push((self.editor.entity_id(), revision));
        let pending = self.documents.iter().position(|d| {
            d.editor.read(cx).is_dirty()
                && !self
                    .quit_approvals
                    .contains(&(d.editor.entity_id(), d.editor.read(cx).content_revision()))
        });
        if let Some(index) = pending {
            self.activate_document(index, window, cx);
            self.request_replace(Intent::Close, window, cx);
        } else {
            self.request_binary_replace(BinaryIntent::Close, window, cx);
        }
    }
    pub(super) fn next_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let position = self
            .document_order
            .iter()
            .position(|id| *id == self.editor.entity_id())
            .unwrap_or(0);
        let next = self.document_order[(position + 1) % self.document_order.len()];
        if let Some(index) = self
            .documents
            .iter()
            .position(|d| d.editor.entity_id() == next)
        {
            self.activate_document(index, window, cx);
        } else {
            self.show_editor(window, cx);
        }
    }
    pub(super) fn document_tabs(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        self.document_order
            .iter()
            .enumerate()
            .map(|(tab, id)| {
                let active = *id == self.editor.entity_id();
                let (editor, path, target) = if active {
                    (&self.editor, &self.path, &self.replacement_target)
                } else {
                    let d = self
                        .documents
                        .iter()
                        .find(|d| d.editor.entity_id() == *id)
                        .expect("open document");
                    (&d.editor, &d.path, &d.replacement_target)
                };
                let name = path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .or_else(|| target.as_ref().map(|t| format!("{}.c", t.site.name)))
                    .unwrap_or_else(|| "Untitled".into());
                let dirty = editor.read(cx).is_dirty();
                let id = *id;
                div()
                    .id(("open-document", tab))
                    .h_full()
                    .max_w(px(220.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .text_color(rgb(0x8590a3))
                    .when(active && !self.systems_active, |v| {
                        v.bg(rgb(0x1b2028))
                            .text_color(rgb(0xd4dce7))
                            .border_b_1()
                            .border_color(rgb(0x7aa2f7))
                    })
                    .child(format!("{name}{}", if dirty { " •" } else { "" }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(index) = this
                            .documents
                            .iter()
                            .position(|d| d.editor.entity_id() == id)
                        {
                            this.activate_document(index, window, cx);
                        } else {
                            this.show_editor(window, cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    #[gpui::test]
    fn tabs_preserve_unsaved_text_history_and_stable_order(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "alpha").unwrap();
        std::fs::write(&b, "beta").unwrap();
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("scratch");
        let scratch = cx.update(|_, cx| shell.read(cx).editor.clone());
        for path in [&a, &b] {
            cx.update(|window, cx| {
                shell.update(cx, |s, cx| {
                    s.open_document(persistence::load(path).unwrap(), None, window, cx)
                })
            });
            cx.simulate_input("edited ");
        }
        let order = cx.update(|_, cx| shell.read(cx).document_order.clone());
        for _ in 0..3 {
            cx.update(|window, cx| shell.update(cx, |s, cx| s.next_document(window, cx)));
        }
        cx.update(|_, cx| {
            let s = shell.read(cx);
            assert_eq!(s.document_order, order);
            assert_eq!(s.path.as_ref(), Some(&b));
            assert_eq!(scratch.read(cx).text(), "scratch");
            assert!(scratch.read(cx).is_dirty());
        });
        cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                s.open_document(persistence::load(&a).unwrap(), None, window, cx)
            })
        });
        assert!(!cx.has_pending_prompt());
        cx.update(|_, cx| {
            assert_eq!(shell.read(cx).documents.len(), 2);
            assert_eq!(shell.read(cx).editor.read(cx).text(), "edited alpha");
        });
        cx.simulate_keystrokes("ctrl-z ctrl-z ctrl-z ctrl-z ctrl-z ctrl-z ctrl-z");
        cx.update(|_, cx| assert_eq!(shell.read(cx).editor.read(cx).text(), "alpha"));
        assert_eq!(std::fs::read_to_string(a).unwrap(), "alpha");
        assert_eq!(std::fs::read_to_string(b).unwrap(), "beta");
    }
    #[gpui::test]
    fn quit_cancel_retains_previously_discard_approved_buffers(cx: &mut TestAppContext) {
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("first");
        let first = cx.update(|_, cx| shell.read(cx).editor.clone());
        cx.dispatch_action(NewDocument);
        cx.simulate_input("second");
        let second = cx.update(|_, cx| shell.read(cx).editor.clone());
        cx.dispatch_action(Quit);
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(first.read(cx).text(), "first");
            assert!(first.read(cx).is_dirty());
            assert_eq!(second.read(cx).text(), "second");
            assert!(second.read(cx).is_dirty());
            assert!(shell.read(cx).quit_approvals.is_empty());
        });
    }
    #[gpui::test]
    fn busy_save_cannot_switch_snapshot_owner_and_close_only_closes_one_tab(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("first");
        cx.dispatch_action(NewDocument);
        cx.simulate_input("second");
        let second = cx.update(|_, cx| shell.read(cx).editor.entity_id());
        cx.dispatch_action(SaveAs);
        cx.dispatch_action(NextDocument);
        cx.update(|_, cx| assert_eq!(shell.read(cx).editor.entity_id(), second));
        cx.simulate_new_path_selection(|_| None);
        cx.run_until_parked();
        cx.dispatch_action(Close);
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let s = shell.read(cx);
            assert_eq!(s.editor.read(cx).text(), "first");
            assert!(s.documents.is_empty());
        });
    }
    #[gpui::test]
    fn save_as_cannot_overwrite_another_open_document(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.txt");
        std::fs::write(&path, "original").unwrap();
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                s.open_document(persistence::load(&path).unwrap(), None, window, cx)
            })
        });
        cx.simulate_input("unsaved ");
        let first = cx.update(|_, cx| shell.read(cx).editor.clone());
        cx.dispatch_action(NewDocument);
        cx.simulate_input("collision");
        cx.dispatch_action(SaveAs);
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        cx.run_until_parked();
        assert!(!cx.has_pending_prompt());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
        cx.update(|_, cx| {
            let s = shell.read(cx);
            assert!(!s.busy);
            assert!(s.editor.read(cx).is_dirty());
            assert_eq!(s.editor.read(cx).text(), "collision");
            assert_eq!(first.read(cx).text(), "unsaved original");
        });
    }
}
