use ale_ui::AppShell;
use gpui::{App, Bounds, WindowBounds, WindowOptions, px, size};
use gpui_platform::application;

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|_| AppShell::new()),
        )
        .expect("failed to open the editor window");

        cx.activate(true);
    });
}
