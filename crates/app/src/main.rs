use ale_ui::{AppShell, Close, Open, Quit, Save, SaveAs};
use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowOptions, px, size,
};

fn main() {
    Application::new().run(|cx: &mut App| {
        ale_ui::prompts::init(cx);
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.bind_keys([
            KeyBinding::new(&format!("{modifier}-o"), Open, None),
            KeyBinding::new(&format!("{modifier}-s"), Save, None),
            KeyBinding::new(&format!("{modifier}-shift-s"), SaveAs, None),
            KeyBinding::new(&format!("{modifier}-w"), Close, None),
            KeyBinding::new(&format!("{modifier}-q"), Quit, None),
        ]);
        cx.set_menus(vec![Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Open", Open),
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
            |window, cx| cx.new(|cx| AppShell::new(window, cx)),
        )
        .expect("failed to open the editor window");
        cx.activate(true);
    });
}
