//! Compact in-window menus and transient navigation inputs, including on Linux.
use super::*;
use ale_editor_view::EditCommand;
use gpui::{FocusHandle, KeyDownEvent};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Bar {
    Find,
    Line,
}
pub(super) struct CommandState {
    pub input: Entity<EditorView>,
    pub bar: Option<Bar>,
    pub menu: Option<usize>,
    selected: usize,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    generation: u64,
    searching: bool,
    find_query: String,
}
impl CommandState {
    pub fn new(cx: &mut Context<AppShell>) -> Self {
        Self {
            input: cx.new(|cx| EditorView::input("", cx)),
            bar: None,
            menu: None,
            selected: 0,
            focus: cx.focus_handle(),
            return_focus: None,
            generation: 0,
            searching: false,
            find_query: String::new(),
        }
    }
}
#[derive(Clone, Copy)]
enum Command {
    New,
    Open,
    Folder,
    Inspect,
    Save,
    SaveAs,
    Close,
    Quit,
    Edit(EditCommand),
    Find,
    NextMatch,
    PreviousMatch,
    Line,
    Files,
    Refresh,
    Output,
    Panels,
    Editor,
    Assembly,
    Bytes,
    Overview,
    Terminal,
    RestartTerminal,
    StopTerminal,
    NextDocument,
    Compile,
    Relocate,
    Export,
}
const MENU_NAMES: [&str; 6] = ["File", "Edit", "View", "Navigate", "Terminal", "Binary"];
fn items(menu: usize) -> Vec<(&'static str, &'static str, Command)> {
    use Command::*;
    match menu {
        0 => vec![
            ("New File", "Ctrl+N", New),
            ("Open File…", "Ctrl+O", Open),
            ("Open Folder…", "", Folder),
            ("Inspect Binary…", "Ctrl+Shift+O", Inspect),
            ("Save", "Ctrl+S", Save),
            ("Save As…", "Ctrl+Shift+S", SaveAs),
            ("Close Document", "Ctrl+W", Close),
            ("Quit", "Ctrl+Q", Quit),
        ],
        1 => vec![
            ("Undo", "Ctrl+Z", Edit(EditCommand::Undo)),
            ("Redo", "Ctrl+Shift+Z", Edit(EditCommand::Redo)),
            ("Cut", "Ctrl+X", Edit(EditCommand::Cut)),
            ("Copy", "Ctrl+C", Edit(EditCommand::Copy)),
            ("Paste", "Ctrl+V", Edit(EditCommand::Paste)),
            ("Select All", "Ctrl+A", Edit(EditCommand::SelectAll)),
            ("Find…", "Ctrl+F", Find),
        ],
        2 => vec![
            ("Files Sidebar", "", Files),
            ("Refresh Files", "", Refresh),
            ("Output", "", Output),
            ("Toggle Bottom Panel", "Ctrl+B", Panels),
            ("Focus Editor", "Ctrl+Alt+E", Editor),
            ("Focus Terminal", "Ctrl+Alt+T", Terminal),
        ],
        3 => vec![
            ("Go to Line…", "Ctrl+G", Line),
            ("Next Match", "F3", NextMatch),
            ("Previous Match", "Shift+F3", PreviousMatch),
            ("Next Document", "Ctrl+Tab", NextDocument),
            ("Text", "Ctrl+1", Editor),
            ("Binary Overview", "Ctrl+2", Overview),
            ("Assembly", "Ctrl+3", Assembly),
            ("Bytes", "Ctrl+4", Bytes),
        ],
        4 => vec![
            ("Focus Terminal", "Ctrl+Alt+T", Terminal),
            ("Restart Shell", "", RestartTerminal),
            ("Stop Shell", "", StopTerminal),
        ],
        _ => vec![
            ("Compile Recovered C / Cancel", "", Compile),
            ("Toggle Relocated Replacement", "", Relocate),
            ("Export Copy…", "", Export),
        ],
    }
}
impl AppShell {
    pub(super) fn dismiss_transient(&mut self, cx: &mut Context<Self>) {
        self.commands.bar = None;
        self.commands.menu = None;
        self.commands.generation = self.commands.generation.wrapping_add(1);
        cx.notify();
    }
    pub(super) fn open_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.commands.menu.is_none() {
            self.commands.return_focus = window.focused(cx);
        }
        self.commands.menu = Some(index);
        self.commands.selected = 0;
        self.commands.focus.focus(window);
        cx.notify();
    }
    pub(super) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commands.menu = None;
        if let Some(focus) = self.commands.return_focus.take() {
            focus.focus(window);
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }
    pub(super) fn open_bar(&mut self, bar: Bar, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.systems_active = false;
        if self.commands.bar != Some(bar) {
            let text = if bar == Bar::Find {
                self.commands.find_query.clone()
            } else {
                String::new()
            };
            self.commands.input.update(cx, |input, cx| {
                input.load(ale_editor_core::EditorBuffer::with_text(&text), cx)
            });
        }
        self.commands.bar = Some(bar);
        self.commands.input.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }
    pub(super) fn run_command_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.commands.bar {
            Some(Bar::Find) => self.find_match(false, cx),
            Some(Bar::Line) => {
                let text = self.commands.input.read(cx).text();
                match text.trim().parse::<usize>() {
                    Ok(line) if line > 0 && line <= self.editor.read(cx).line_count() => {
                        self.editor
                            .update(cx, |editor, cx| editor.go_to_line(line - 1, cx));
                        self.commands.bar = None;
                        self.focus_active(window, cx);
                    }
                    _ => {
                        self.status = format!(
                            "Enter a line from 1 to {}.",
                            self.editor.read(cx).line_count()
                        )
                    }
                }
                cx.notify();
            }
            None => {}
        }
    }
    pub(super) fn find_match(&mut self, backwards: bool, cx: &mut Context<Self>) {
        if self.systems_active || self.commands.searching {
            return;
        }
        let input = self.commands.input.clone();
        let input_active = self.commands.bar == Some(Bar::Find);
        let query = if input_active {
            input.read(cx).text()
        } else {
            self.commands.find_query.clone()
        };
        self.commands.find_query = query.clone();
        if query.is_empty() {
            self.status = "Enter literal text to find.".into();
            cx.notify();
            return;
        }
        let input_revision = input.read(cx).content_revision();
        let editor = self.editor.clone();
        let revision = editor.read(cx).content_revision();
        let selection = editor.read(cx).selection().range();
        let snapshot = editor.read(cx).snapshot();
        self.commands.generation = self.commands.generation.wrapping_add(1);
        let generation = self.commands.generation;
        self.commands.searching = true;
        let work = cx
            .background_executor()
            .spawn(async move { snapshot.find_literal(&query, 10_000) });
        cx.spawn(async move |this, cx| {
            let found = work.await;
            let _ = this.update(cx, |this, cx| {
                this.commands.searching = false;
                if this.commands.generation != generation {
                    return;
                }
                if this.editor.entity_id() != editor.entity_id()
                    || editor.read(cx).content_revision() != revision
                    || (input_active && input.read(cx).content_revision() != input_revision)
                {
                    return;
                }
                let index = if backwards {
                    found
                        .ranges
                        .iter()
                        .rposition(|r| r.start < selection.start)
                        .or_else(|| found.ranges.len().checked_sub(1))
                } else {
                    found
                        .ranges
                        .iter()
                        .position(|r| r.start >= selection.end)
                        .or(if found.ranges.is_empty() {
                            None
                        } else {
                            Some(0)
                        })
                };
                if let Some(index) = index {
                    editor.update(cx, |editor, cx| {
                        editor.select_range(found.ranges[index].clone(), cx)
                    });
                    this.status = format!(
                        "Match {} of {}{}",
                        index + 1,
                        found.total,
                        if found.total > found.ranges.len() {
                            " (navigation limited to first 10,000; refine the query)"
                        } else {
                            ""
                        }
                    );
                } else {
                    this.status = "No matches".into();
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn execute_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(window, cx);
        use Command::*;
        match command {
            New => {
                if self.fresh_document(cx) {
                    self.show_editor(window, cx);
                }
            }
            Open => self.request_open(window, cx),
            Folder => self.choose_folder(window, cx),
            Inspect => self.request_inspect(window, cx),
            Save => self.save_active(false, window, cx),
            SaveAs => self.save_active(true, window, cx),
            Close => self.close_document(window, cx),
            Quit => self.request_replace(Intent::Close, window, cx),
            Edit(command) => {
                if self
                    .commands
                    .input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
                {
                    self.commands
                        .input
                        .update(cx, |e, cx| e.edit_command(command, cx));
                } else if self.systems_active {
                    if let Some(w) = &self.workbench {
                        w.update(cx, |w, cx| w.edit_command(command, window, cx));
                    }
                } else if self.editor.read(cx).focus_handle(cx).is_focused(window) {
                    self.editor.update(cx, |e, cx| e.edit_command(command, cx));
                }
            }
            Find => self.open_bar(Bar::Find, window, cx),
            Line => self.open_bar(Bar::Line, window, cx),
            NextMatch => self.find_match(false, cx),
            PreviousMatch => self.find_match(true, cx),
            Files => {
                self.explorer_visible = !self.explorer_visible;
            }
            Refresh => {
                let paths = self.explorer_tree.refresh();
                self.load_folders(paths, window, cx);
            }
            Output => {
                self.panel = Panel::Output;
                self.panels_visible = true;
            }
            Panels => {
                self.panels_visible = !self.panels_visible;
            }
            Editor => self.show_editor(window, cx),
            Assembly => self.show_systems(Lens::Assembly, window, cx),
            Bytes => self.show_systems(Lens::Bytes, window, cx),
            Overview => self.show_systems(Lens::Overview, window, cx),
            Terminal => self.show_terminal(window, cx),
            RestartTerminal => {
                if let Some(t) = &self.terminal {
                    self.panels_visible = true;
                    self.panel = Panel::Terminal;
                    t.update(cx, |t, cx| t.restart(window, cx));
                } else {
                    self.show_terminal(window, cx);
                }
            }
            StopTerminal => {
                if let Some(t) = &self.terminal {
                    t.update(cx, |t, cx| t.stop(cx));
                }
            }
            NextDocument => {
                self.next_document(window, cx);
            }
            Compile => {
                self.compile_replacement(cx);
                self.focus_active(window, cx);
            }
            Relocate => {
                if let Some(w) = &self.workbench {
                    w.update(cx, |w, cx| w.toggle_relocated_mode(cx));
                    self.status = if w.read(cx).relocated_mode() {
                        "Relocated replacement enabled. Review the supported ELF and ABI limits."
                    } else {
                        "Fixed-span replacement: the function must fit its original space."
                    }
                    .into();
                }
            }
            Export => self.request_export(window, cx),
        }
        cx.notify();
    }
    pub(super) fn menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.commands.menu else {
            return;
        };
        let count = items(menu).len();
        match event.keystroke.key.as_str() {
            "escape" => self.close_menu(window, cx),
            "down" => self.commands.selected = (self.commands.selected + 1) % count,
            "up" => self.commands.selected = (self.commands.selected + count - 1) % count,
            "right" => self.open_menu((menu + 1) % MENU_NAMES.len(), window, cx),
            "left" => self.open_menu((menu + MENU_NAMES.len() - 1) % MENU_NAMES.len(), window, cx),
            "enter" => self.execute_command(items(menu)[self.commands.selected].2, window, cx),
            _ => {}
        }
        cx.stop_propagation();
        cx.notify();
    }
    pub(super) fn menu_bar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(28.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(0x2a313c))
            .bg(rgb(0x15191f))
            .children(MENU_NAMES.iter().enumerate().map(|(index, name)| {
                div()
                    .id(("menu", index))
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .when(self.commands.menu == Some(index), |v| v.bg(rgb(0x233044)))
                    .child(*name)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.commands.menu == Some(index) {
                            this.close_menu(window, cx);
                        } else {
                            this.open_menu(index, window, cx);
                        }
                    }))
            }))
    }
    pub(super) fn menu_popup(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let offsets = [0., 48., 98., 150., 231., 310.];
        div()
            .absolute()
            .top(px(28.0))
            .left(px(offsets[index]))
            .w(px(300.0))
            .p_1()
            .bg(rgb(0x1b2028))
            .border_1()
            .border_color(rgb(0x465064))
            .shadow_lg()
            .occlude()
            .track_focus(&self.commands.focus)
            .key_context("EditorMenu")
            .on_key_down(cx.listener(Self::menu_key))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_menu(window, cx)))
            .children(
                items(index)
                    .into_iter()
                    .enumerate()
                    .map(|(row, (label, key, command))| {
                        let label = if matches!(command, Command::Relocate) {
                            format!(
                                "Relocated Replacement: {}",
                                if self
                                    .workbench
                                    .as_ref()
                                    .is_some_and(|w| w.read(cx).relocated_mode())
                                {
                                    "On"
                                } else {
                                    "Off"
                                }
                            )
                        } else {
                            label.to_owned()
                        };
                        let key = if cfg!(target_os = "macos") {
                            key.replace("Ctrl", "Cmd")
                        } else {
                            key.into()
                        };
                        div()
                            .id(("menu-row", row))
                            .h(px(26.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .justify_between()
                            .cursor_pointer()
                            .when(row == self.commands.selected, |v| v.bg(rgb(0x233044)))
                            .hover(|v| v.bg(rgb(0x233044)))
                            .child(label)
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(0x8590a3))
                                    .child(key),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.execute_command(command, window, cx)
                            }))
                    }),
            )
    }
    pub(super) fn command_bar(&self, bar: Bar) -> Div {
        div()
            .h(px(30.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(rgb(0x2a313c))
            .bg(rgb(0x15191f))
            .child(if bar == Bar::Find { "Find" } else { "Line" })
            .child(
                div()
                    .w(px(280.0))
                    .h(px(24.0))
                    .child(self.commands.input.clone()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(0x8590a3))
                    .child(if bar == Bar::Find {
                        "Enter / F3 next · Shift+F3 previous · Esc close"
                    } else {
                        "Enter to go · Esc cancel"
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    #[gpui::test]
    fn find_wraps_without_edits_and_remains_available_after_escape(cx: &mut TestAppContext) {
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("λ one λ two");
        let revision = cx.update(|_, cx| shell.read(cx).editor.read(cx).content_revision());
        cx.dispatch_action(Find);
        cx.simulate_input("λ");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(shell.read(cx).editor.read(cx).selection().range(), 0..1));
        cx.dispatch_action(FindNext);
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(shell.read(cx).editor.read(cx).selection().range(), 6..7));
        cx.simulate_keystrokes("escape");
        cx.dispatch_action(FindNext);
        cx.run_until_parked();
        cx.update(|_, cx| {
            let s = shell.read(cx);
            let e = s.editor.read(cx);
            assert_eq!(e.selection().range(), 0..1);
            assert_eq!(e.content_revision(), revision);
            assert!(s.commands.bar.is_none());
        });
    }
    #[gpui::test]
    fn stale_search_cannot_select_text_in_another_document(cx: &mut TestAppContext) {
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("needle");
        cx.dispatch_action(Find);
        cx.simulate_input("needle");
        cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                s.find_match(false, cx);
                assert!(s.fresh_document(cx));
                s.show_editor(window, cx);
            })
        });
        cx.simulate_input("different");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let e = shell.read(cx).editor.read(cx);
            assert_eq!(e.text(), "different");
            assert_eq!(e.selection().range(), 9..9);
        });
    }
    #[gpui::test]
    fn goto_validates_one_based_lines_and_menu_escape_restores_focus(cx: &mut TestAppContext) {
        let (shell, cx) = cx.add_window_view(AppShell::new);
        cx.simulate_input("first\nsecond\nthird");
        cx.dispatch_action(GoToLine);
        cx.simulate_input("0");
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| assert_eq!(shell.read(cx).editor.read(cx).cursor().line, 2));
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("2");
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| assert_eq!(shell.read(cx).editor.read(cx).cursor().line, 1));
        cx.dispatch_action(OpenMenu);
        cx.simulate_keystrokes("right down escape");
        cx.simulate_input("X");
        cx.update(|_, cx| {
            let s = shell.read(cx);
            assert!(s.commands.menu.is_none());
            assert_eq!(s.editor.read(cx).text(), "first\nXsecond\nthird");
        });
    }
}
