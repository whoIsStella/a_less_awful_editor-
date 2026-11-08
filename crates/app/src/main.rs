mod cli;

use ale_ui::{
    AppShell, Close, Find, FindNext, FindPrevious, FocusEditor, FocusTerminal, GoToLine,
    InspectBinary, NewDocument, NextDocument, Open, OpenFolder, OpenMenu, Quit, Save, SaveAs,
    ShowAssembly, ShowBytes, ShowEditor, ShowOverview, TogglePanels,
};
use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowOptions, px, size,
};

fn main() {
    let target = match cli::parse(std::env::args_os().skip(1)) {
        Ok(cli::Launch::Help) => {
            print!("{}", cli::HELP);
            return;
        }
        Ok(cli::Launch::Empty) => None,
        Ok(cli::Launch::Open(target)) => Some(target),
        Err(error) => {
            eprintln!("a-less-awful-editor: {error}");
            std::process::exit(2);
        }
    };
    Application::new().run(move |cx: &mut App| {
        ale_ui::prompts::init(cx);
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.bind_keys([
            KeyBinding::new(&format!("{modifier}-n"), NewDocument, None),
            KeyBinding::new(&format!("{modifier}-f"), Find, None),
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
                    if let Some(target) = target {
                        shell.open_target(target, window, cx);
                    }
                    shell
                })
            },
        )
        .expect("failed to open the editor window");
        cx.activate(true);
    });
}
