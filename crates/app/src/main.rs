use ale_ui::AppShell;
use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("A Less Awful Editor — Scratch".into()),
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
