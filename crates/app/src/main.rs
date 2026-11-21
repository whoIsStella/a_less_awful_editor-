mod cli;
#[cfg(unix)]
mod reuse;

use ale_ui::{
    AppShell, Close, Find, FindNext, FindPrevious, FindWorkspace, FocusEditor, FocusTerminal,
    GoToLine, InspectBinary, NewDocument, NextDocument, Open, OpenFolder, OpenMenu, QuickOpen,
    Quit, Save, SaveAs, ShowAssembly, ShowBytes, ShowEditor, ShowOverview, TogglePanels,
};
use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowOptions, px, size,
};

fn main() {
    let mut args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let reuse_window = args.first().is_some_and(|arg| arg == "--reuse-window");
    if reuse_window {
        args.remove(0);
    }
    let target = match cli::parse(args) {
        Ok(cli::Launch::Help) => {
            print!("{}", cli::HELP);
            return;
        }
        Ok(cli::Launch::Empty) => None,
        Ok(cli::Launch::Open(target)) => Some(cli::Launch::Open(target)),
        Ok(cli::Launch::OpenText {
            paths,
            line,
            column,
        }) => Some(cli::Launch::OpenText {
            paths,
            line,
            column,
        }),
        Err(error) => {
            eprintln!("a-less-awful-editor: {error}");
            std::process::exit(2);
        }
    };
    #[cfg(unix)]
    if reuse_window {
        let request = match target {
            Some(cli::Launch::Open(ale_ui::OpenTarget::Text(path))) => Some((vec![path], 1, 1)),
            Some(cli::Launch::OpenText {
                paths,
                line,
                column,
            }) => Some((paths, line, column)),
            _ => None,
        };
        let result = request
            .ok_or_else(|| "--reuse-window requires text paths.".to_string())
            .and_then(|(paths, line, column)| reuse::send(paths, line, column));
        if let Err(error) = result {
            eprintln!("a-less-awful-editor: {error}");
            std::process::exit(2);
        }
        return;
    }
    #[cfg(not(unix))]
    if reuse_window {
        eprintln!("a-less-awful-editor: --reuse-window is unavailable on this platform.");
        std::process::exit(2);
    }
    #[cfg(unix)]
    let server = reuse::start().ok();
    Application::new().run(move |cx: &mut App| {
        #[cfg(unix)]
        let (server_guard, requests) = match server {
            Some((guard, requests)) => (Some(guard), Some(requests)),
            None => (None, None),
        };
        ale_ui::prompts::init(cx);
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.bind_keys([
            KeyBinding::new(&format!("{modifier}-p"), QuickOpen, None),
            KeyBinding::new(&format!("{modifier}-n"), NewDocument, None),
            KeyBinding::new(&format!("{modifier}-f"), Find, None),
            KeyBinding::new(&format!("{modifier}-shift-f"), FindWorkspace, None),
            KeyBinding::new(&format!("{modifier}-g"), GoToLine, None),
            KeyBinding::new("f3", FindNext, None),
            KeyBinding::new("shift-f3", FindPrevious, None),
            KeyBinding::new(&format!("{modifier}-alt-e"), FocusEditor, None),
            KeyBinding::new(&format!("{modifier}-alt-t"), FocusTerminal, None),
            KeyBinding::new(&format!("{modifier}-tab"), NextDocument, None),
            KeyBinding::new("f10", OpenMenu, None),
            KeyBinding::new(&format!("{modifier}-o"), Open, None),
            KeyBinding::new(&format!("{modifier}-shift-o"), InspectBinary, None),
            KeyBinding::new(&format!("{modifier}-1"), ShowEditor, None),
            KeyBinding::new(&format!("{modifier}-2"), ShowOverview, None),
            KeyBinding::new(&format!("{modifier}-3"), ShowAssembly, None),
            KeyBinding::new(&format!("{modifier}-4"), ShowBytes, None),
            KeyBinding::new(&format!("{modifier}-b"), TogglePanels, None),
            KeyBinding::new(&format!("{modifier}-s"), Save, None),
            KeyBinding::new(&format!("{modifier}-shift-s"), SaveAs, None),
            KeyBinding::new(&format!("{modifier}-w"), Close, None),
            KeyBinding::new(&format!("{modifier}-q"), Quit, None),
        ]);
        ale_ui::init_terminal_keymap(cx);
        cx.set_menus(vec![Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New File", NewDocument),
                MenuItem::action("Open", Open),
                MenuItem::action("Quick Open", QuickOpen),
                MenuItem::action("Open Folder", OpenFolder),
                MenuItem::action("Inspect binary", InspectBinary),
                MenuItem::action("Save", Save),
                MenuItem::action("Save As", SaveAs),
                MenuItem::action("Close", Close),
                MenuItem::action("Quit", Quit),
            ],
        }]);
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("A Less Awful Editor - Untitled".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let mut shell = AppShell::new(window, cx);
                    #[cfg(unix)]
                    if let (Some(guard), Some(requests)) = (server_guard, requests) {
                        shell.accept_open_requests(requests, Box::new(guard), window, cx);
                    }
                    match target {
                        Some(cli::Launch::Open(target)) => shell.open_target(target, window, cx),
                        Some(cli::Launch::OpenText {
                            paths,
                            line,
                            column,
                        }) => shell.open_text_targets(paths, line, column, window, cx),
                        _ => {}
                    }
                    shell
                })
            },
        )
        .expect("failed to open the editor window");
        cx.activate(true);
    });
}
