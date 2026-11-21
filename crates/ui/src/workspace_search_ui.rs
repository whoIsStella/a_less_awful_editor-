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
    pub(super) replacement: Entity<EditorView>,
    selected: Vec<bool>,
    preview: Vec<PreviewFile>,
    undo: Vec<UndoFile>,
    outcomes: Vec<String>,
    current_file: bool,
    active_entity: Option<gpui::EntityId>,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    generation: u64,
    page: usize,
    preview_page: usize,
    cancel: Option<Arc<AtomicBool>>,
    pub report: Option<SearchReport>,
    pub(super) input_revisions: [u64; 4],
    root: Option<PathBuf>,
}

#[derive(Clone)]
struct PreviewFile {
    plan: workspace_replace::FilePlan,
    snippets: Vec<String>,
    version: Option<DiskVersion>,
}

struct UndoFile {
    editor: Entity<EditorView>,
    revision: u64,
    path: PathBuf,
}

fn snippets(plan: &workspace_replace::FilePlan) -> Vec<String> {
    let mut chars = plan.before.chars();
    let mut position = 0;
    plan.edits
        .iter()
        .map(|edit| {
            let skip = edit.range.start.saturating_sub(position);
            let _ = chars.by_ref().take(skip).count();
            let length = edit.range.end - edit.range.start;
            let old = chars.by_ref().take(length).collect::<String>();
            position = edit.range.end;
            format!("- {old}\n+ {}", edit.replacement)
        })
        .collect()
}

fn preview_bytes(plan: &workspace_replace::FilePlan) -> Option<usize> {
    let edit_bytes = plan
        .edits
        .iter()
        .try_fold(0usize, |sum, edit| sum.checked_add(edit.replacement.len()))?;
    // A snippet duplicates the selected old bytes and replacement bytes.
    plan.before
        .len()
        .checked_mul(2)?
        .checked_add(plan.after.len())?
        .checked_add(edit_bytes.checked_mul(2)?)?
        .checked_add(plan.edits.len().checked_mul(5)?)
}

