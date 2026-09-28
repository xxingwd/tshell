#![cfg_attr(not(test), windows_subsystem = "windows")]
extern crate gpui_kit as gpui;
#[cfg(windows)]
mod app_identity;
mod appearance;
mod backend;
mod i18n;
mod input;
mod shortcuts;
mod ssh_pool;
mod terminal;
mod terminal_clipboard;
mod terminal_decorations;
mod terminal_glyphs;
mod terminal_metrics;
mod terminal_notifications;
mod terminal_protocol;
mod terminal_render;
mod terminal_snapshot;
mod terminal_theme;
mod terminal_view;
mod tmux;
mod tmux_client;
mod tmux_titles;
#[cfg(windows)]
mod update;
#[cfg(all(windows, target_arch = "x86_64"))]
mod windows_runtime;
mod workspace;
#[cfg(test)]
mod xterm_compat_tests;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::{prelude::*, *};

rust_i18n::i18n!("locales", fallback = "en");

/// Translate a UI string; see [`i18n`] for the locale files.
///
/// Re-exported at the crate root so every module can reach it as `crate::t!`.
pub(crate) use rust_i18n::t;

fn main() {
    #[cfg(debug_assertions)]
    if let Some(output) = std::env::args()
        .skip_while(|arg| arg != "--workspace-ui-check")
        .nth(1)
    {
        let passed = workspace::run_ui_check(std::path::PathBuf::from(output));
        std::process::exit(if passed { 0 } else { 1 });
    }
    #[cfg(debug_assertions)]
    if let Some(output) = std::env::args()
        .skip_while(|arg| arg != "--terminal-render-check")
        .nth(1)
    {
        let passed = terminal_view::run_render_check(std::path::PathBuf::from(output));
        std::process::exit(if passed { 0 } else { 1 });
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    #[cfg(windows)]
    let _update_lease = update::startup();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            terminal_view::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let window_size = workspace::saved_window_size().unwrap_or_else(|| {
                size(
                    px(workspace::DEFAULT_WINDOW_SIZE[0]),
                    px(workspace::DEFAULT_WINDOW_SIZE[1]),
                )
            });
            let bounds = Bounds::centered(None, window_size, cx);
            cx.spawn(async move |cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("TShell".into()),
                            ..TitleBar::title_bar_options()
                        }),
                        window_min_size: Some(size(px(860.), px(520.))),
                        ..TitleBar::window_options()
                    },
                    |window, cx| {
                        #[cfg(debug_assertions)]
                        if std::env::var_os("TSHELL_DEBUG_FRAMES").is_some() {
                            window.set_debug_frame_overlay_mode(DebugFrameOverlayMode::Full);
                        }
                        let app = cx.new(|cx| workspace::AppView::new(window, cx));
                        cx.new(|cx| Root::new(app, window, cx).bg(transparent_black()))
                    },
                )
                .unwrap_or_else(|_| panic!("{}", crate::t!("app.window_failed")));
                cx.update(|cx| cx.activate(true));
            })
            .detach();
        });
}
