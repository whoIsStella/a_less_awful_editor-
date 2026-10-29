//! PTY-backed Linux shell panel. Terminal output never becomes editor text.
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent, MouseButton,
    Pixels, Render, TextRun, Window, canvas, div, fill, point, prelude::*, px, rgb, size,
};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    num::NonZeroU16,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
fn nz(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value.max(1)).unwrap()
}
struct Reply(mpsc::SyncSender<Vec<u8>>);
impl vt100::Callbacks for Reply {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        _: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let response = if c == 'n' && params.first().is_some_and(|p| *p == [6]) {
            let (r, c) = screen.cursor_position();
            Some(format!("\x1b[{};{}R", r + 1, c + 1))
        } else if c == 'c' {
            Some("\x1b[?1;2c".into())
        } else {
            None
        };
        if let Some(response) = response {
            let _ = self.0.try_send(response.into_bytes());
        }
    }
}
struct State {
    parser: vt100::Parser<Reply>,
    exit: Option<String>,
    error: Option<String>,
}
pub struct Session {
    master: Box<dyn MasterPty + Send>,
    tx: mpsc::SyncSender<Vec<u8>>,
    state: Arc<Mutex<State>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    pid: Option<u32>,
    dimensions: (u16, u16),
    stopped: Arc<AtomicBool>,
}
impl Session {
    fn start(directory: &Path, shell: &str) -> Result<Self, String> {
        let pair = portable_pty::native_pty_system()
            .openpty(PtySize {
                rows: 12,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(32);
        let state = Arc::new(Mutex::new(State {
            parser: vt100::Parser::new_with_callbacks(nz(12), nz(80), 2000, Reply(tx.clone())),
            exit: None,
            error: None,
        }));
        let mut command = CommandBuilder::new(shell);
        command.arg("-i");
        command.cwd(directory);
        command.env("TERM", "xterm-256color");
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| e.to_string())?;
        let pid = child.process_id();
        let killer = child.clone_killer();
        drop(pair.slave);
        let shared = state.clone();
        std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => shared.lock().unwrap().parser.process(&bytes[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
        let stopped = Arc::new(AtomicBool::new(false));
        let stop_writer = stopped.clone();
        let shared = state.clone();
        std::thread::spawn(move || {
            while !stop_writer.load(Ordering::Acquire) {
                let bytes = match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(bytes) => bytes,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                if let Err(e) = writer.write_all(&bytes) {
                    shared.lock().unwrap().error = Some(e.to_string());
                    break;
                }
            }
        });
        let shared = state.clone();
        std::thread::spawn(move || {
            let status = match child.wait() {
                Ok(status) => format!("Shell exited: {status}"),
                Err(e) => format!("Shell wait failed: {e}"),
            };
            shared.lock().unwrap().exit = Some(status);
        });
        Ok(Self {
            master: pair.master,
            tx,
            state,
            killer,
            pid,
            dimensions: (12, 80),
            stopped,
        })
    }
    fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() > 65536 {
            return Err("Paste is limited to 64 KiB.".into());
        }
        self.tx
            .try_send(bytes)
            .map_err(|_| "Terminal input queue is full or closed; input was not sent.".into())
    }
    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), String> {
        let dims = (rows.clamp(1, 120), cols.clamp(1, 400));
        if self.dimensions == dims {
            return Ok(());
        }
        self.master
            .resize(PtySize {
                rows: dims.0,
                cols: dims.1,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        self.state
            .lock()
            .unwrap()
            .parser
            .set_size(nz(dims.0), nz(dims.1));
        self.dimensions = dims;
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // Signal the PTY foreground group as well as the shell. Detached daemons are
        // outside this session's ownership; no host-wide process scan is performed.
        #[cfg(unix)]
        {
            if let Some(group) = self.master.process_group_leader() {
                if group > 1 {
                    unsafe {
                        libc::kill(-group, libc::SIGHUP);
                    }
                }
            }
            if let Some(pid) = self.pid {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGHUP);
                }
            }
        }
        let _ = self.killer.kill();
    }
}

pub struct Terminal {
    session: Option<Session>,
    directory: PathBuf,
    focus: FocusHandle,
    message: String,
}
impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Terminal {
    pub fn new(directory: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            session: None,
            directory,
            focus: cx.focus_handle(),
            message: String::new(),
        };
        view.start();
        view.focus.focus(window);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        view
    }
    fn start(&mut self) {
        self.session = None;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        match Session::start(&self.directory, &shell) {
            Ok(s) => {
                self.session = Some(s);
                self.message = String::new();
            }
            Err(e) => self.message = format!("Could not start shell: {e}"),
        }
    }
    fn send(&mut self, bytes: Vec<u8>) {
        if let Some(session) = &self.session {
            session
                .state
                .lock()
                .unwrap()
                .parser
                .screen_mut()
                .set_scrollback(0);
            if let Err(e) = session.send(bytes) {
                self.message = e;
            }
        }
    }
    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let key = k.key.as_str();
        if k.modifiers.control && k.modifiers.shift && key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                let bracketed = self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.state.lock().unwrap().parser.screen().bracketed_paste());
                self.send(if bracketed {
                    format!("\x1b[200~{text}\x1b[201~").into_bytes()
                } else {
                    text.into_bytes()
                });
            }
            cx.stop_propagation();
            return;
        }
        let application = self
            .session
            .as_ref()
            .is_some_and(|s| s.state.lock().unwrap().parser.screen().application_cursor());
        let bytes = if k.modifiers.control && !k.modifiers.shift && key.len() == 1 {
            let byte = key.as_bytes()[0];
            Some(vec![if byte == b' ' {
                0
            } else {
                byte.to_ascii_uppercase() & 0x1f
            }])
        } else {
            match key {
                "enter" => Some(vec![13]),
                "backspace" => Some(vec![127]),
                "tab" => Some(vec![9]),
                "escape" => Some(vec![27]),
                "up" | "down" | "right" | "left" => Some(
                    format!(
                        "\x1b{}{}",
                        if application { "O" } else { "[" },
                        match key {
                            "up" => "A",
                            "down" => "B",
                            "right" => "C",
                            _ => "D",
                        }
                    )
                    .into_bytes(),
                ),
                "home" => Some(b"\x1b[H".to_vec()),
                "end" => Some(b"\x1b[F".to_vec()),
                "delete" => Some(b"\x1b[3~".to_vec()),
                "pageup" => Some(b"\x1b[5~".to_vec()),
                "pagedown" => Some(b"\x1b[6~".to_vec()),
                _ => k.key_char.as_ref().map(|s| s.as_bytes().to_vec()),
            }
        };
        if let Some(mut bytes) = bytes {
            if k.modifiers.alt {
                bytes.insert(0, 27);
            }
            self.send(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }
    fn paint(view: Entity<Self>, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let font = gpui::font("DejaVu Sans Mono");
        let font_size = px(13.0);
        let sample = window.text_system().shape_line(
            "M".into(),
            font_size,
            &[TextRun {
                len: 1,
                font: font.clone(),
                color: rgb(0xd4dce7).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let cell_width = f32::from(sample.width).max(1.0);
        let rows = (f32::from(bounds.size.height) / 18.0).floor().max(1.0) as u16;
        let cols = (f32::from(bounds.size.width) / cell_width).floor().max(1.0) as u16;
        view.update(cx, |this, cx| {
            let Some(session) = this.session.as_mut() else {
                return;
            };
            if session.dimensions != (rows.clamp(1, 120), cols.clamp(1, 400)) {
                if let Err(e) = session.resize(rows, cols) {
                    this.message = e;
                }
                cx.notify();
            }
            let state = session.state.lock().unwrap();
            let screen = state.parser.screen();
            for (row, text) in screen.rows(0, cols.min(400)).enumerate() {
                let run = TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: rgb(0xd4dce7).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line = window
                    .text_system()
                    .shape_line(text.into(), font_size, &[run], None);
                let _ = line.paint(
                    bounds.origin + point(px(0.0), px(row as f32 * 18.0)),
                    px(18.0),
                    window,
                    cx,
                );
            }
            if !screen.hide_cursor() && this.focus.is_focused(window) {
                let (r, c) = screen.cursor_position();
                window.paint_quad(fill(
                    Bounds {
                        origin: bounds.origin
                            + point(px(c as f32 * cell_width), px(r as f32 * 18.0)),
                        size: size(px(2.0), px(18.0)),
                    },
                    rgb(0x7aa2f7),
                ));
            }
        });
    }
}
impl Render for Terminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self
            .session
            .as_ref()
            .and_then(|s| {
                let s = s.state.lock().unwrap();
                s.error.clone().or(s.exit.clone())
            })
            .unwrap_or_else(|| self.message.clone());
        let entity = cx.entity();
        div()
            .flex()
            .flex_col()
            .size_full()
            .font_family("monospace")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                if let Some(session) = &this.session {
                    let mut state = session.state.lock().unwrap();
                    let screen = state.parser.screen_mut();
                    let delta = f32::from(event.delta.pixel_delta(px(18.0)).y) / 18.0;
                    let offset = (screen.scrollback() as f32 + delta).max(0.0) as usize;
                    screen.set_scrollback(offset);
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| this.focus.focus(window)),
            )
            .child(
                div()
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.directory.display().to_string())
                    .child(
                        crate::systems::button("Restart shell").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.start();
                                this.focus.focus(window);
                                cx.notify();
                            },
                        )),
                    )
                    .child(crate::systems::button("Stop").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.session = None;
                            this.message = "Shell stopped".into();
                            cx.notify();
                        },
                    )))
                    .child(crate::systems::button("Copy screen").on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(s) = &this.session {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    s.state.lock().unwrap().parser.screen().contents(),
                                ));
                            }
                        },
                    ))),
            )
            .child(
                div().flex_1().min_h_0().overflow_hidden().child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, (), window, cx| Self::paint(entity, bounds, window, cx),
                    )
                    .size_full(),
                ),
            )
            .when(!status.is_empty(), |v| {
                v.child(div().text_size(px(11.0)).child(status))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(session: &Session, text: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < deadline {
            if session
                .state
                .lock()
                .unwrap()
                .parser
                .screen()
                .contents()
                .contains(text)
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "missing {text}: {}",
            session.state.lock().unwrap().parser.screen().contents()
        );
    }
    #[test]
    fn real_shell_directory_resize_unicode_and_interrupt() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Session::start(d.path(), "/bin/sh").unwrap();
        s.resize(18, 92).unwrap();
        s.send(b"printf 'HELLO-%s\\n' '\xce\xbb'; pwd; stty size\r".to_vec())
            .unwrap();
        wait(&s, "HELLO-λ");
        wait(&s, d.path().to_str().unwrap());
        wait(&s, "18 92");
        s.send(b"sleep 30\r".to_vec()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        s.send(vec![3]).unwrap();
        s.send(b"printf 'AFTER-%s\\n' INTERRUPT\r".to_vec())
            .unwrap();
        wait(&s, "AFTER-INTERRUPT");
    }
    #[test]
    fn drop_hangs_up_foreground_job_and_releases_workers() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("child.pid");
        let s = Session::start(dir.path(), "/bin/sh").unwrap();
        s.send(b"sh -c 'echo $$ > child.pid; exec sleep 30'\r".to_vec())
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !pidfile.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let pid: u32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let weak = Arc::downgrade(&s.state);
        drop(s);
        while std::time::Instant::now() < deadline {
            let dead = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|v| {
                    v.split(')')
                        .nth(1)
                        .unwrap_or("")
                        .trim_start()
                        .starts_with('Z')
                })
                .unwrap_or(true);
            if dead && weak.upgrade().is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("terminal foreground job or workers survived session drop");
    }
    #[test]
    fn bounded_paste_and_exit_status() {
        let s = Session::start(Path::new("/tmp"), "/bin/sh").unwrap();
        assert!(s.send(vec![b'x'; 65537]).is_err());
        s.send(b"exit 7\r".to_vec()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(status) = s.state.lock().unwrap().exit.clone() {
                assert!(status.contains('7'), "{status}");
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("shell exit status was not recorded");
    }
    #[test]
    fn session_drop_reaps_shell() {
        let s = Session::start(Path::new("/tmp"), "/bin/sh").unwrap();
        let pid = s.pid.unwrap();
        let weak = Arc::downgrade(&s.state);
        drop(s);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while Path::new(&format!("/proc/{pid}")).exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        while weak.upgrade().is_some() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            weak.upgrade().is_none(),
            "terminal workers retained session state"
        );
    }
}