fn is_untitled(active: bool, path: Option<&std::path::Path>) -> bool {
    active && path.is_none()
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
    pub(super) fn clear_preview(&mut self) {
        self.preview.clear();
    }
    pub fn new(cx: &mut Context<AppShell>) -> Self {
        Self {
            visible: false,
            query: cx.new(|cx| EditorView::input("", cx)),
            include: cx.new(|cx| EditorView::input("", cx)),
            exclude: cx.new(|cx| EditorView::input("", cx)),
            replacement: cx.new(|cx| EditorView::input("", cx)),
            selected: Vec::new(),
            preview: Vec::new(),
            undo: Vec::new(),
            outcomes: Vec::new(),
            current_file: false,
            active_entity: None,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            generation: 0,
            page: 0,
            preview_page: 0,
            cancel: None,
            report: None,
            input_revisions: [0; 4],
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
        self.workspace_search.preview.clear();
    }

    pub(super) fn show_workspace_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.quick_open.close();
        self.workspace_search.visible = true;
        self.workspace_search
            .query
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.notify();
    }

    pub(super) fn run_workspace_search(&mut self, cx: &mut Context<Self>) {
        let root = self.explorer_tree.root.clone();
        if !self.workspace_search.current_file && root.is_none() {
            self.status = "Open a folder to search the workspace.".into();
            cx.notify();
            return;
        }
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
        self.workspace_search.root = root.clone();
        let active_entity = self
            .workspace_search
            .current_file
            .then_some(self.editor.entity_id());
        self.workspace_search.active_entity = active_entity;
        let active_text = if active_entity.is_some() {
            self.editor.read(cx).text()
        } else {
            String::new()
        };
        let active_path = self
            .path
            .clone()
            .unwrap_or_else(|| PathBuf::from("/__untitled_workspace_buffer__"));
        self.workspace_search.report = None;
        self.workspace_search.page = 0;
        let generation = self.workspace_search.generation;
        self.status = "Searching workspace…".into();
        let work = cx.background_executor().spawn(async move {
            if active_entity.is_some() {
                workspace_search::search_buffer(active_path, active_text, options, &cancel)
            } else {
                workspace_search::search(
                    root.as_ref().expect("workspace root"),
                    overlays,
                    options,
                    &cancel,
                )
            }
        });
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
                        this.workspace_search.selected = vec![true; report.matches.len()];
                        this.workspace_search.preview.clear();
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

    fn preview_replacements(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.workspace_search.root != self.explorer_tree.root {
            return;
        }
        let Some(report) = self.workspace_search.report.as_ref() else {
            return;
        };
        if report.truncated {
            self.status = "Refine the search before replacing: results are incomplete.".into();
            cx.notify();
            return;
        }
        let active_entity = self.workspace_search.active_entity;
        let active_text = if active_entity.is_some() {
            self.editor.read(cx).text()
        } else {
            String::new()
        };
        let active_untitled = is_untitled(active_entity.is_some(), self.path.as_deref());
        let active_path = self
            .path
            .clone()
            .unwrap_or_else(|| PathBuf::from("/__untitled_workspace_buffer__"));
        if active_entity.is_some_and(|id| id != self.editor.entity_id()) {
            self.status = "Active document changed. Search again.".into();
            cx.notify();
            return;
        }
        let mut grouped: std::collections::BTreeMap<PathBuf, Vec<workspace_search::SearchMatch>> =
            std::collections::BTreeMap::new();
        for (index, item) in report.matches.iter().enumerate() {
            if self.workspace_search.selected.get(index) == Some(&true) {
                grouped
                    .entry(item.path.clone())
                    .or_default()
                    .push(item.clone());
            }
        }
        if grouped.is_empty() {
            self.status = "Select matches to replace.".into();
            cx.notify();
            return;
        }
        let mut buffers = Vec::new();
        let mut buffer_bytes = 0usize;
        for (path, editor, version) in
            std::iter::once((self.path.as_ref(), &self.editor, self.disk.as_ref())).chain(
                self.documents
                    .iter()
                    .map(|d| (d.path.as_ref(), &d.editor, d.disk.as_ref())),
            )
        {
            if let (Some(path), Some(version)) = (path, version) {
                if !grouped.keys().any(|target| same_path(path, target)) {
                    continue;
                }
                let text = editor.read(cx).text();
                buffer_bytes += text.len();
                if buffer_bytes > 32 * 1024 * 1024 {
                    self.status =
                        "Selection exceeds the 32 MiB preview limit. Refine the search.".into();
                    cx.notify();
                    return;
                }
                buffers.push((path.clone(), text, version.clone()));
            }
        }
        let options = SearchOptions {
            query: self.workspace_search.query.read(cx).text(),
            include: self.workspace_search.include.read(cx).text(),
            exclude: self.workspace_search.exclude.read(cx).text(),
            case_sensitive: self.workspace_search.case_sensitive,
            whole_word: self.workspace_search.whole_word,
            regex: self.workspace_search.regex,
        };
        let replacement = self.workspace_search.replacement.read(cx).text();
        let generation = self.workspace_search.generation;
        self.busy = true;
        self.status = "Preparing replacement preview…".into();
        let work = cx.background_executor().spawn(async move {
            let mut plans = Vec::new();
            let mut errors = Vec::new();
            let mut total_bytes = 0usize;
            let mut retained_bytes = 0usize;
            for (path, matches) in grouped {
                let result = (|| {
                    if active_untitled && path == active_path {
                        let plan = workspace_replace::plan(
                            &active_text,
                            &matches,
                            &options,
                            &replacement,
                        )?;
                        total_bytes += active_text.len();
                        if total_bytes > 32 * 1024 * 1024 {
                            return Err("Preview limit reached (32 MiB).".into());
                        }
                        let plan_bytes = preview_bytes(&plan).ok_or("Preview size overflow.")?;
                        let candidate = retained_bytes
                            .checked_add(plan_bytes)
                            .ok_or("Preview size overflow.")?;
                        if candidate > 32 * 1024 * 1024 {
                            return Err("Combined preview limit reached (32 MiB).".into());
                        }
                        retained_bytes = candidate;
                        let snippets = snippets(&plan);
                        return Ok(PreviewFile {
                            plan,
                            snippets,
                            version: None,
                        });
                    }
                    let loaded = persistence::load(&path)?;
                    let buffer = buffers.iter().find(|(p, _, _)| same_path(p, &path));
                    if buffer.is_some_and(|(_, _, version)| *version != loaded.version) {
                        return Err("The file changed on disk.".to_string());
                    }
                    let text = match buffer {
                        Some((_, text, _)) => text.clone(),
                        None => loaded
                            .buffer
                            .text_in_range(0..loaded.buffer.len_chars())
                            .unwrap_or_default(),
                    };
                    total_bytes += text.len();
                    if total_bytes > 32 * 1024 * 1024 {
                        return Err("Preview limit reached (32 MiB).".into());
                    }
                    let plan = workspace_replace::plan(&text, &matches, &options, &replacement)?;
                    let plan_bytes = preview_bytes(&plan).ok_or("Preview size overflow.")?;
                    let candidate = retained_bytes
                        .checked_add(plan_bytes)
                        .ok_or("Preview size overflow.")?;
                    if candidate > 32 * 1024 * 1024 {
                        return Err("Combined preview limit reached (32 MiB).".into());
                    }
                    retained_bytes = candidate;
                    let snippets = snippets(&plan);
                    Ok(PreviewFile {
                        plan,
                        snippets,
                        version: Some(loaded.version),
                    })
                })();
                match result {
                    Ok(plan) => plans.push(plan),
                    Err(error) => errors.push(format!("{}: {error}", path.display())),
                }
            }
            (plans, errors)
        });
        cx.spawn(async move |this, cx| {
            let (plans, errors) = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if this.workspace_search.generation != generation {
                    return;
                }
                this.workspace_search.preview_page = 0;
                this.workspace_search.preview = plans;
                this.status = if errors.is_empty() {
                    format!(
                        "Previewing {} files. Apply creates unsaved edits.",
                        this.workspace_search.preview.len()
                    )
                } else {
                    format!(
                        "Preview has {} files. Conflicts: {}",
                        this.workspace_search.preview.len(),
                        errors.join("; ")
                    )
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_replacements(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.workspace_search.preview.is_empty() {
            return;
        }
        let plans = std::mem::take(&mut self.workspace_search.preview);
        let generation = self.workspace_search.generation;
        let active_entity = self.workspace_search.active_entity;
        self.busy = true;
        self.status = "Checking files before applying…".into();
        let work = cx.background_executor().spawn(async move {
            plans
                .into_iter()
                .map(|p| {
                    let current = p.version.as_ref().map(|_| persistence::load(&p.plan.path));
                    (p, current)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let checked = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if this.workspace_search.generation != generation {
                    return;
                }
                let mut applied = Vec::new();
                let mut failures = Vec::new();
                this.workspace_search.undo.clear();
                for (preview, loaded) in checked {
                    let path = preview.plan.path.clone();
                    let loaded = match (preview.version.as_ref(), loaded) {
                        (None, None) if active_entity == Some(this.editor.entity_id()) => None,
                        (Some(version), Some(Ok(loaded))) if loaded.version == *version => {
                            Some(loaded)
                        }
                        (_, Some(Err(error))) => {
                            failures.push(format!("{}: {error}", path.display()));
                            continue;
                        }
                        _ => {
                            failures.push(format!(
                                "{} changed on disk or active document changed",
                                path.display()
                            ));
                            continue;
                        }
                    };
                    let editor = if preview.version.is_none()
                        && active_entity == Some(this.editor.entity_id())
                    {
                        Some(this.editor.clone())
                    } else if this.path.as_ref().is_some_and(|p| same_path(p, &path)) {
                        Some(this.editor.clone())
                    } else {
                        this.documents
                            .iter()
                            .find(|d| d.path.as_ref().is_some_and(|p| same_path(p, &path)))
                            .map(|d| d.editor.clone())
                    };
                    let editor = match editor {
                        Some(editor) => editor,
                        None if this.documents.len() + 1 < 32 => {
                            let Some(loaded) = loaded else {
                                failures.push(format!("{}: document unavailable", path.display()));
                                continue;
                            };
                            let loaded_text = loaded
                                .buffer
                                .text_in_range(0..loaded.buffer.len_chars())
                                .unwrap_or_default();
                            if loaded_text != preview.plan.before {
                                failures.push(format!("{} changed in the editor", path.display()));
                                continue;
                            }
                            let editor = cx.new(|cx| EditorView::new("", cx));
                            editor.update(cx, |e, cx| e.load(loaded.buffer, cx));
                            cx.observe(&editor, |_, _, cx| cx.notify()).detach();
                            this.document_order.push(editor.entity_id());
                            this.documents.push(documents::Document {
                                editor: editor.clone(),
                                path: Some(path.clone()),
                                disk: Some(loaded.version),
                                replacement_target: None,
                            });
                            editor
                        }
                        None => {
                            failures.push(format!("{}: document limit reached", path.display()));
                            continue;
                        }
                    };
                    if editor.read(cx).text() != preview.plan.before {
                        failures.push(format!("{} changed in the editor", path.display()));
                        continue;
                    }
                    if preview.plan.before == preview.plan.after {
                        continue;
                    }
                    editor.update(cx, |e, cx| e.replace_document(&preview.plan.after, cx));
                    this.workspace_search.undo.push(UndoFile {
                        revision: editor.read(cx).content_revision(),
                        editor,
                        path: path.clone(),
                    });
                    applied.push(path.display().to_string());
                }
                this.workspace_search.preview.clear();
                this.workspace_search.report = None;
                this.workspace_search.outcomes = applied
                    .iter()
                    .map(|path| format!("Applied: {path}"))
                    .chain(failures.iter().map(|path| format!("Skipped: {path}")))
                    .collect();
                this.status = format!(
                    "Applied {} files: {}. Skipped {} files: {}",
                    applied.len(),
                    if applied.is_empty() {
                        "none".into()
                    } else {
                        applied.join(", ")
                    },
                    failures.len(),
                    if failures.is_empty() {
                        "none".into()
                    } else {
                        failures.join(", ")
                    }
                );
                cx.notify();
            });
        })
        .detach();
    }

    fn undo_replacements(&mut self, cx: &mut Context<Self>) {
        let mut undone = Vec::new();
        let mut retained = Vec::new();
        let mut skipped = Vec::new();
        for entry in self.workspace_search.undo.drain(..) {
            let open = self.editor.entity_id() == entry.editor.entity_id()
                || self
                    .documents
                    .iter()
                    .any(|d| d.editor.entity_id() == entry.editor.entity_id());
            if open && entry.editor.read(cx).content_revision() == entry.revision {
                entry.editor.update(cx, |e, cx| {
                    e.edit_command(ale_editor_view::EditCommand::Undo, cx)
                });
                undone.push(entry.path.display().to_string());
            } else {
                skipped.push(entry.path.display().to_string());
                retained.push(entry);
            }
        }
        self.workspace_search.undo = retained;
        self.status = format!(
            "Undone: {}. Changed since apply: {}.",
            if undone.is_empty() {
                "none".into()
            } else {
                undone.join(", ")
            },
            if skipped.is_empty() {
                "none".into()
            } else {
                skipped.join(", ")
            }
        );
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
        if self.workspace_search.active_entity == Some(self.editor.entity_id())
            && self.path.is_none()
        {
            if <[u8; 32]>::from(Sha256::digest(self.editor.read(cx).text().as_bytes())) == digest {
                self.show_editor(window, cx);
                self.editor
                    .update(cx, |editor, cx| editor.select_range(range.clone(), cx));
            } else {
                self.status = "File changed since search. Search again.".into();
                cx.notify();
            }
            return;
        }
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

    pub(super) fn workspace_search_view(&self, cx: &Context<Self>) -> impl IntoElement {
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
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id(("select-match", index))
                                            .cursor_pointer()
                                            .child(
                                                if self.workspace_search.selected.get(index)
                                                    == Some(&true)
                                                {
                                                    "☑"
                                                } else {
                                                    "☐"
                                                },
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(selected) =
                                                    this.workspace_search.selected.get_mut(index)
                                                {
                                                    *selected = !*selected;
                                                }
                                                this.cancel_workspace_search();
                                                cx.stop_propagation();
                                                cx.notify();
                                            })),
                                    )
                                    .child(format!(
                                        "{}:{}:{}",
                                        path.display(),
                                        result.line + 1,
                                        result.column + 1
                                    )),
                            )
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
            .id("workspace-search-pane")
            .overflow_y_scroll()
            .min_h_0()
            .w(px(330.0))
            .max_w(gpui::relative(0.5))
            .min_w_0()
            .flex_shrink()
            .bg(rgb(0x15191f))
            .child(
                div()
                    .w_full()
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
                    .child(
                        toggle(
                            if self.workspace_search.current_file {
                                "Scope: current file"
                            } else {
                                "Scope: workspace"
                            },
                            self.workspace_search.current_file,
                        )
                        .id("search-scope")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.workspace_search.current_file =
                                !this.workspace_search.current_file;
                            this.cancel_workspace_search();
                            this.workspace_search.report = None;
                            this.workspace_search.preview.clear();
                            cx.notify();
                        })),
                    )
                    .child(input("Search", self.workspace_search.query.clone()))
                    .child(input(
                        "Replace with",
                        self.workspace_search.replacement.clone(),
                    ))
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
                            .child(toggle("Search", false).id("run-search").on_click(
                                cx.listener(|this, _, _, cx| this.run_workspace_search(cx)),
                            ))
                            .child(toggle("Cancel", false).id("cancel-search").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.cancel_workspace_search();
                                    this.status = "Search cancelled.".into();
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(toggle("Preview", false).id("preview-replace").on_click(
                                cx.listener(|this, _, _, cx| this.preview_replacements(cx)),
                            ))
                            .child(toggle("Apply", false).id("apply-replace").on_click(
                                cx.listener(|this, _, _, cx| this.apply_replacements(cx)),
                            ))
                            .child(toggle("Undo all", false).id("undo-replace").on_click(
                                cx.listener(|this, _, _, cx| this.undo_replacements(cx)),
                            )),
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
                            .child(toggle("Next", false).id("next-search-page").on_click(
                                cx.listener(|this, _, _, cx| {
                                    if this.workspace_search.report.as_ref().is_some_and(|report| {
                                        (this.workspace_search.page + 1) * 100
                                            < report.matches.len()
                                    }) {
                                        this.workspace_search.page += 1;
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(format!("Page {}", self.workspace_search.page + 1)),
                    )
                    .when(!self.workspace_search.preview.is_empty(), |view| {
                        view.child(
                            div()
                                .flex()
                                .gap_2()
                                .child(toggle("Previous diff", false).id("previous-diff").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.workspace_search.preview_page =
                                            this.workspace_search.preview_page.saturating_sub(1);
                                        cx.notify();
                                    }),
                                ))
                                .child(toggle("Next diff", false).id("next-diff").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        let total: usize = this
                                            .workspace_search
                                            .preview
                                            .iter()
                                            .map(|file| file.snippets.len())
                                            .sum();
                                        if (this.workspace_search.preview_page + 1) * 100 < total {
                                            this.workspace_search.preview_page += 1;
                                        }
                                        cx.notify();
                                    }),
                                ))
                                .child(format!(
                                    "Diff page {}",
                                    self.workspace_search.preview_page + 1
                                )),
                        )
                    })
                    .child(div().text_size(px(11.0)).child(self.status.clone()))
                    .child(
                        div()
                            .id("workspace-results")
                            .text_size(px(12.0))
                            .children(
                                self.workspace_search
                                    .preview
                                    .iter()
                                    .flat_map(|file| {
                                        file.snippets
                                            .iter()
                                            .map(move |snippet| (&file.plan.path, snippet))
                                    })
                                    .skip(self.workspace_search.preview_page * 100)
                                    .take(100)
                                    .map(|(path, snippet)| {
                                        div()
                                            .whitespace_normal()
                                            .text_size(px(11.0))
                                            .child(path.display().to_string())
                                            .child(snippet.clone())
                                    }),
                            )
                            .children(
                                self.workspace_search
                                    .outcomes
                                    .iter()
                                    .cloned()
                                    .map(|outcome| div().whitespace_normal().child(outcome)),
                            )
                            .children(rows),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn replace_preview_skips_conflict_and_undoes_dirty_and_disk_files(cx: &mut TestAppContext) {
        use gpui::EntityInputHandler;
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        let c = dir.path().join("c.txt");
        let d = dir.path().join("d.txt");
        std::fs::write(&a, "needle needle").unwrap();
        std::fs::write(&b, "needle").unwrap();
        std::fs::write(&c, "needle").unwrap();
        std::fs::write(&d, "needle").unwrap();
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                s.select_folder(dir.path().to_path_buf(), window, cx);
                s.open_document(persistence::load(&a).unwrap(), None, window, cx);
            })
        });
        cx.simulate_keystrokes("ctrl-end");
        cx.simulate_input(" dirty");
        cx.update(|window, cx| shell.update(cx, |s, cx| s.show_workspace_search(window, cx)));
        cx.simulate_input("needle");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                let report = s.workspace_search.report.as_ref().unwrap();
                assert_eq!(report.total, 5);
                let last_a = report
                    .matches
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.path == a)
                    .next_back()
                    .unwrap()
                    .0;
                s.workspace_search.selected[last_a] = false;
                s.workspace_search.replacement.update(cx, |e, cx| {
                    e.replace_text_in_range(None, "change", window, cx)
                });
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| shell.update(cx, |s, cx| s.preview_replacements(cx)));
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(shell.read(cx).workspace_search.preview.len(), 4));
        std::fs::write(&c, "external").unwrap();
        cx.update(|_, cx| shell.update(cx, |s, cx| s.apply_replacements(cx)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            shell.update(cx, |s, cx| {
                assert_eq!(s.editor.read(cx).text(), "change needle dirty");
                let editor_d = s
                    .documents
                    .iter()
                    .find(|doc| doc.path.as_ref() == Some(&d))
                    .unwrap()
                    .editor
                    .clone();
                assert_eq!(editor_d.read(cx).text(), "change");
                let editor_b = s
                    .documents
                    .iter()
                    .find(|d| d.path.as_ref() == Some(&b))
                    .unwrap()
                    .editor
                    .clone();
                assert_eq!(editor_b.read(cx).text(), "change");
                assert!(s.status.contains("changed on disk"));
                assert_eq!(std::fs::read_to_string(&c).unwrap(), "external");
                s.undo_replacements(cx);
                assert_eq!(s.editor.read(cx).text(), "needle needle dirty");
                assert_eq!(editor_b.read(cx).text(), "needle");
                assert_eq!(editor_d.read(cx).text(), "needle");
            })
        });
    }

    #[test]
    fn sentinel_named_document_still_requires_file_backing() {
        let path = std::path::Path::new("/__untitled_workspace_buffer__");
        assert!(!is_untitled(true, Some(path)));
        assert!(is_untitled(true, None));
        assert!(!is_untitled(false, None));
    }

    #[test]
    fn preview_budget_accounts_for_output_and_diff_copies() {
        let plan = workspace_replace::FilePlan {
            path: PathBuf::from("example"),
            before: "x".into(),
            after: "y".repeat(100),
            edits: vec![workspace_replace::Edit {
                range: 0..1,
                replacement: "y".repeat(100),
            }],
        };
        assert_eq!(preview_bytes(&plan), Some(307));
    }

    #[gpui::test]
    fn replace_current_untitled_buffer(cx: &mut TestAppContext) {
        use gpui::EntityInputHandler;
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("needle");
        cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                s.show_workspace_search(window, cx);
                s.workspace_search.current_file = true;
            })
        });
        cx.simulate_input("needle");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                assert_eq!(s.workspace_search.report.as_ref().unwrap().total, 1);
                s.workspace_search.replacement.update(cx, |e, cx| {
                    e.replace_text_in_range(None, "changed", window, cx)
                });
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| shell.update(cx, |s, cx| s.preview_replacements(cx)));
        cx.run_until_parked();
        cx.update(|_, cx| shell.update(cx, |s, cx| s.apply_replacements(cx)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            shell.update(cx, |s, cx| {
                assert_eq!(s.editor.read(cx).text(), "changed");
                s.undo_replacements(cx);
                assert_eq!(s.editor.read(cx).text(), "needle");
            })
        });
    }

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
