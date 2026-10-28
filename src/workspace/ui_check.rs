use super::*;
use crate::appearance::PaletteTerminalTheme;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;
use tshell_explorer::{SessionFileState, preview, workbench};

fn check_equal_split_bounds(app: &AppView) -> anyhow::Result<()> {
    let host = &app.hosts[0];
    let window = host.snapshot.window().unwrap();
    let Some(Backend::Local(backend)) = &host.backend else {
        anyhow::bail!("split check requires local backend");
    };
    let viewport = host.viewport.as_ref().unwrap().1;
    let panes = backend.pixel_panes(window, viewport);
    let min_width = panes
        .iter()
        .map(|pane| pane.width)
        .fold(f32::INFINITY, f32::min);
    let max_width = panes.iter().map(|pane| pane.width).fold(0., f32::max);
    anyhow::ensure!(max_width - min_width <= 1., "unequal columns: {panes:?}");
    for pane in &panes {
        let rows: Vec<_> = panes.iter().filter(|row| row.x == pane.x).collect();
        let min_height = rows
            .iter()
            .map(|row| row.height)
            .fold(f32::INFINITY, f32::min);
        let max_height = rows.iter().map(|row| row.height).fold(0., f32::max);
        anyhow::ensure!(max_height - min_height <= 1., "unequal rows: {rows:?}");
    }
    Ok(())
}

fn check_transient_layout_sizes(
    app: &mut AppView,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> anyhow::Result<()> {
    let candidates = std::mem::take(&mut app.directory_candidates);
    let directory_error = app.directory_error.take();
    let message = app.message.take();
    let theme_error = app.theme_error.take();
    let metrics = app.metrics.take();
    let metrics_target = app.metrics_target.clone();
    app.metrics_target = Some(host_metrics::Target::Local);
    let result = (|| {
        for width in [400., 560.] {
            let available = size(
                AvailableSpace::Definite(px(width)),
                AvailableSpace::MinContent,
            );
            let baseline = app
                .new_session_editor(window, cx)
                .layout_as_root(available, window, cx);
            anyhow::ensure!(
                baseline.height > px(150.),
                "directory fixture did not measure its form"
            );
            for count in [3, 15, 40, 0] {
                app.directory_candidates = (0..count)
                    .map(|index| format!("/fixture/directory-{index}"))
                    .collect();
                app.directory_error = Some("Fixture directory lookup failed. ".repeat(12));
                anyhow::ensure!(
                    app.new_session_editor(window, cx)
                        .layout_as_root(available, window, cx)
                        == baseline,
                    "directory results or validation changed form size at {width}px"
                );
            }
            app.directory_candidates.clear();
            app.directory_error = None;
            let baseline = app
                .scheme_settings(cx)
                .layout_as_root(available, window, cx);
            app.theme_error = Some("Fixture theme could not be saved. ".repeat(12));
            let failed = app
                .scheme_settings(cx)
                .layout_as_root(available, window, cx);
            anyhow::ensure!(
                failed == baseline,
                "theme-list feedback changed its size at {width}px: {baseline:?} -> {failed:?}"
            );
            app.theme_error = None;
        }
        let available = size(
            AvailableSpace::MinContent,
            AvailableSpace::Definite(px(CHROME_BAR_HEIGHT)),
        );
        for side in [metrics_config::Side::Left, metrics_config::Side::Right] {
            app.metrics = None;
            let baseline = app
                .terminal_status_strip(&app.hosts[app.active].snapshot, side)
                .layout_as_root(available, window, cx);
            let identities = app
                .terminal_status_items(&app.hosts[app.active].snapshot, side)
                .into_iter()
                .map(|(metric, _, _)| metric)
                .collect::<Vec<_>>();
            for state in [
                Some(Err("Fixture metrics request failed".to_owned())),
                metrics.clone(),
                None,
            ] {
                app.metrics = state;
                anyhow::ensure!(
                    app.terminal_status_strip(&app.hosts[app.active].snapshot, side)
                        .layout_as_root(available, window, cx)
                        .height
                        == baseline.height,
                    "metrics response changed status-strip height"
                );
                anyhow::ensure!(
                    app.terminal_status_items(&app.hosts[app.active].snapshot, side)
                        .into_iter()
                        .map(|(metric, _, _)| metric)
                        .collect::<Vec<_>>()
                        == identities,
                    "metrics response inserted or removed status items"
                );
            }
        }
        Ok(())
    })();
    app.directory_candidates = candidates;
    app.directory_error = directory_error;
    app.message = message;
    app.theme_error = theme_error;
    app.metrics = metrics;
    app.metrics_target = metrics_target;
    result
}

struct FeedbackLayoutProbe {
    owner: Entity<AppView>,
    result: std::rc::Rc<std::cell::RefCell<Option<anyhow::Result<()>>>>,
}

impl Render for FeedbackLayoutProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let owner = self.owner.clone();
        let result = self.result.clone();
        canvas(
            move |_, window, cx| {
                if result.borrow().is_none() {
                    let measured =
                        owner.update(cx, |app, cx| check_transient_layout_sizes(app, window, cx));
                    *result.borrow_mut() = Some(measured);
                }
            },
            |_, _, _, _| {},
        )
        .size_full()
    }
}

fn check_session_switch_layout(
    handle: WindowHandle<Root>,
    app: &Entity<AppView>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    // Select and draw in one UI callback so no settling frame can hide a shift.
    let draw_bounds = |action: Option<Action>, cx: &mut AsyncApp| {
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                if let Some(action) = action {
                    app.act(action, cx);
                } else {
                    app.sync(cx);
                }
            });
            window.refresh();
            window.draw(cx).clear(cx);
            let app = app.read(cx);
            let host = &app.hosts[app.active];
            host.snapshot
                .window()
                .unwrap()
                .panes
                .iter()
                .map(|pane| host.views[&pane.id].read(cx).rendered_bounds())
                .collect::<Vec<_>>()
        })
    };
    let original = app.read_with(cx, |app, _cx| app.hosts[0].snapshot.active_session.clone());
    let baseline = draw_bounds(None, cx)?;
    let original_message = app.read_with(cx, |app, _| app.message.clone());
    for message in [
        Some("Fixture workspace failure that must not resize the terminal.".to_owned()),
        None,
    ] {
        app.update(cx, |app, _| app.message = message);
        anyhow::ensure!(
            draw_bounds(None, cx)? == baseline,
            "workspace feedback changed terminal geometry"
        );
    }
    app.update(cx, |app, _| app.message = original_message);
    let first = draw_bounds(
        Some(Action::NewNamedSession {
            name: "session-layout-check".into(),
            path: std::env::temp_dir().to_string_lossy().into_owned(),
        }),
        cx,
    )?;
    anyhow::ensure!(
        first == baseline,
        "new session shifted on its first frame: {first:?} != {baseline:?}"
    );
    let other = app.read_with(cx, |app, _cx| app.hosts[0].snapshot.active_session.clone());
    draw_bounds(Some(Action::Split(SplitAxis::Horizontal)), cx)?;
    draw_bounds(Some(Action::Split(SplitAxis::Vertical)), cx)?;
    let grouped = draw_bounds(None, cx)?;
    let first_tab = app.read_with(cx, |app, _cx| {
        app.hosts[0].snapshot.window().unwrap().id.clone()
    });
    let first = draw_bounds(Some(Action::NewWindow), cx)?;
    anyhow::ensure!(first == baseline, "new tab shifted on its first frame");
    let second_tab = app.read_with(cx, |app, _cx| {
        app.hosts[0].snapshot.window().unwrap().id.clone()
    });
    for reset_viewport in [false, true] {
        for (action, expected) in [
            (Action::SelectWindow(first_tab.clone()), &grouped),
            (Action::SelectSession(original.clone()), &baseline),
            (Action::SelectSession(other.clone()), &grouped),
            (Action::SelectWindow(second_tab.clone()), &baseline),
        ] {
            if reset_viewport {
                app.update(cx, |app, _cx| app.hosts[0].viewport = None);
            }
            let first = draw_bounds(Some(action), cx)?;
            let settled = draw_bounds(None, cx)?;
            anyhow::ensure!(
                first == *expected && first == settled,
                "terminal bounds changed after switching: first={first:?}, settled={settled:?}, expected={expected:?}, reset={reset_viewport}"
            );
        }
        draw_bounds(Some(Action::SelectWindow(first_tab.clone())), cx)?;
    }
    draw_bounds(Some(Action::SelectSession(original)), cx)?;
    draw_bounds(Some(Action::RemoveSession(other)), cx)?;
    Ok(())
}

/// Exercise the real overlay draw/focus lifecycle in an isolated workspace.
pub(crate) fn run_ui_check(output: PathBuf) -> bool {
    if std::env::var_os("TSHELL_DATA_DIR").is_none() {
        return false;
    }
    let passed = Arc::new(AtomicBool::new(false));
    let result = passed.clone();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            crate::terminal_view::init(cx);
            cx.spawn(async move |cx| {
                let report = check(cx).await;
                let preview_scope = std::env::var("TSHELL_UI_CHECK_SCOPE").ok();
                let html_only = matches!(preview_scope.as_deref(), Some("html-preview" | "common-previews"));
                let json = match report {
                    Ok(()) if html_only => {
                        result.store(true, Ordering::Release);
                        serde_json::json!({"passed": true, "clipboard_checked": false, "remote_files_checked": false, "checked": if preview_scope.as_deref() == Some("common-previews") { vec!["HTML and PDF navigation through restricted local protocols", "SVG image decode and offline Mermaid flowchart/sequence rendering", "malformed SVG and Mermaid show recoverable errors", "source editing, file switches and hidden terminal views"] } else { vec!["index.html opens through native frame events without reentrant GPUI borrowing", "HTML preview preserves source and hides over the terminal"] }})
                    }
                    Ok(()) => {
                        result.store(true, Ordering::Release);
                        let session_only = std::env::var("TSHELL_UI_CHECK_SCOPE").as_deref() == Ok("session-submission");
                        let navigation_only = std::env::var("TSHELL_UI_CHECK_SCOPE").as_deref() == Ok("navigation-order");
                        serde_json::json!({"passed": true, "clipboard_checked": !session_only && !navigation_only && std::env::var_os("TSHELL_UI_CHECK_SKIP_CLIPBOARD").is_none(), "remote_files_checked": !session_only && !navigation_only && std::env::var_os("TSHELL_REMOTE_UI_ROOT").is_some(), "checked": if navigation_only { vec!["host order persistence without changing active host", "native session dragging in both directions", "session refresh restores persisted drag order", "session move action", "deleted session and host order cleanup", "disconnected host preserves session order"] } else if session_only { vec!["remote session Enter and confirm", "duplicate submission", "missing directory", "edit, close and host-switch cancellation"] } else { vec![
            "host order persistence without changing active host", "native session dragging and persisted refresh order", "deleted session and host order cleanup", "disconnected host preserves session order", "event-driven title wakes workspace without polling", "terminal Tab input retains focus", "Ctrl+B toggles sidebar with terminal focus", "sidebar viewport and persisted state", "Zen preserves sidebar preference", "session and tab first-frame terminal bounds stay stable", "single and grouped terminal sidebar rendering", "equal split shortcuts and close redistribution", "host and key settings pages", "standalone host validation",
                            "terminal viewport unchanged", "settings restores terminal focus",
            "command palette focus and dispatch", "GPUI frame overlay and status item defaults", "Git mode", "file tree", "syntax editor", "status bar and notifications", "unread terminal notice acknowledgement",
                            "settings dialog and pages", "component settings pages render after scrolling", "resource order, visibility, persistence and stop", "font preference", "ligature preference", "2 file-backed terminal themes", "theme selection", "theme persistence", "theme.json create, edit, delete and invalid-file fallback", "unified interface appearance", "opacity preference",
            "shortcut recording and cancellation", "Escape removes overlay", "host rename dialog",
            "language selector", "session deletion persists and preserves other sessions", "host disconnect preserves configuration and other hosts",
            "new tab has no dialog", "session file state", "Markdown and HTML preview", "session directory and optional name", "session groups with compact tab separators", "named session creation and previous tab restoration",
            "SSH forwarding dialog dark and narrow light layout", "forwarding dialog X closes after cancelling or saving Add", "transfer panel dark and light layout", "transfer header drag and viewport constraints", "transfer launcher collapse and expansion", "individual transfer cancellation and clearing", "transfer hover retention and automatic collapse",
            "Git raw Unicode/tab selection in both layouts", "Git word and line selection", "Git off-screen selection anchor", "terminal search focus, navigation and live output", "terminal search dark and narrow light layouts", "Ctrl-click links and unchanged ordinary TUI mouse reports", "terminal file link line/column and draft preservation", "terminal directory link Explorer root", "notification pane targeting and closed identity", "Docker separate image aliases, tagged-first ordering, untagged tail and complete IDs", "Docker resource tables, responsive adjacent previews, keyboard selection/expand/close/return and per-host restoration", "Docker functional sidebar menu, 10k containers/images, YAML/JSON/output editors in dark/light narrow layouts", "Docker follows visible/hidden hosts, preserves resource selection and dirty drafts, rejects stale previews and works without sessions"
                        ] }})
                    }
                    Err(error) => {
                        serde_json::json!({"passed": false, "error": crate::i18n::format_error(&error)})
                    }
                };
                if std::fs::write(output, serde_json::to_vec_pretty(&json).unwrap()).is_err() {
                    result.store(false, Ordering::Release);
                }
                cx.update(|cx| {
                    for handle in cx.windows() {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                    cx.quit();
                });
            })
            .detach();
        });
    passed.load(Ordering::Acquire)
}

async fn check(cx: &mut AsyncApp) -> anyhow::Result<()> {
    let handle = cx.open_window(
        WindowOptions {
            show: std::env::var_os("TSHELL_UI_CHECK_VISIBLE").is_some(),
            focus: false,
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(1320.), px(840.)),
            ))),
            ..Default::default()
        },
        |window, cx| {
            let app = cx.new(|cx| AppView::new(window, cx));
            cx.new(|cx| {
                Root::new(app, window, cx)
                    .w(px(1320.))
                    .h(px(840.))
                    .bg(transparent_black())
            })
        },
    )?;
    let app = handle.update(cx, |root, _, _| {
        root.view().clone().downcast::<AppView>().unwrap()
    })?;
    if std::env::var("TSHELL_UI_CHECK_SCOPE").as_deref() == Ok("html-preview") {
        return check_html_preview(handle, &app, cx).await;
    }
    if std::env::var("TSHELL_UI_CHECK_SCOPE").as_deref() == Ok("common-previews") {
        return check_common_previews(handle, &app, cx).await;
    }
    if std::env::var("TSHELL_UI_CHECK_SCOPE").as_deref() == Ok("session-submission") {
        let destination = std::env::var("TSHELL_SSH_TEST_HOST")?;
        return ssh_ui_check::check_session_submission(handle, &app, destination, cx).await;
    }
    navigation_order::check(app.clone(), cx).await?;
    if std::env::var("TSHELL_UI_CHECK_SCOPE").as_deref() == Ok("navigation-order") {
        return Ok(());
    }
    anyhow::ensure!(
        app.read_with(cx, |app, _cx| app.sidebar_opacity) == 1.,
        "sidebar did not default to opaque"
    );
    anyhow::ensure!(
        app.read_with(cx, |app, _cx| app.settings_ui.page) == 0,
        "appearance is not the initial settings page"
    );
    app.read_with(cx, |app, _cx| {
        anyhow::ensure!(
            app.background_appearance() == WindowBackgroundAppearance::Opaque,
            "opaque sidebar enabled the glass backdrop"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    let draw = |cx: &mut AsyncApp| -> anyhow::Result<()> {
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })?;
        Ok(())
    };
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.check_initialize_tools(window, cx))
    })?;
    let measured = std::rc::Rc::new(std::cell::RefCell::new(None));
    let probe = cx.open_window(
        WindowOptions {
            show: false,
            focus: false,
            ..Default::default()
        },
        |_, cx| {
            cx.new(|_| FeedbackLayoutProbe {
                owner: app.clone(),
                result: measured.clone(),
            })
        },
    )?;
    cx.update_window(probe.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
        window.remove_window();
    })?;
    measured
        .borrow_mut()
        .take()
        .ok_or_else(|| anyhow::anyhow!("feedback layout probe did not draw"))??;
    app.update(cx, |app, cx| {
        let session = app.hosts[app.active].snapshot.session().unwrap().clone();
        let before = app.hosts[app.active].snapshot.clone();
        app.explorer_entity()
            .unwrap()
            .update(cx, |explorer, _| explorer.editor_dirty = true);
        assert!(
            app.closes_session_with_unsaved_files(&Action::RemoveSession(session.id.clone()), cx)
        );
        if session.windows.len() == 1 {
            assert!(app.closes_session_with_unsaved_files(&Action::CloseWindow, cx));
            app.act(Action::CloseWindow, cx);
            assert_eq!(app.hosts[app.active].snapshot, before);
            if session.windows[0].panes.len() == 1 {
                assert!(app.closes_session_with_unsaved_files(&Action::ClosePane, cx));
                app.act(Action::ClosePane, cx);
                assert_eq!(app.hosts[app.active].snapshot, before);
            }
        }
        app.act(Action::RemoveSession(session.id), cx);
        assert_eq!(app.hosts[app.active].snapshot, before);
        app.explorer_entity()
            .unwrap()
            .update(cx, |explorer, _| explorer.editor_dirty = false);
        app.message = None;
        let index = app.hosts.len();
        let config = HostDefinition {
            id: Default::default(),
            ssh: tshell_host::SshConfig {
                destination: "guard.test".into(),
                user: "test".into(),
                port: None,
                identity_file: None,
            },
            name: "Guard test".into(),
            tmux: false,
            socket: None,
        };
        app.hosts.push(Host {
            name: config.name.clone(),
            config: Some(config),
            backend: None,
            snapshot: Snapshot::default(),
            pending: Vec::new(),
            views: BTreeMap::new(),
            read_notices: BTreeMap::new(),
            collapsed_sessions: BTreeSet::new(),
            viewport: None,
        });
        let file_key = (app.hosts[index].identity(), "session".to_string());
        let explorer = app.explorer_entity().unwrap();
        explorer.update(cx, |explorer, _| explorer.editor_dirty = true);
        app.workspaces.insert(
            file_key.clone(),
            SessionWorkspace {
                explorer: Some(explorer.clone()),
                ..Default::default()
            },
        );
        app.remove_host(index, cx);
        anyhow::ensure!(
            app.hosts.len() == index + 1,
            "unsaved host files were discarded"
        );
        explorer.update(cx, |explorer, _| explorer.editor_dirty = false);
        let generation = app.navigation_request;
        anyhow::ensure!(app.prepare_host_connection_change(index, cx));
        anyhow::ensure!(
            !app.workspaces.contains_key(&file_key) && app.navigation_request == generation,
            "inactive host change cancelled active file requests"
        );
        app.remove_host(index, cx);
        anyhow::ensure!(app.hosts.len() == index, "saved host was not removed");
        anyhow::ensure!(
            app.navigation_request == generation,
            "inactive host removal cancelled active file requests"
        );
        app.message = None;
        Ok::<_, anyhow::Error>(())
    })?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            assert_eq!(
                window.debug_frame_overlay_mode(),
                DebugFrameOverlayMode::Hidden
            );
            assert!(app.latency_task.is_none());
            app.execute_shortcut(Shortcut::RenderStats, window, cx);
            assert_eq!(
                window.debug_frame_overlay_mode(),
                DebugFrameOverlayMode::Minimal
            );
            assert!(app.latency_task.is_none());
            app.execute_shortcut(Shortcut::RenderStats, window, cx);
            assert_eq!(
                window.debug_frame_overlay_mode(),
                DebugFrameOverlayMode::Full
            );
            app.execute_shortcut(Shortcut::RenderStats, window, cx);
            assert_eq!(
                window.debug_frame_overlay_mode(),
                DebugFrameOverlayMode::Hidden
            );
            assert!(app.latency_task.is_none());
        });
    })?;
    draw(cx)?;
    // Exercise the async wake path without calling AppView::sync manually.
    let screen = app.read_with(cx, |app, _cx| {
        let host = &app.hosts[0];
        host.backend
            .as_ref()
            .unwrap()
            .screen(&host.snapshot.window().unwrap().active_pane)
            .unwrap()
    });
    // Notification metadata wakes the workspace without changing terminal text.
    screen
        .notice_count
        .store(1, std::sync::atomic::Ordering::Release);
    screen.remote_output(b"\x1b]2;Event-driven title\x07");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let ready = app.read_with(cx, |app, _cx| {
            app.hosts[0]
                .snapshot
                .window()
                .unwrap()
                .panes
                .iter()
                .any(|p| p.title == "Event-driven title")
        });
        if ready {
            break;
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "title did not wake the workspace"
        );
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    cx.update_window(handle.into(), |_, _, cx| {
        app.update(cx, |app, cx| {
            let pane = app.hosts[0].snapshot.window().unwrap().panes[0].clone();
            assert_eq!(pane.notice_count, 1);
            assert!(!app.hosts[0].read_notices.contains_key(&pane.id));
            app.mark_terminal_read(&pane.id, cx);
            assert_eq!(app.hosts[0].read_notices.get(&pane.id), Some(&1));
        });
    })?;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        let terminal_focus = app.read_with(cx, |app, cx| {
            let current = app.hosts[app.active]
                .snapshot
                .window()
                .ok_or_else(|| anyhow::anyhow!("terminal window missing"))?;
            let view = app.hosts[app.active]
                .views
                .get(&current.active_pane)
                .ok_or_else(|| anyhow::anyhow!("active terminal view missing"))?;
            anyhow::ensure!(
                view.read(cx).focus.is_focused(window),
                "terminal was not initially focused"
            );
            Ok::<_, anyhow::Error>(view.read(cx).focus.clone())
        })?;
        window.dispatch_keystroke(Keystroke::parse("tab").unwrap(), cx);
        anyhow::ensure!(
            terminal_focus.is_focused(window),
            "Tab moved focus away from the terminal"
        );
        anyhow::ensure!(
            !app.read(cx).keyboard_focus,
            "terminal Tab enabled interface focus styling"
        );
        Ok::<_, anyhow::Error>(())
    })??;
    let viewport = app.read_with(cx, |app, _cx| app.hosts[0].viewport.clone());
    for collapsed in [true, false] {
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse("ctrl-b").unwrap(), cx);
            app.read_with(cx, |app, _cx| {
                anyhow::ensure!(
                    app.sidebar_collapsed == collapsed,
                    "Ctrl+B did not toggle sidebar"
                );
                anyhow::ensure!(
                    Preferences::load().sidebar_collapsed == collapsed,
                    "sidebar preference not saved"
                );
                anyhow::ensure!(!app.zen, "sidebar shortcut changed Zen mode");
                Ok::<_, anyhow::Error>(())
            })
        })??;
        draw(cx)?;
        cx.update_window(handle.into(), |_, window, cx| {
            app.read_with(cx, |app, cx| {
                let original = viewport.as_ref().unwrap().1;
                let resized = app.hosts[0].viewport.as_ref().unwrap().1;
                let extra = if collapsed { app.sidebar_width } else { 0. };
                anyhow::ensure!(
                    (resized.width - original.width - extra).abs() < 1.,
                    "sidebar did not resize terminal viewport: {original:?} -> {resized:?}, collapsed={collapsed}"
                );
                anyhow::ensure!(
                    resized.height == original.height,
                    "sidebar changed terminal height"
                );
                let active = &app.hosts[0].snapshot.window().unwrap().active_pane;
                anyhow::ensure!(
                    app.hosts[0].views[active].read(cx).focus.is_focused(window),
                    "sidebar toggle lost terminal focus"
                );
                Ok::<_, anyhow::Error>(())
            })?;
            for _ in 0..2 {
                window.dispatch_keystroke(Keystroke::parse("ctrl-shift-z").unwrap(), cx);
            }
            app.read_with(cx, |app, _cx| {
                anyhow::ensure!(
                    !app.zen && app.sidebar_collapsed == collapsed,
                    "Zen changed sidebar preference"
                );
                Ok::<_, anyhow::Error>(())
            })
        })??;
        draw(cx)?;
    }
    check_session_switch_layout(handle, &app, cx)?;
    cx.update_window(handle.into(), |_, _, cx| {
        app.update(cx, |app, cx| {
            app.act(Action::Split(SplitAxis::Horizontal), cx)
        });
    })?;
    draw(cx)?;
    app.read_with(cx, |app, _cx| {
        anyhow::ensure!(
            app.hosts[0].snapshot.window().unwrap().panes.len() == 2,
            "split terminal did not enter grouped sidebar state"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    for key in ["alt-shift-n", "alt-shift-n", "alt-n"] {
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
        })?;
        draw(cx)?;
        app.read_with(cx, |app, _cx| check_equal_split_bounds(app))?;
    }
    app.read_with(cx, |app, _cx| {
        let window = app.hosts[0].snapshot.window().unwrap();
        anyhow::ensure!(window.panes.len() == 5, "split shortcuts lost a pane");
        let columns: BTreeSet<_> = window.panes.iter().map(|pane| pane.x).collect();
        anyhow::ensure!(columns.len() == 3, "new column changed row groups");
        Ok::<_, anyhow::Error>(())
    })?;
    for _ in 0..4 {
        cx.update_window(handle.into(), |_, _, cx| {
            app.update(cx, |app, cx| app.act(Action::ClosePane, cx));
        })?;
        draw(cx)?;
        app.read_with(cx, |app, _cx| check_equal_split_bounds(app))?;
    }
    app.read_with(cx, |app, _cx| {
        anyhow::ensure!(
            app.hosts[0].snapshot.window().unwrap().panes.len() == 1,
            "closing split terminal did not restore flat sidebar state"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    // Render the actual Git workspace, including a long line and virtualized hunks.
    app.update(cx, |app, cx| {
        let change = git::GitChange {
            status: " M".into(),
            path: app.cwd.join("src/a-long-file-name.rs"),
            original: None,
            root: app.cwd.clone(),
            stats: Some(git::Stats {
                added: 200,
                removed: 200,
            }),
        };
        app.page = Page::Git;
        app.git_entity().unwrap().update(cx, |git, _cx| {
            git.git_error = None;
        });
        app.git_entity().unwrap().update(cx, |git, _cx| {
            git.git_changes = (0..10_000)
                .map(|index| {
                    let mut entry = change.clone();
                    entry.path = app.cwd.join(format!("src/file-{index:05}.rs"));
                    entry
                })
                .collect();
        });
        app.git_entity().unwrap().update(cx, |git, cx| {
            git.git_diff = Some(git_view::DiffView {
                change,
                scroll: UniformListScrollHandle::new(),
                horizontal: std::array::from_fn(|_| ScrollHandle::new()),
                selection: std::array::from_fn(|_| git_selection::DiffSelection::new(cx)),
                content: Ok(git_view::DiffContent::new(
                    (0..10_000)
                        .map(|index| git::DiffLine {
                            old: (index % 2 == 0).then_some(index + 1),
                            new: (index % 2 != 0).then_some(index + 1),
                            kind: if index % 2 == 0 {
                                git::LineKind::Removed
                            } else {
                                git::LineKind::Added
                            },
                            text: format!(
                                "{}{}",
                                if index % 2 == 0 { '-' } else { '+' },
                                "content ".repeat(40)
                            ),
                        })
                        .collect(),
                )),
            });
        });
        cx.notify();
    });
    let mut render_times = Vec::new();
    for _ in 0..3 {
        app.update(cx, |_, cx| cx.notify());
        let started = std::time::Instant::now();
        draw(cx)?;
        render_times.push(started.elapsed().as_secs_f64() * 1000.);
    }
    std::fs::write(
        Preferences::path().with_file_name("git-render.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"files": 10_000, "diff_lines": 10_000, "draw_ms": render_times}),
        )?,
    )?;
    let git = app.read_with(cx, |app, _| app.git_entity().unwrap());
    git.update(cx, |git, cx| {
        git.git_scroll.scroll_to_item(4500, ScrollStrategy::Top);
        cx.notify();
    });
    draw(cx)?;
    let list_offset = git.read_with(cx, |git, _| git.git_scroll.0.borrow().base_handle.offset());
    anyhow::ensure!(list_offset.y < px(-1000.), "Git file list did not scroll");
    let git_root = git.update(cx, |git, cx| {
        let root = git.cwd.clone();
        git.cwd = Preferences::path().with_file_name("missing-git-refresh-directory");
        git.refresh_git(cx);
        root
    });
    draw(cx)?;
    anyhow::ensure!(
        git.read_with(cx, |git, _| git.git_scroll.0.borrow().base_handle.offset()) == list_offset,
        "starting Git refresh moved the file list"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while git.read_with(cx, |git, _| git.git_loading) {
        anyhow::ensure!(Instant::now() < deadline, "Git refresh fixture timed out");
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    draw(cx)?;
    anyhow::ensure!(
        git.read_with(cx, |git, _| git.git_error.is_some()
            && git.git_changes.len() == 10_000
            && git.git_scroll.0.borrow().base_handle.offset() == list_offset),
        "failed Git refresh discarded or scrolled the file list"
    );
    git.update(cx, |git, cx| {
        git.cwd = git_root;
        git.git_error = None;
        cx.notify();
    });
    app.update(cx, |app, cx| {
        app.check_git(cx)
            .git_diff
            .as_ref()
            .unwrap()
            .scroll
            .scroll_to_item(4500, ScrollStrategy::Top);
        cx.notify();
    });
    draw(cx)?;
    app.read_with(cx, |app, cx| {
        anyhow::ensure!(
            app.check_git(cx)
                .git_diff
                .as_ref()
                .unwrap()
                .scroll
                .0
                .borrow()
                .base_handle
                .offset()
                .y
                < px(-1000.),
            "diff comparison did not scroll to the requested rows"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    // A hidden Windows window can start with an invalid native client size.
    // Give hit testing a real viewport before dispatching mouse-wheel events.
    cx.update_window(handle.into(), |_, window, _| {
        window.resize(size(px(1320.), px(840.)))
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(25))
        .await;
    // Exercise actual wheel dispatch: vertical input must not drift horizontally,
    // and horizontal input must not change the shared vertical offset.
    for mode in [git_view::DiffMode::Inline, git_view::DiffMode::SideBySide] {
        app.update(cx, |app, cx| app.set_diff_mode(mode, cx));
        draw(cx)?;
        let pane_indices = if mode == git_view::DiffMode::Inline {
            vec![2]
        } else {
            vec![0, 1]
        };
        for pane in pane_indices {
            let viewport =
                cx.update_window(handle.into(), |_, window, _| window.viewport_size())?;
            let position = app.read_with(cx, |app, cx| {
                app.check_git(cx).git_diff.as_ref().unwrap().horizontal[pane]
                    .bounds()
                    .center()
            });
            cx.update_window(handle.into(), |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
            })?;
            draw(cx)?;
            for _ in 0..5 {
                cx.update_window(handle.into(), |_, window, cx| {
                    window.dispatch_event(
                        PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position,
                            delta: ScrollDelta::Lines(point(0., -3.)),
                            ..Default::default()
                        }),
                        cx,
                    );
                })?;
                draw(cx)?;
            }
            let vertical = app.read_with(cx, |app, cx| {
                let diff = app.check_git(cx).git_diff.as_ref().unwrap();
                anyhow::ensure!(diff.horizontal[pane].offset().x == px(0.), "vertical wheel caused horizontal drift");
                let vertical = diff.scroll.0.borrow().base_handle.offset().y;
                anyhow::ensure!(vertical < px(0.), "vertical wheel did not scroll diff: viewport={viewport:?}, pane={pane}, position={position:?}, bounds={:?}, list={:?}, size={:?}", diff.horizontal[pane].bounds(), diff.scroll.0.borrow().base_handle.bounds(), diff.scroll.0.borrow().last_item_size);
                Ok::<_, anyhow::Error>(vertical)
            })?;
            cx.update_window(handle.into(), |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Lines(point(-3., 0.)),
                        ..Default::default()
                    }),
                    cx,
                );
            })?;
            draw(cx)?;
            app.read_with(cx, |app, cx| {
                let diff = app.check_git(cx).git_diff.as_ref().unwrap();
                anyhow::ensure!(
                    diff.horizontal[pane].offset().x < px(0.),
                    "horizontal wheel did not scroll diff"
                );
                anyhow::ensure!(
                    diff.scroll.0.borrow().base_handle.offset().y == vertical,
                    "horizontal wheel changed vertical offset"
                );
                Ok::<_, anyhow::Error>(())
            })?;
        }
    }
    let opened_from_git = Preferences::path().with_file_name("open-from-git.txt");
    std::fs::write(&opened_from_git, "editor content\n")?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.open_path(opened_from_git.clone(), window, cx)
        });
    })?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while app.read_with(cx, |app, cx| {
        app.navigation_loading.is_some() || app.check_explorer(cx).file_loading.is_some()
    }) {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "opening file from Git timed out"
        );
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            anyhow::ensure!(
                app.page == Page::Explorer
                    && app.check_explorer(cx).open_file.as_ref() == Some(&opened_from_git),
                "opening file from Git did not switch to editor"
            );
            app.check_explorer(cx)
                .file_editor
                .clone()
                .update(cx, |editor, cx| {
                    editor.set_value("unsaved edit", window, cx)
                });
            app.explorer_entity().unwrap().update(cx, |explorer, _cx| {
                explorer.editor_dirty = true;
            });
            app.page = Page::Git;
            app.open_path(opened_from_git.clone(), window, cx);
            anyhow::ensure!(
                app.page == Page::Explorer && app.check_explorer(cx).editor_dirty,
                "returning to the same file lost unsaved changes"
            );
            anyhow::ensure!(
                app.check_explorer(cx).file_editor.read(cx).value().as_ref() == "unsaved edit",
                "open file reloaded the dirty editor"
            );
            app.explorer_entity().unwrap().update(cx, |explorer, _cx| {
                explorer.pending_file_state = Some(SessionFileState::default());
            });
            Ok::<_, anyhow::Error>(())
        })
    })??;
    app.update(cx, |app, cx| {
        app.git_entity().unwrap().update(cx, |git, _cx| {
            git.git_diff = None;
        });
        app.git_entity()
            .unwrap()
            .update(cx, |git, _| git.git_changes.clear());
        app.page = Page::Terminal;
        cx.notify();
    });
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.show_command_palette(window, cx));
    })?;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        anyhow::ensure!(window.has_active_dialog(cx), "command palette missing");
        app.update(cx, |app, cx| {
            anyhow::ensure!(app.command_palette, "command palette state missing");
            anyhow::ensure!(
                app.command_state
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window),
                "command search is not focused"
            );
            app.run_palette_target(
                launcher::PaletteTarget::Shortcut(Shortcut::ShowFiles),
                window,
                cx,
            );
            anyhow::ensure!(
                app.page == Page::Explorer,
                "file explorer command did not switch workspace"
            );
            Ok::<_, anyhow::Error>(())
        })?;
        anyhow::ensure!(
            !window.has_active_dialog(cx),
            "command confirmation left its dialog open"
        );
        Ok::<_, anyhow::Error>(())
    })??;
    draw(cx)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while app.read_with(cx, |app, cx| {
        app.navigation_loading.is_some() || app.check_explorer(cx).file_loading.is_some()
    }) {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "tool root lookup timed out"
        );
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
    // Use the tree's expansion event, including the same path used by keyboard reveal.
    app.update(cx, |app, cx| {
        let directory = workbench::path_id(&app.cwd.join("src"));
        assert!(!app.check_explorer(cx).tree_loaded.contains(&directory));
        app.check_explorer(cx)
            .file_tree
            .clone()
            .update(cx, |tree, cx| {
                let index = tree.index_of(&directory.into()).expect("src directory");
                let child = tree.entry(index).unwrap().item().children[0].id.clone();
                tree.reveal_item(&child, gpui::ScrollStrategy::Top, cx);
            });
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !app.read_with(cx, |app, cx| {
        app.check_explorer(cx)
            .tree_loaded
            .contains(&workbench::path_id(&app.cwd.join("src")))
    }) {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "directory expansion did not load children"
        );
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
    app.read_with(cx, |app, cx| {
        let child = workbench::path_id(&app.cwd.join("src").join("workspace.rs"));
        assert!(
            app.check_explorer(cx)
                .file_tree
                .read(cx)
                .index_of(&child.into())
                .is_some()
        );
        assert!(
            !app.check_explorer(cx)
                .tree_loaded
                .contains(&workbench::path_id(&app.cwd.join("src").join("workspace")))
        );
    });
    let explorer = app.read_with(cx, |app, _| app.explorer_entity().unwrap());
    let (selected_file, entry_count) = explorer.update(cx, |explorer, cx| {
        let selected_file = workbench::path_id(&explorer.cwd.join("src").join("workspace.rs"));
        let entry_count = explorer.file_tree.update(cx, |tree, cx| {
            let index = tree.index_of(&selected_file.clone().into()).unwrap();
            tree.set_selected_index(Some(index), cx);
            tree.scroll_to_item(index, ScrollStrategy::Top);
            (0..).map_while(|index| tree.entry(index)).count()
        });
        (selected_file, entry_count)
    });
    draw(cx)?;
    let tree_offset = explorer.read_with(cx, |explorer, cx| {
        explorer
            .file_tree
            .read(cx)
            .scroll_handle()
            .0
            .borrow()
            .base_handle
            .offset()
    });
    explorer.update(cx, |explorer, cx| {
        explorer.load_files(cx);
        let tree = explorer.file_tree.read(cx);
        anyhow::ensure!(
            (0..).map_while(|index| tree.entry(index)).count() == entry_count
                && tree
                    .selected_item()
                    .is_some_and(|item| item.id.as_ref() == selected_file),
            "starting Explorer refresh cleared its rows or selection"
        );
        cx.notify();
        Ok::<_, anyhow::Error>(())
    })?;
    draw(cx)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while explorer.read_with(cx, |explorer, _| explorer.file_loading.is_some()) {
        anyhow::ensure!(
            Instant::now() < deadline,
            "Explorer refresh fixture timed out"
        );
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    draw(cx)?;
    explorer.read_with(cx, |explorer, cx| {
        let tree = explorer.file_tree.read(cx);
        anyhow::ensure!(
            tree.selected_item()
                .is_some_and(|item| item.id.as_ref() == selected_file)
                && tree.scroll_handle().0.borrow().base_handle.offset() == tree_offset,
            "Explorer refresh moved its selection or scroll position"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            let cargo_toml = app.cwd.join("Cargo.toml");
            app.open_path(cargo_toml.clone(), window, cx);
        });
    })?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while app.read_with(cx, |app, cx| {
        app.navigation_loading.is_some() || app.check_explorer(cx).file_loading.is_some()
    }) {
        anyhow::ensure!(std::time::Instant::now() < deadline, "file read timed out");
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            let cargo_toml = app.cwd.join("Cargo.toml");
            anyhow::ensure!(
                app.check_explorer(cx).open_file.as_ref() == Some(&cargo_toml),
                "file did not open"
            );
            anyhow::ensure!(
                app.check_explorer(cx).editor_language == "toml",
                "file language not detected"
            );
            anyhow::ensure!(
                app.check_explorer(cx)
                    .file_editor
                    .read(cx)
                    .value()
                    .contains("[package]"),
                "editor did not receive file contents"
            );
            window.push_notification(Notification::success("UI check"), cx);
            app.check_explorer(cx)
                .file_editor
                .clone()
                .update(cx, |editor, cx| editor.open_search(false, cx));
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            anyhow::ensure!(
                app.check_explorer(cx)
                    .editor_search
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window),
                "editor search focus missing"
            );
            app.check_explorer(cx)
                .file_editor
                .clone()
                .update(cx, |editor, cx| {
                    editor.set_search_query("package", true, cx);
                    assert!(!editor.search_session().matcher.is_empty());
                    editor.close_search(cx);
                });
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.open_path(app.cwd.join("assets/tshell.png"), window, cx)
        });
    })?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while app.read_with(cx, |app, cx| {
        app.navigation_loading.is_some() || app.check_explorer(cx).file_loading.is_some()
    }) {
        anyhow::ensure!(std::time::Instant::now() < deadline, "PNG read timed out");
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
    draw(cx)?;
    app.read_with(cx, |app, cx| {
        anyhow::ensure!(
            app.check_explorer(cx).image_preview.is_some(),
            "PNG preview did not open"
        );
        anyhow::ensure!(
            !app.check_explorer(cx).editor_dirty,
            "PNG preview marked the editor dirty"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.open_path(app.cwd.join("README.md"), window, cx)
        });
    })?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while app.read_with(cx, |app, cx| {
        app.navigation_loading.is_some() || app.check_explorer(cx).file_loading.is_some()
    }) {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "Markdown read timed out"
        );
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
    draw(cx)?;
    app.read_with(cx, |app, cx| {
        anyhow::ensure!(
            app.check_explorer(cx).preview_mode,
            "Markdown preview did not open"
        );
        anyhow::ensure!(
            app.check_explorer(cx)
                .file_preview
                .as_ref()
                .and_then(|document| document.markdown_source())
                .is_some_and(|source| source.starts_with("# TShell")),
            "Markdown preview did not retain the source"
        );
        anyhow::ensure!(
            !app.check_explorer(cx).editor_dirty,
            "Markdown preview marked the editor dirty"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    #[cfg(windows)]
    app.read_with(cx, |app, cx| {
        if let Some(web) = &app.check_explorer(cx).web_preview {
            anyhow::ensure!(!web.is_visible(), "WebView opened for Markdown");
        }
        Ok::<_, anyhow::Error>(())
    })?;
    let html = preview::parse(
        "html",
        "<h1>Safe</h1><script>not visible</script><p>Text &amp; more</p>",
    );
    anyhow::ensure!(
        html.blocks.contains(&preview::Block::Heading {
            level: 1,
            text: "Safe".into(),
        }) && html
            .blocks
            .contains(&preview::Block::Paragraph("Text & more".into()))
            && !html
                .blocks
                .iter()
                .any(|block| format!("{block:?}").contains("not visible")),
        "HTML fallback preview did not filter active content"
    );
    app.update(cx, |app, cx| {
        app.explorer_entity().unwrap().update(cx, |explorer, _cx| {
            explorer.file_preview = Some(html);
        });
        app.explorer_entity().unwrap().update(cx, |explorer, _cx| {
            explorer.editor_language = "html".into();
        });
        cx.notify();
    });
    draw(cx)?;
    #[cfg(windows)]
    if wry::webview_version().is_ok() {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !app.read_with(cx, |app, cx| {
            let explorer = app.check_explorer(cx);
            explorer.web_preview_failed
                || explorer
                    .web_preview
                    .as_ref()
                    .is_some_and(|web| web.is_visible())
        }) {
            anyhow::ensure!(Instant::now() < deadline, "HTML initialization timed out");
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
            draw(cx)?;
        }
    }
    #[cfg(windows)]
    app.read_with(cx, |app, cx| {
        if wry::webview_version().is_ok() {
            anyhow::ensure!(
                app.check_explorer(cx)
                    .web_preview
                    .as_ref()
                    .is_some_and(|web| {
                        web.is_visible() && web.html().contains("<h1>Safe</h1>")
                    }),
                "WebView did not load the HTML preview"
            );
        }
        Ok::<_, anyhow::Error>(())
    })?;
    app.update(cx, |app, cx| {
        app.explorer_entity().unwrap().update(cx, |explorer, _cx| {
            explorer.file_preview = Some(preview::parse("markdown", "# Back to Markdown"));
        });
        app.explorer_entity().unwrap().update(cx, |explorer, _cx| {
            explorer.editor_language = "markdown".into();
        });
        cx.notify();
    });
    draw(cx)?;
    #[cfg(windows)]
    app.read_with(cx, |app, cx| {
        if let Some(web) = &app.check_explorer(cx).web_preview {
            anyhow::ensure!(!web.is_visible(), "WebView stayed visible over Markdown");
        }
        Ok::<_, anyhow::Error>(())
    })?;
    app.update(cx, |app, cx| app.show_terminal(cx));
    draw(cx)?;
    #[cfg(windows)]
    app.read_with(cx, |app, cx| {
        if let Some(web) = &app.check_explorer(cx).web_preview {
            anyhow::ensure!(
                !web.is_visible(),
                "WebView stayed visible over the terminal"
            );
        }
        Ok::<_, anyhow::Error>(())
    })?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.show_add(window, cx));
    })?;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        anyhow::ensure!(window.has_active_dialog(cx), "host dialog missing");
        app.update(cx, |app, cx| {
            anyhow::ensure!(
                !app.settings && app.settings_ui.host_form,
                "host form unexpectedly opened settings"
            );
            anyhow::ensure!(
                app.destination.read(cx).focus_handle(cx).is_focused(window),
                "host input not focused"
            );
            anyhow::ensure!(
                !Theme::global(cx).focus_ring && Theme::global(cx).ring == Theme::global(cx).input,
                "focused host input has an outer ring or a highlighted border"
            );
            app.destination
                .update(cx, |input, cx| input.set_value("-invalid", window, cx));
            app.add_host(window, cx);
            anyhow::ensure!(app.message.is_some(), "invalid host accepted");
            anyhow::ensure!(
                app.host_error_field == Some(HostField::Destination),
                "host error was not attached to the destination field"
            );
            anyhow::ensure!(
                app.destination.read(cx).focus_handle(cx).is_focused(window),
                "invalid destination was not focused"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.destination
                .update(cx, |input, cx| input.set_value("example.test", window, cx));
            app.add_host(window, cx);
            anyhow::ensure!(
                app.message.as_deref() == Some(crate::t!("ws.user_invalid").as_ref()),
                "empty username did not show a validation error"
            );
            anyhow::ensure!(
                window.has_active_dialog(cx),
                "invalid username closed host dialog"
            );
            anyhow::ensure!(
                app.host_error_field == Some(HostField::User),
                "username error was not attached to the user field"
            );
            anyhow::ensure!(
                app.user.read(cx).focus_handle(cx).is_focused(window),
                "invalid username was not focused"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, _| {
        window.resize(size(px(1320.), px(520.)))
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(25))
        .await;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, _| {
        window.resize(size(px(1320.), px(840.)))
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(25))
        .await;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.user
                .update(cx, |input, cx| input.set_value("tester", window, cx));
            app.add_host(window, cx);
            anyhow::ensure!(
                app.hosts.len() == 2 && !app.settings_ui.host_form && !app.settings,
                "host dialog did not save"
            );
            app.remove_host(1, cx);
            app.show_settings(window, cx);
            app.settings_ui.page = 5;
            app.show_add(window, cx);
            anyhow::ensure!(
                app.settings && app.settings_ui.host_form,
                "host dialog did not open over settings"
            );
            app.destination
                .update(cx, |input, cx| input.set_value("draft.test", window, cx));
            app.show_add(window, cx);
            anyhow::ensure!(
                app.destination.read(cx).value().as_ref() == "draft.test",
                "repeated add reset the open host dialog"
            );
            app.keyboard(
                &KeyDownEvent {
                    keystroke: Keystroke::parse("escape").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            anyhow::ensure!(
                app.settings && !app.settings_ui.host_form,
                "Escape did not return to the host list"
            );
            anyhow::ensure!(
                window.has_active_dialog(cx),
                "closing host dialog also closed settings"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    app.update(cx, |app, cx| {
        app.settings_ui.page = 6;
        cx.notify();
    });
    draw(cx)?;
    app.read_with(cx, |app, _cx| {
        anyhow::ensure!(
            app.settings && app.settings_ui.page == 6,
            "key settings missing"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    #[cfg(windows)]
    {
        app.update(cx, |app, cx| {
            app.settings_ui.page = 7;
            cx.notify();
        });
        draw(cx)?;
        app.read_with(cx, |app, cx| {
            anyhow::ensure!(
                matches!(app.updater.state, crate::update::State::Disabled),
                "debug builds must not check for updates"
            );
            anyhow::ensure!(
                !app.update_has_unsaved_files(cx),
                "clean workspace should allow update restart"
            );
            Ok::<_, anyhow::Error>(())
        })?;
    }
    app.read_with(cx, |app, _cx| {
        anyhow::ensure!(
            app.hosts[0].viewport == viewport,
            "settings changed terminal viewport"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    let (reply, answer) = async_channel::bounded(1);
    let (finish, finished) = async_channel::bounded(1);
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.open_auth_prompt(
                crate::ssh_pool::interactive::Prompt {
                    host: "test@example.test:22".into(),
                    kind: crate::ssh_pool::interactive::PromptKind::HostKey {
                        fingerprint: "SHA256:test".into(),
                    },
                    reply,
                    finish,
                    finished,
                },
                window,
                cx,
            )
        });
    })?;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
    })?;
    draw(cx)?;
    anyhow::ensure!(
        answer.try_recv() == Ok(None),
        "SSH dialog did not cancel over settings"
    );
    anyhow::ensure!(
        app.read_with(cx, |app, _cx| app.settings),
        "SSH dialog closed settings"
    );
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.close_modal(window, cx));
    })?;
    draw(cx)?;
    for (kind, key, expected) in [
        (
            crate::ssh_pool::interactive::PromptKind::Secret {
                label: crate::ssh_pool::interactive::PromptLabel::Remote("Test password".into()),
                echo: false,
            },
            "enter",
            Some(String::new()),
        ),
        (
            crate::ssh_pool::interactive::PromptKind::HostKey {
                fingerprint: "SHA256:test".into(),
            },
            "escape",
            None,
        ),
    ] {
        let (reply, answer) = async_channel::bounded(1);
        let (finish, finished) = async_channel::bounded(1);
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.open_auth_prompt(
                    crate::ssh_pool::interactive::Prompt {
                        host: "test@example.test:22".into(),
                        kind,
                        reply,
                        finish,
                        finished,
                    },
                    window,
                    cx,
                )
            });
        })?;
        draw(cx)?;
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
        })?;
        draw(cx)?;
        let received = answer.try_recv();
        anyhow::ensure!(
            received == Ok(expected),
            "SSH dialog did not answer {key}: {received:?}"
        );
    }
    cx.update_window(handle.into(), |_, window, cx| {
        anyhow::ensure!(!window.has_active_dialog(cx), "cancel left an overlay");
        app.update(cx, |app, cx| {
            if let Some(view) = app.hosts[0].views.values().next() {
                anyhow::ensure!(
                    view.read(cx).focus.is_focused(window),
                    "terminal focus not restored"
                );
            }
            app.show_settings(window, cx);
            anyhow::ensure!(
                app.settings && app.page == Page::Terminal,
                "settings changed the workspace mode"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    let menu_position = point(px(620.), px(100.));
    let content_position = point(px(1050.), px(450.));
    cx.update_window(handle.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("tab").unwrap(), cx);
    })?;
    draw(cx)?;
    cx.update_window(handle.into(), |_, _, cx| {
        let app = app.read(cx);
        anyhow::ensure!(
            app.keyboard_focus,
            "settings Tab did not show keyboard focus"
        );
        anyhow::ensure!(
            !Theme::global(cx).focus_ring
                && Theme::global(cx).ring == rgb(app.palette.accent).into(),
            "keyboard focus did not emphasize the border"
        );
        Ok::<_, anyhow::Error>(())
    })??;
    for _ in 0..12 {
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: content_position,
                    delta: ScrollDelta::Lines(point(0., -3.)),
                    ..Default::default()
                }),
                cx,
            );
        })?;
        draw(cx)?;
    }
    cx.update_window(handle.into(), |_, window, cx| {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                position: menu_position,
                click_count: 1,
                ..Default::default()
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                position: menu_position,
                click_count: 1,
                ..Default::default()
            }),
            cx,
        );
    })?;
    draw(cx)?;
    app.update(cx, |app, cx| {
        anyhow::ensure!(
            !app.keyboard_focus,
            "mouse click retained keyboard focus styling"
        );
        anyhow::ensure!(
            Theme::global(cx).ring == Theme::global(cx).input,
            "mouse click retained a highlighted border"
        );
        app.settings_ui.page = 2;
        cx.notify();
        Ok::<_, anyhow::Error>(())
    })?;
    draw(cx)?;
    app.update(cx, |app, cx| {
        app.settings_ui.page = 0;
        cx.notify();
    });
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        anyhow::ensure!(window.has_active_dialog(cx), "settings dialog missing");
        anyhow::ensure!(app.read(cx).settings, "settings state missing");
        app.update(cx, |app, cx| {
            let family = window
                .text_system()
                .all_font_names()
                .into_iter()
                .find(|f| f != &app.font_family)
                .unwrap();
            app.set_font_family(family, cx);
            app.set_ligatures(true, cx);
            app.set_line_height(1.2, cx);
            anyhow::ensure!(
                Preferences::load().line_height_scale.factor() == 1.2,
                "line height was not saved"
            );
            anyhow::ensure!(
                app.hosts
                    .iter()
                    .flat_map(|host| host.views.values())
                    .all(|view| view.read(cx).line_height_scale.factor() == 1.2),
                "line height did not reach existing views"
            );
            app.set_language(crate::i18n::Language::En, cx);
            anyhow::ensure!(
                crate::t!("settings.appearance") == "Appearance",
                "translated label missing after language switch"
            );
            app.set_language(crate::i18n::Language::System, cx);
            app.settings_ui.page = 1;
            let terminal_binding = shortcuts::BINDINGS
                .iter()
                .position(|binding| binding.id == "show_terminal")
                .unwrap();
            app.settings_ui.recording = Some(terminal_binding);
            app.keyboard(
                &KeyDownEvent {
                    keystroke: Keystroke::parse("ctrl-shift-b").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            anyhow::ensure!(
                app.shortcut_map
                    .resolve(&Keystroke::parse("ctrl-1").unwrap())
                    .is_none(),
                "old binding still active"
            );
            anyhow::ensure!(
                app.shortcut_map
                    .resolve(&Keystroke::parse("ctrl-shift-b").unwrap())
                    == Some(Shortcut::ShowTerminal),
                "recording not applied"
            );
            app.settings_ui.recording = Some(terminal_binding);
            app.keyboard(
                &KeyDownEvent {
                    keystroke: Keystroke::parse("escape").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            anyhow::ensure!(
                app.settings && app.settings_ui.recording.is_none(),
                "Escape left settings during recording"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    app.update(cx, |app, cx| {
        app.sidebar_width = 300.;
        app.save(cx);
        assert_eq!(Preferences::load().sidebar_width, Some(300.));
        app.settings_ui.page = 2;
        assert!(app.metrics_config.move_to(
            metrics_config::Metric::System,
            metrics_config::Metric::Memory
        ));
        assert_eq!(
            app.metrics_config.0[3].metric,
            metrics_config::Metric::System
        );
        assert!(app.metrics_config.move_to(
            metrics_config::Metric::Geometry,
            metrics_config::Metric::Memory
        ));
        assert_eq!(
            app.metrics_config.0[2].metric,
            metrics_config::Metric::Geometry
        );
        assert_eq!(app.metrics_config.0[2].side(), metrics_config::Side::Left);
        app.metrics_config.0[1].enabled = false;
        app.save(cx);
        assert_eq!(Preferences::load().metrics_config, app.metrics_config);
        cx.notify();
    });
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            let ssh = app
                .metrics_config
                .0
                .iter_mut()
                .find(|item| item.metric == metrics_config::Metric::SshRtt)
                .unwrap();
            ssh.enabled = true;
            app.sync_latency_monitor(window, cx);
            assert!(app.latency_task.is_some());
            assert_eq!(app.metrics_config.probes(), (true, false));
            app.metrics_config
                .0
                .iter_mut()
                .find(|item| item.metric == metrics_config::Metric::SshRtt)
                .unwrap()
                .enabled = false;
            app.sync_latency_monitor(window, cx);
            assert!(app.latency_task.is_none());
        });
    })?;
    app.update(cx, |app, cx| {
        for item in &mut app.metrics_config.0 {
            item.enabled = false;
        }
        app.sync_metrics(cx);
        assert!(app.metrics_monitor.is_none());
        assert!(
            app.terminal_status_items(&app.hosts[app.active].snapshot, metrics_config::Side::Left)
                .is_empty()
        );
        assert!(
            app.terminal_status_items(&app.hosts[app.active].snapshot, metrics_config::Side::Right)
                .is_empty()
        );
        app.metrics_config = metrics_config::Config::default();
        app.sync_metrics(cx);
        app.save(cx);
    });
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.settings_ui.page = 0;
            app.background_opacity = 0.65;
            app.sidebar_opacity = 0.45;
            app.apply_opacity(window, cx);
            assert_eq!(
                app.background_appearance(),
                WindowBackgroundAppearance::Blurred
            );
            app.toggle_sidebar(window, cx);
            assert_eq!(
                app.background_appearance(),
                WindowBackgroundAppearance::Transparent
            );
            app.toggle_sidebar(window, cx);
            assert_eq!(
                app.background_appearance(),
                WindowBackgroundAppearance::Blurred
            );
            app.appearance = Appearance::Light;
            app.select_theme(ThemeMode::Light, "vscode-dark", window, cx);
            assert_eq!(app.palette.background, app.terminal_palette.terminal);
            for host in &app.hosts {
                for view in host.views.values() {
                    assert!(view.read(cx).ligatures);
                    assert_eq!(
                        view.read(cx).palette.terminal,
                        app.terminal_palette.terminal
                    );
                }
            }
            app.save(cx);
        });
    })?;
    draw(cx)?;
    let theme_ids = app.read_with(cx, |app, _cx| {
        app.themes
            .themes
            .iter()
            .map(|theme| theme.id.clone())
            .collect::<Vec<_>>()
    });
    for id in theme_ids {
        let expected = app.read_with(cx, |app, _cx| app.themes.selected(&id).unwrap().colors());
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.set_appearance(
                    if expected.light() {
                        Appearance::Light
                    } else {
                        Appearance::Dark
                    },
                    window,
                    cx,
                );
                app.select_theme(app.appearance.mode(window.appearance()), &id, window, cx);
            });
        })?;
        draw(cx)?;
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                let slot = if expected.light() {
                    &app.light_theme
                } else {
                    &app.dark_theme
                };
                anyhow::ensure!(slot == &id, "theme selector event was not applied");
                anyhow::ensure!(
                    app.appearance
                        == if expected.light() {
                            Appearance::Light
                        } else {
                            Appearance::Dark
                        },
                    "selecting a scheme changed appearance mode"
                );
                anyhow::ensure!(
                    app.terminal_palette.terminal_theme() == expected,
                    "theme colours not applied"
                );
                anyhow::ensure!(
                    crate::terminal_protocol::default_theme() == expected,
                    "new sessions have stale colours"
                );
                for host in &app.hosts {
                    for view in host.views.values() {
                        anyhow::ensure!(
                            view.read(cx).palette.terminal_theme() == expected,
                            "existing pane has stale colours"
                        );
                    }
                }
                app.set_appearance(app.appearance.next(), window, cx);
                anyhow::ensure!(
                    Preferences::load().appearance == app.appearance,
                    "interface appearance not saved"
                );
                let active_id = active_theme_id(
                    &app.themes,
                    &app.light_theme,
                    &app.dark_theme,
                    app.appearance,
                    window.appearance(),
                );
                let active = app.themes.selected(&active_id).unwrap().colors();
                anyhow::ensure!(
                    app.terminal_palette.terminal_theme() == active,
                    "interface and terminal themes diverged"
                );
                anyhow::ensure!(
                    app.palette.background == active.background,
                    "interface background diverged from terminal"
                );
                anyhow::ensure!(
                    !Theme::global(cx).focus_ring
                        && Theme::global(cx).ring
                            == rgb(if app.keyboard_focus {
                                app.palette.accent
                            } else {
                                app.palette.border
                            })
                            .into(),
                    "theme change lost the active focus presentation"
                );
                anyhow::ensure!(
                    Theme::global(cx).tokens.background.background
                        == rgb(app.palette.background).into(),
                    "dialog background token diverged from theme"
                );
                anyhow::ensure!(
                    Theme::global(cx).tokens.popover.background
                        == rgb(if active.light() {
                            app.palette.background
                        } else {
                            app.palette.panel
                        })
                        .into(),
                    "popover surface diverged from control palette"
                );
                let saved = Preferences::load();
                anyhow::ensure!(
                    saved.light_theme.as_deref() == Some(app.light_theme.as_str())
                        && saved.dark_theme.as_deref() == Some(app.dark_theme.as_str()),
                    "theme slots were not saved"
                );
                Ok::<_, anyhow::Error>(())
            })
        })??;
        draw(cx)?;
    }
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.set_appearance(Appearance::System, window, cx);
            app.select_theme(ThemeMode::Light, "vscode-light", window, cx);
            app.select_theme(ThemeMode::Dark, "vscode-dark", window, cx);
            anyhow::ensure!(
                app.appearance == Appearance::System,
                "selecting a theme disabled automatic mode"
            );
            anyhow::ensure!(
                app.light_theme == "vscode-light" && app.dark_theme == "vscode-dark",
                "theme slots were not retained"
            );
            let active_id = active_theme_id(
                &app.themes,
                &app.light_theme,
                &app.dark_theme,
                app.appearance,
                window.appearance(),
            );
            anyhow::ensure!(
                app.terminal_palette.terminal_theme()
                    == app.themes.selected(&active_id).unwrap().colors(),
                "automatic mode chose the wrong variant"
            );
            let saved = Preferences::load();
            anyhow::ensure!(
                saved.light_theme.as_deref() == Some("vscode-light")
                    && saved.dark_theme.as_deref() == Some("vscode-dark"),
                "theme slots were not persisted"
            );
            app.set_appearance(Appearance::Dark, window, cx);
            anyhow::ensure!(
                app.light_theme == "vscode-light" && app.dark_theme == "vscode-dark",
                "appearance replaced the selected schemes"
            );
            anyhow::ensure!(
                app.terminal_palette.terminal_theme()
                    == app.themes.selected("vscode-dark").unwrap().colors(),
                "dark mode did not select dark scheme"
            );
            app.set_appearance(Appearance::Light, window, cx);
            anyhow::ensure!(
                app.light_theme == "vscode-light" && app.dark_theme == "vscode-dark",
                "appearance replaced the selected schemes"
            );
            anyhow::ensure!(
                app.terminal_palette.terminal_theme()
                    == app.themes.selected("vscode-light").unwrap().colors(),
                "light mode did not select light scheme"
            );
            app.select_theme(ThemeMode::Light, "vscode-light", window, cx);
            anyhow::ensure!(
                app.appearance == Appearance::Light
                    && app.light_theme == "vscode-light"
                    && app.dark_theme == "vscode-dark",
                "changing one slot changed the other"
            );
            anyhow::ensure!(
                app.terminal_palette.terminal_theme()
                    == app.themes.selected("vscode-light").unwrap().colors(),
                "VS Code light variant was not applied"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    let theme_path = AppView::theme_path();
    let original = std::fs::read(&theme_path)?;
    let first_id = app.read_with(cx, |app, _cx| app.themes.themes[0].id.clone());
    let mut edited_file: serde_json::Value = serde_json::from_slice(&original)?;
    edited_file["themes"][0]["ink"] = "#123456".into();
    edited_file["themes"][0]["interface"]["hover"] = "#E0E0E0".into();
    std::fs::write(&theme_path, serde_json::to_vec_pretty(&edited_file)?)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.reload_theme_file(window, cx);
            app.set_appearance(Appearance::Light, window, cx);
            app.select_theme(ThemeMode::Light, &first_id, window, cx);
            anyhow::ensure!(
                app.terminal_palette.text == 0x123456,
                "theme file edit did not apply"
            );
            anyhow::ensure!(
                app.palette.row_hover() == 0xe0e0e0
                    && app.terminal_palette.selection
                        == app.themes.selected(&first_id).unwrap().selection(),
                "interface edit changed terminal selection or failed to apply"
            );
            anyhow::ensure!(
                Preferences::load().light_theme.as_deref() == Some(first_id.as_str()),
                "light theme choice was not saved"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    let active_text = app.read_with(cx, |app, _cx| app.terminal_palette.text);
    std::fs::write(&theme_path, b"{invalid")?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.reload_theme_file(window, cx);
            anyhow::ensure!(
                app.terminal_palette.text == active_text,
                "invalid theme replaced active colours"
            );
            anyhow::ensure!(app.theme_error.is_some(), "invalid theme error missing");
            Ok::<_, anyhow::Error>(())
        })
    })??;
    std::fs::write(&theme_path, &original)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| app.reload_theme_file(window, cx));
    })?;
    draw(cx)?;
    let before_editor = std::fs::read(&theme_path)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.settings_ui.page = 8;
            app.open_theme_editor(None, window, cx);
        });
        anyhow::ensure!(window.has_active_dialog(cx), "theme editor dialog missing");
        Ok::<_, anyhow::Error>(())
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.settings_ui
                .theme_editor
                .update(cx, |editor, cx| editor.set_value("{invalid", window, cx));
            app.save_theme_editor(window, cx);
            anyhow::ensure!(
                app.settings_ui.theme_edit_error.is_some(),
                "theme editor accepted invalid JSON"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            let id = app.themes.next_id();
            let template = app.themes.selected(&app.light_theme).unwrap().clone();
            let mut theme =
                serde_json::to_value(ThemeDefinition::from_theme(&template, id.clone()))?;
            theme["ink"] = "#123456".into();
            app.settings_ui.theme_editor.update(cx, |editor, cx| {
                editor.set_value(serde_json::to_string_pretty(&theme).unwrap(), window, cx)
            });
            app.save_theme_editor(window, cx);
            anyhow::ensure!(
                app.light_theme != id,
                "editing a scheme changed the selected theme"
            );
            anyhow::ensure!(
                ThemeFile::load(&AppView::theme_path())?
                    .selected(&id)
                    .is_some(),
                "new theme was not saved"
            );
            app.select_theme(ThemeMode::Light, &id, window, cx);
            anyhow::ensure!(
                app.terminal_palette.text == 0x123456,
                "new theme colours did not apply"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            let id = app.light_theme.clone();
            app.open_theme_editor(Some(id), window, cx);
        });
    })?;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            let mut theme: serde_json::Value =
                serde_json::from_str(app.settings_ui.theme_editor.read(cx).value().as_ref())?;
            theme["ink"] = "#654321".into();
            app.settings_ui.theme_editor.update(cx, |editor, cx| {
                editor.set_value(serde_json::to_string_pretty(&theme).unwrap(), window, cx)
            });
            app.save_theme_editor(window, cx);
            anyhow::ensure!(
                app.terminal_palette.text == 0x654321,
                "edited theme colours did not apply"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            let id = app.light_theme.clone();
            app.delete_theme(&id, window, cx);
            anyhow::ensure!(
                app.light_theme == "vscode-light",
                "deleted light scheme did not use its fallback"
            );
            anyhow::ensure!(
                ThemeFile::load(&AppView::theme_path())?
                    .selected(&id)
                    .is_none(),
                "deleted theme stayed in theme.json"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    std::fs::write(AppView::theme_path(), before_editor)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.select_theme(
                app.appearance.mode(window.appearance()),
                "vscode-dark",
                window,
                cx,
            );
            app.reload_theme_file(window, cx);
        });
    })?;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            let event = KeyDownEvent {
                keystroke: Keystroke::parse("escape").unwrap(),
                is_held: false,
                prefer_character_input: false,
            };
            app.keyboard(&event, window, cx);
        });
        anyhow::ensure!(
            !window.has_active_dialog(cx)
                && !app.read(cx).settings
                && app.read(cx).page == Page::Terminal,
            "Escape did not return to the workspace"
        );
        app.read_with(cx, |app, _cx| {
            let prefs = Preferences::load();
            anyhow::ensure!(prefs.font_family == app.font_family, "font not saved");
            anyhow::ensure!(prefs.ligatures, "ligature preference not saved");
            anyhow::ensure!(
                prefs.light_theme.as_deref() == Some(app.light_theme.as_str())
                    && prefs.dark_theme.as_deref() == Some(app.dark_theme.as_str()),
                "theme slots not saved"
            );
            anyhow::ensure!(prefs.background_opacity == Some(0.65), "opacity not saved");
            anyhow::ensure!(
                prefs.sidebar_opacity == Some(0.45),
                "sidebar opacity not saved"
            );
            anyhow::ensure!(prefs.keybindings == app.keybindings, "shortcuts not saved");
            anyhow::ensure!(
                prefs.language == app.language && prefs.language == crate::i18n::Language::System,
                "language preference not saved"
            );
            Ok::<_, anyhow::Error>(())
        })?;
        app.update(cx, |app, cx| app.edit_host(0, window, cx));
        Ok::<_, anyhow::Error>(())
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.label
                .update(cx, |input, cx| input.set_value("Demo", window, cx));
            app.add_host(window, cx);
            anyhow::ensure!(app.hosts[0].name == "Demo", "rename failed");
            Ok::<_, anyhow::Error>(())
        })?;
        anyhow::ensure!(
            !window.has_active_dialog(cx),
            "host dialog remained after rename"
        );
        Ok::<_, anyhow::Error>(())
    })??;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            let count = app.hosts[0].snapshot.windows().count();
            app.show_new_tab(window, cx);
            anyhow::ensure!(
                !window.has_active_dialog(cx),
                "tab creation opened a dialog"
            );
            anyhow::ensure!(
                app.hosts[0].snapshot.windows().count() == count + 1,
                "tab not created"
            );
            let sessions = app.hosts[0].snapshot.sessions.len();
            app.show_new_session(window, cx);
            anyhow::ensure!(
                window.has_active_dialog(cx) && app.creating_tab,
                "session creation dialog missing"
            );
            let home = dirs::home_dir().unwrap();
            anyhow::ensure!(
                app.tab_path.read(cx).value().as_ref() == home.to_string_lossy(),
                "session directory should default to full home path"
            );
            anyhow::ensure!(
                app.session_directory(cx).is_some(),
                "home directory not verified"
            );
            let missing = home.join("tshell-ui-check-missing-directory");
            app.tab_path.update(cx, |input, cx| {
                input.set_value(missing.to_string_lossy().into_owned(), window, cx)
            });
            app.complete_tab_directory(cx);
            anyhow::ensure!(
                app.session_directory(cx).is_none(),
                "missing directory accepted"
            );
            app.create_tab(window, cx);
            anyhow::ensure!(
                app.hosts[0].snapshot.sessions.len() == sessions,
                "missing directory created session"
            );
            let file = Preferences::path();
            app.tab_path.update(cx, |input, cx| {
                input.set_value(file.to_string_lossy().into_owned(), window, cx)
            });
            app.complete_tab_directory(cx);
            anyhow::ensure!(
                app.session_directory(cx).is_none(),
                "file accepted as session directory"
            );
            app.tab_path.update(cx, |input, cx| {
                input.set_value(home.to_string_lossy().into_owned(), window, cx)
            });
            app.complete_tab_directory(cx);
            anyhow::ensure!(
                app.session_directory(cx).is_some(),
                "existing directory not accepted"
            );
            app.tab_name
                .update(cx, |input, cx| input.set_value("UI session", window, cx));
            app.create_tab(window, cx);
            anyhow::ensure!(
                app.hosts[0].snapshot.sessions.len() == sessions + 1,
                "session not created"
            );
            let saved = Preferences::load();
            anyhow::ensure!(
                saved
                    .sessions
                    .get("local")
                    .is_some_and(|profiles| profiles.iter().any(|p| p.name == "UI session"
                        && PathBuf::from(&p.path).canonicalize().ok() == home.canonicalize().ok())),
                "session profile not saved"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    draw(cx)?;
    if let (Ok(destination), Ok(root)) = (
        std::env::var("TSHELL_SSH_TEST_HOST"),
        std::env::var("TSHELL_REMOTE_UI_ROOT"),
    ) {
        let host = HostDefinition {
            id: Default::default(),
            ssh: tshell_host::SshConfig {
                destination,
                user: String::new(),
                port: None,
                identity_file: None,
            },
            name: "Remote test".into(),
            tmux: false,

            socket: None,
        };
        let path = PathBuf::from(format!("{root}/main.rs"));
        cx.update_window(handle.into(), |_, _, cx| {
            app.update(cx, |app, cx| {
                app.hosts.push(Host {
                    name: "Remote test".into(),
                    config: Some(host.clone()),
                    backend: None,
                    snapshot: Snapshot {
                        active_session: "test".into(),
                        sessions: vec![crate::backend::SessionInfo {
                            id: "test".into(),
                            cwd: root.clone(),
                            active_window: "test-tab".into(),
                            windows: vec![crate::backend::WindowInfo {
                                id: "test-tab".into(),
                                active_pane: "test-pane".into(),
                                panes: vec![crate::backend::PaneInfo {
                                    id: "test-pane".into(),
                                    cwd: root.clone(),
                                    ..Default::default()
                                }],
                                ..Default::default()
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    views: BTreeMap::new(),
                    read_notices: BTreeMap::new(),
                    collapsed_sessions: BTreeSet::new(),
                    viewport: None,
                    pending: Vec::new(),
                });
                app.active = app.hosts.len() - 1;
                app.sync(cx);
                app.show_files(cx);
            });
        })?;
        let deadline = std::time::Instant::now() + Duration::from_secs(25);
        while app.read_with(cx, |app, cx| {
            app.navigation_loading.is_some() || app.check_explorer(cx).file_loading.is_some()
        }) {
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "remote tree timed out"
            );
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
        }
        draw(cx)?;
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| app.open_path(path.clone(), window, cx));
        })?;
        let deadline = std::time::Instant::now() + Duration::from_secs(25);
        while app.read_with(cx, |app, cx| {
            app.navigation_loading.is_some() || app.check_explorer(cx).file_loading.is_some()
        }) {
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "remote read timed out"
            );
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
        }
        draw(cx)?;
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                anyhow::ensure!(
                    app.check_explorer(cx).open_file.as_ref() == Some(&path),
                    "remote file not opened"
                );
                anyhow::ensure!(
                    app.check_explorer(cx)
                        .file_editor
                        .read(cx)
                        .value()
                        .contains("fn main"),
                    "remote contents missing"
                );
                app.check_explorer(cx)
                    .file_editor
                    .clone()
                    .update(cx, |editor, cx| {
                        editor.set_value("fn main() { /* remote UI saved */ }", window, cx)
                    });
                app.save_open_file(window, cx);
                Ok::<_, anyhow::Error>(())
            })
        })??;
        let deadline = std::time::Instant::now() + Duration::from_secs(25);
        while app.read_with(cx, |app, cx| app.check_explorer(cx).file_saving) {
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "remote save timed out"
            );
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
        }
        let remote_path = path.clone();
        let remote = cx
            .background_executor()
            .spawn(async move {
                remote_files::read(&remote_files::Session::new(host.ssh), &remote_path)
            })
            .await?;
        anyhow::ensure!(
            remote == "fn main() { /* remote UI saved */ }",
            "remote save did not persist"
        );
        app.update(cx, |app, cx| app.show_git(cx));
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while app.read_with(cx, |app, cx| app.check_git(cx).git_loading) {
            anyhow::ensure!(std::time::Instant::now() < deadline, "remote Git timed out");
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
        }
        app.read_with(cx, |app, cx| {
            anyhow::ensure!(
                app.check_git(cx).git_error.is_none(),
                "remote Git failed: {:?}",
                app.check_git(cx).git_error
            );
            anyhow::ensure!(
                app.check_git(cx)
                    .git_changes
                    .iter()
                    .any(|change| change.path == path),
                "remote Git path did not resolve to the file"
            );
            Ok::<_, anyhow::Error>(())
        })?;
        draw(cx)?;
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.open_path(path, window, cx);
                app.active = 0;
                app.sync(cx);
            });
        })?;
        cx.background_executor()
            .timer(Duration::from_millis(750))
            .await;
        draw(cx)?;
        anyhow::ensure!(
            app.read_with(cx, |app, cx| app
                .check_explorer(cx)
                .open_file
                .as_ref()
                .is_some_and(|path| path.ends_with("Cargo.toml"))),
            "stale remote read replaced local editor"
        );
    }
    if let Ok(destination) = std::env::var("TSHELL_SSH_TEST_HOST") {
        app.update(cx, |app, cx| {
            app.active = 0;
            app.hosts[0].config = Some(HostDefinition {
                id: Default::default(),
                ssh: tshell_host::SshConfig {
                    destination,
                    user: String::new(),
                    port: None,
                    identity_file: None,
                },
                name: String::new(),
                tmux: false,

                socket: None,
            });
            app.show_terminal(cx);
            app.sync_metrics(cx);
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while app.read_with(cx, |app, _cx| app.metrics.is_none()) {
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "host metrics timed out"
            );
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
        }
        app.read_with(cx, |app, _cx| -> anyhow::Result<()> {
            anyhow::ensure!(
                matches!(&app.metrics, Some(Ok(_))),
                "host metrics unavailable"
            );
            Ok(())
        })?;
        draw(cx)?;
        app.update(cx, |app, cx| {
            app.hosts[0].config = None;
            app.sync_metrics(cx);
            assert!(app.metrics.is_none() && app.metrics_monitor.is_some());
        });
        draw(cx)?;
    }
    // Search through the actual command model and confirm the filtered result.
    for (query, section) in [
        ("new_session", Some(2)),
        ("add_host", Some(2)),
        ("edit_host", Some(2)),
        ("column", Some(2)),
        ("refresh_files", Some(2)),
        ("toggle_sidebar", Some(2)),
        ("palette-no-match-fixture", None),
        ("remove_host", None),
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.show_command_palette(window, cx);
                app.command_state
                    .update(cx, |state, cx| state.set_query(query, window, cx));
            });
        })?;
        draw(cx)?;
        app.read_with(cx, |app, cx| {
            anyhow::ensure!(
                app.command_state
                    .read(cx)
                    .selected_index()
                    .map(|index| index.section)
                    == section,
                "command palette category/disabled filtering failed for {query}"
            );
            Ok::<_, anyhow::Error>(())
        })?;
    }
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.command_state
                .update(cx, |state, cx| state.set_query("show_", window, cx));
        });
    })?;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("down").unwrap(), cx);
    })?;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(25))
        .await;
    draw(cx)?;
    app.read_with(cx, |app, _cx| {
        anyhow::ensure!(
            !app.command_palette && app.page == Page::Explorer,
            "filtered arrow/Enter command selected the wrong original target"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.show_command_palette(window, cx);
            app.command_state
                .update(cx, |state, cx| state.set_query("new_session", window, cx));
        });
    })?;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(25))
        .await;
    draw(cx)?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            anyhow::ensure!(
                app.creating_tab && !app.command_palette,
                "new-session command did not replace the palette with its editor"
            );
            app.close_modal(window, cx);
            anyhow::ensure!(
                !window.has_active_dialog(cx),
                "reopening the command palette left stacked dialogs after dismissal"
            );
            Ok::<_, anyhow::Error>(())
        })
    })??;
    let launcher_dir = Preferences::path().with_file_name("launcher-search-directory");
    std::fs::create_dir_all(&launcher_dir)?;
    let original_session =
        app.read_with(cx, |app, _cx| app.hosts[0].snapshot.active_session.clone());
    app.update(cx, |app, cx| {
        app.switch_host(0, cx);
        app.act(
            Action::NewNamedSession {
                name: "launcher-search-session".into(),
                path: launcher_dir.to_string_lossy().into_owned(),
            },
            cx,
        );
        app.act(Action::SelectSession(original_session), cx);
    });
    let session_targets = app.read_with(cx, |app, _cx| {
        app.hosts[0]
            .snapshot
            .sessions
            .iter()
            .enumerate()
            .filter(|(index, session)| *index == 0 || session.name == "launcher-search-session")
            .map(|(index, session)| (index, session.id.clone()))
            .collect::<Vec<_>>()
    });
    for (index, session_id) in &session_targets {
        let key = format!("alt-{}", index + 1);
        let sessions_before = app.read_with(cx, |app, _cx| app.hosts[0].snapshot.sessions.len());
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse(&key).unwrap(), cx);
        })?;
        draw(cx)?;
        app.read_with(cx, |app, _cx| {
            anyhow::ensure!(
                app.active == 0 && app.hosts[0].snapshot.active_session == *session_id,
                "number shortcut selected the wrong host/session: {key}"
            );
            anyhow::ensure!(
                app.hosts[0].snapshot.sessions.len() == sessions_before,
                "number shortcut created a session"
            );
            anyhow::ensure!(
                app.page == Page::Terminal,
                "number shortcut did not show the selected terminal"
            );
            Ok::<_, anyhow::Error>(())
        })?;
    }
    let active_session = app.read_with(cx, |app, _cx| app.hosts[0].snapshot.active_session.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("alt-9").unwrap(), cx);
    })?;
    draw(cx)?;
    app.read_with(cx, |app, _cx| {
        anyhow::ensure!(
            app.active == 0 && app.hosts[0].snapshot.active_session == active_session,
            "unavailable session shortcut changed the selection"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    app.update(cx, |app, _cx| {
        let mut snapshot = app.hosts[0].snapshot.clone();
        for session in &mut snapshot.sessions {
            session.name = "palette-other-host-session".into();
            session.cwd = "palette-other-host-directory".into();
        }
        app.hosts.push(Host {
            name: "Other host fixture".into(),
            config: None,
            backend: None,
            snapshot,
            pending: Vec::new(),
            views: BTreeMap::new(),
            read_notices: BTreeMap::new(),
            collapsed_sessions: BTreeSet::new(),
            viewport: None,
        });
    });
    for query in ["palette-other-host-session", "palette-other-host-directory"] {
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.show_command_palette(window, cx);
                app.command_state
                    .update(cx, |state, cx| state.set_query(query, window, cx));
            });
        })?;
        draw(cx)?;
        app.read_with(cx, |app, cx| {
            anyhow::ensure!(
                app.command_state.read(cx).matched_count() == 0,
                "command palette exposed another host's session: {query}"
            );
            Ok::<_, anyhow::Error>(())
        })?;
    }
    app.update(cx, |app, _cx| {
        app.hosts.pop();
    });
    for (query, section) in [
        ("launcher-search-session", 1),
        ("launcher-search-directory", 1),
        ("localhost", 0),
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.show_command_palette(window, cx);
                app.command_state
                    .update(cx, |state, cx| state.set_query(query, window, cx));
            });
        })?;
        draw(cx)?;
        app.read_with(cx, |app, cx| {
            anyhow::ensure!(
                app.command_state
                    .read(cx)
                    .selected_index()
                    .is_some_and(|index| index.section == section),
                "launcher query did not select the correct category target: {query}"
            );
            Ok::<_, anyhow::Error>(())
        })?;
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
        })?;
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        draw(cx)?;
        app.read_with(cx, |app, _cx| {
            anyhow::ensure!(
                !app.command_palette && app.active == 0,
                "launcher did not navigate and close"
            );
            if section == 1 {
                anyhow::ensure!(
                    app.hosts[0]
                        .snapshot
                        .session()
                        .is_some_and(|session| session.name == "launcher-search-session"),
                    "filtered launcher result selected the wrong session"
                );
                anyhow::ensure!(
                    app.page == Page::Terminal,
                    "session launcher did not show terminal"
                );
            }
            Ok::<_, anyhow::Error>(())
        })?;
    }
    app.update(cx, |app, cx| -> anyhow::Result<()> {
        let deleted = app.hosts[0].snapshot.active_session.clone();
        let remaining: Vec<_> = app.hosts[0]
            .snapshot
            .sessions
            .iter()
            .filter(|session| session.id != deleted)
            .cloned()
            .collect();
        app.act(Action::RemoveSession(deleted.clone()), cx);
        anyhow::ensure!(
            !app.hosts[0]
                .snapshot
                .sessions
                .iter()
                .any(|session| session.id == deleted),
            "deleted session remained in the sidebar"
        );
        anyhow::ensure!(
            app.hosts[0].snapshot.sessions == remaining,
            "deleting a session changed another session"
        );
        anyhow::ensure!(
            !Preferences::load().sessions["local"]
                .iter()
                .any(|profile| profile.name == "launcher-search-session"),
            "deleted session remained in saved profiles"
        );
        app.disconnect_host(0, cx);
        anyhow::ensure!(
            app.hosts[0].backend.is_some(),
            "local host was disconnected"
        );
        let local = app.hosts[0].snapshot.clone();
        let config = HostDefinition {
            id: Default::default(),
            ssh: tshell_host::SshConfig {
                destination: "example.invalid".into(),
                user: "test".into(),
                port: None,
                identity_file: None,
            },
            name: "Disconnect test".into(),
            tmux: false,
            socket: None,
        };
        app.hosts.push(Host {
            name: config.name.clone(),
            config: Some(config.clone()),
            backend: Some(Backend::Local(LocalBackend::restore(
                std::env::temp_dir(),
                None,
                Some(Vec::new()),
            )?)),
            snapshot: Snapshot {
                connection: Connection::Ready,
                ..Default::default()
            },
            pending: vec![Action::NewSession],
            views: BTreeMap::new(),
            read_notices: BTreeMap::new(),
            collapsed_sessions: BTreeSet::new(),
            viewport: None,
        });
        let index = app.hosts.len() - 1;
        let saved = app.session_profiles.clone();
        let active = app.active;
        let file_key = (app.hosts[index].identity(), "session".to_string());
        let explorer = app.explorer_entity().unwrap();
        explorer.update(cx, |explorer, _| explorer.editor_dirty = true);
        app.workspaces.insert(
            file_key.clone(),
            SessionWorkspace {
                explorer: Some(explorer.clone()),
                ..Default::default()
            },
        );
        app.disconnect_host(index, cx);
        anyhow::ensure!(
            app.hosts[index].backend.is_none(),
            "remote backend remained connected"
        );
        anyhow::ensure!(
            app.workspaces.contains_key(&file_key),
            "disconnect discarded unsaved host files"
        );
        explorer.update(cx, |explorer, _| explorer.editor_dirty = false);
        let request = app.navigation_request;
        anyhow::ensure!(
            app.prepare_host_connection_change(index, cx),
            "saved host files prevented connection change"
        );
        anyhow::ensure!(
            !app.workspaces.contains_key(&file_key) && app.navigation_request == request,
            "inactive connection change cancelled active file requests"
        );
        app.message = None;
        anyhow::ensure!(app.active == active, "disconnect switched the active host");
        anyhow::ensure!(
            app.hosts[index].backend.is_none(),
            "remote backend remained connected"
        );
        anyhow::ensure!(
            app.hosts[index].views.is_empty(),
            "disconnected terminal views remained"
        );
        anyhow::ensure!(
            app.hosts[index].config.as_ref() == Some(&config),
            "disconnect removed host configuration"
        );
        anyhow::ensure!(
            app.session_profiles == saved,
            "disconnect removed saved sessions"
        );
        anyhow::ensure!(
            app.hosts[index].connection() == Connection::Closed,
            "disconnect status not cleared"
        );
        anyhow::ensure!(
            app.hosts[0].snapshot == local,
            "disconnect changed another host"
        );
        app.active = index;
        app.sync(cx);
        app.show_files(cx);
        anyhow::ensure!(
            app.page == Page::Terminal && app.remote_files().is_none(),
            "disconnected host reopened Explorer or SFTP"
        );
        let git_request = app.git_entity().map(|entity| entity.read(cx).git_request);
        app.show_git(cx);
        app.refresh_git(cx);
        anyhow::ensure!(
            app.page == Page::Terminal
                && app.git_entity().map(|entity| entity.read(cx).git_request) == git_request,
            "disconnected host started Git access"
        );
        anyhow::ensure!(
            app.metrics_monitor.is_none(),
            "disconnected host started metrics"
        );
        Ok(())
    })?;
    draw(cx)?;
    #[cfg(feature = "ui-check-screenshots")]
    check_command_palette_styles(handle, app.clone(), cx).await?;
    ssh_ui_check::check(handle, app.clone(), cx).await?;
    transfer_queue::check_transfer_panel(handle, app.clone(), cx).await?;
    #[cfg(feature = "ui-check-screenshots")]
    check_workspace_styles(handle, app.clone(), cx).await?;
    feature_check::check(handle, app.clone(), cx).await?;
    check_docker_workspace(handle, app.clone(), cx).await?;
    if let Ok(destination) = std::env::var("TSHELL_SSH_TEST_HOST") {
        ssh_ui_check::check_session_submission(handle, &app, destination, cx).await?;
    }
    Ok(())
}

async fn check_html_preview(
    handle: WindowHandle<Root>,
    app: &Entity<AppView>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let directory = PathBuf::from(std::env::var_os("TSHELL_DATA_DIR").unwrap());
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("index.html");
    std::fs::write(
        &path,
        "<!doctype html><h1>HTML preview regression</h1><p>Ready</p>",
    )?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.check_initialize_tools(window, cx);
            app.open_path(path.clone(), window, cx);
        });
    })?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let ready = app.read_with(cx, |app, cx| {
            let explorer = app.check_explorer(cx);
            #[cfg(windows)]
            return explorer
                .web_preview
                .as_ref()
                .is_some_and(|web| web.is_visible());
            #[cfg(not(windows))]
            return explorer.preview_mode && explorer.open_file.as_ref() == Some(&path);
        });
        if ready {
            break;
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "HTML preview did not become visible"
        );
    }
    // Keep dispatching real native events after the initial preview creation.
    cx.background_executor()
        .timer(Duration::from_millis(500))
        .await;
    app.read_with(cx, |app, cx| {
        let explorer = app.check_explorer(cx);
        anyhow::ensure!(
            explorer.open_file.as_ref() == Some(&path),
            "HTML file changed"
        );
        anyhow::ensure!(!explorer.editor_dirty, "HTML preview dirtied the file");
        Ok::<_, anyhow::Error>(())
    })?;
    app.update(cx, |app, cx| app.show_terminal(cx));
    cx.background_executor()
        .timer(Duration::from_millis(100))
        .await;
    #[cfg(windows)]
    app.read_with(cx, |app, cx| {
        anyhow::ensure!(
            app.check_explorer(cx)
                .web_preview
                .as_ref()
                .is_some_and(|web| !web.is_visible()),
            "HTML preview stayed visible over the terminal"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(())
}

async fn check_common_previews(
    handle: WindowHandle<Root>,
    app: &Entity<AppView>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    check_html_preview(handle, app, cx).await?;
    let directory = PathBuf::from(std::env::var_os("TSHELL_DATA_DIR").unwrap());
    for (name, source, malformed) in [
        (
            "document.pdf",
            std::str::from_utf8(include_bytes!("../../tests/fixtures/explorer-preview.pdf"))?,
            false,
        ),
        (
            "icon.svg",
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 80"><rect width="240" height="80" fill="white"/><text x="12" y="45" fill="black">TShell 中文 SVG</text></svg>"#,
            false,
        ),
        ("flow.mmd", "flowchart LR\n A[本地] --> B[远程]", false),
        (
            "sequence.mermaid",
            "sequenceDiagram\n Alice->>Bob: Hello",
            false,
        ),
        ("invalid.svg", "<svg>broken", true),
        ("invalid.mmd", "this is not a diagram", true),
    ] {
        let path = directory.join(name);
        let is_pdf = name.ends_with(".pdf");
        std::fs::write(&path, source)?;
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| app.open_path(path.clone(), window, cx));
        })?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
            let complete = app.read_with(cx, |app, cx| {
                let explorer = app.check_explorer(cx);
                if explorer.open_file.as_ref() != Some(&path) || explorer.file_loading.is_some() {
                    return Ok(false);
                }
                anyhow::ensure!(!explorer.editor_dirty, "{name} preview dirtied the file");
                if is_pdf {
                    anyhow::ensure!(
                        explorer
                            .pdf_preview
                            .as_ref()
                            .is_some_and(|bytes| bytes.as_slice() == source.as_bytes()),
                        "PDF bytes changed"
                    );
                    anyhow::ensure!(
                        explorer.file_editor.read(cx).value().is_empty(),
                        "PDF entered the text editor"
                    );
                } else {
                    anyhow::ensure!(
                        explorer.file_editor.read(cx).value().as_ref() == source,
                        "{name} source changed"
                    );
                }
                #[cfg(windows)]
                {
                    use tshell_explorer::preview::RenderStatus;
                    anyhow::ensure!(
                        !explorer.web_preview_failed,
                        "{name} WebView initialization failed"
                    );
                    let Some(web) = &explorer.web_preview else {
                        return Ok(false);
                    };
                    if web.status() == RenderStatus::Loading || !web.is_visible() {
                        return Ok(false);
                    }
                    anyhow::ensure!(
                        web.status()
                            == if malformed {
                                RenderStatus::Failed
                            } else {
                                RenderStatus::Ready
                            },
                        "{name} render status: {:?}",
                        web.status()
                    );
                }
                #[cfg(not(windows))]
                let _ = malformed;
                Ok::<_, anyhow::Error>(true)
            })?;
            if complete {
                break;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "{name} did not render before the deadline"
            );
        }
        if is_pdf {
            cx.update_window(handle.into(), |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.explorer_entity().unwrap().update(cx, |explorer, cx| {
                        explorer.save_open_file(window, cx);
                        anyhow::ensure!(!explorer.file_saving, "PDF save entered the text writer");
                        Ok::<_, anyhow::Error>(())
                    })
                })
            })??;
            anyhow::ensure!(
                std::fs::read(&path)? == source.as_bytes(),
                "PDF save changed its bytes"
            );
            app.update(cx, |app, cx| app.show_terminal(cx));
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            continue;
        }
        // Exercise Edit/Preview transitions without replacing or saving the source.
        app.update(cx, |app, cx| {
            app.explorer_entity().unwrap().update(cx, |explorer, cx| {
                explorer.preview_mode = false;
                explorer.file_preview = None;
                cx.notify();
            });
        });
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        #[cfg(windows)]
        app.read_with(cx, |app, cx| {
            anyhow::ensure!(
                app.check_explorer(cx)
                    .web_preview
                    .as_ref()
                    .is_some_and(|web| !web.is_visible()),
                "{name} covers its source editor"
            );
            Ok::<_, anyhow::Error>(())
        })?;
        app.update(cx, |app, cx| app.show_terminal(cx));
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
    Ok(())
}

#[cfg(feature = "ui-check-screenshots")]
async fn check_command_palette_styles(
    handle: WindowHandle<Root>,
    app: Entity<AppView>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") else {
        return Ok(());
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory)?;
    let original = app.read_with(cx, |app, _cx| (app.active, app.appearance));
    for (name, appearance, viewport) in [
        ("dark", Appearance::Dark, size(px(1320.), px(840.))),
        ("light-small", Appearance::Light, size(px(860.), px(520.))),
    ] {
        handle.update(cx, |root, window, _| {
            root.style().size.width = Some(viewport.width.into());
            root.style().size.height = Some(viewport.height.into());
            window.resize(viewport);
        })?;
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.switch_host(0, cx);
                app.select_theme(ThemeMode::Dark, "vscode-dark", window, cx);
                app.select_theme(ThemeMode::Light, "vscode-light", window, cx);
                app.set_appearance(appearance, window, cx);
                app.show_command_palette(window, cx);
            });
        })?;
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
        for (suffix, query) in [
            ("", None),
            ("-hosts", Some("localhost")),
            ("-sessions", Some("UI session")),
        ] {
            cx.update_window(handle.into(), |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.command_state.update(cx, |state, cx| {
                        state.set_query(query.unwrap_or(""), window, cx)
                    });
                });
                window.refresh();
                let _ = window.draw(cx);
                window.render_to_image()?.save(
                    directory.join(format!("workspace-{name}-command-palette{suffix}.png")),
                )?;
                Ok::<_, anyhow::Error>(())
            })??;
        }
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| app.close_modal(window, cx));
        })?;
    }
    handle.update(cx, |root, window, _| {
        root.style().size.width = Some(px(1320.).into());
        root.style().size.height = Some(px(840.).into());
        window.resize(size(px(1320.), px(840.)));
    })?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.switch_host(original.0, cx);
            app.set_appearance(original.1, window, cx);
        });
    })?;
    Ok(())
}

#[cfg(feature = "ui-check-screenshots")]
async fn check_workspace_styles(
    handle: WindowHandle<Root>,
    app: Entity<AppView>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") else {
        return Ok(());
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory)?;
    let capture = |name: &str, cx: &mut AsyncApp| -> anyhow::Result<()> {
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
            window
                .render_to_image()?
                .save(directory.join(format!("workspace-{name}.png")))?;
            Ok::<_, anyhow::Error>(())
        })??;
        Ok(())
    };
    for (name, appearance, viewport) in [
        ("dark", Appearance::Dark, size(px(1320.), px(840.))),
        ("light-small", Appearance::Light, size(px(860.), px(520.))),
    ] {
        handle.update(cx, |root, window, _| {
            root.style().size.width = Some(viewport.width.into());
            root.style().size.height = Some(viewport.height.into());
            window.resize(viewport);
        })?;
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.close_modal(window, cx);
                app.active = 0;
                app.zen = false;
                app.floating = false;
                app.sidebar_collapsed = false;
                app.sidebar_width = SIDEBAR_WIDTH;
                app.show_status_bar = true;
                app.page = Page::Terminal;
                app.hosts[0].collapsed_sessions.clear();
                app.sync(cx);
                app.select_theme(ThemeMode::Light, "vscode-light", window, cx);
                app.set_appearance(appearance, window, cx);
            });
        })?;
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
        capture(&format!("{name}-sidebar"), cx)?;
        let (original_host, original_message) = app.update(cx, |app, cx| {
            let original = (app.active, app.message.take());
            let config = HostDefinition {
                id: Default::default(),
                name: "Reconnect fixture".into(),
                ssh: tshell_host::SshConfig {
                    destination: "reconnect-ui-check.invalid".into(),
                    user: "test".into(),
                    port: None,
                    identity_file: None,
                },
                tmux: false,
                socket: None,
            };
            app.active = app.hosts.len();
            app.hosts.push(Host {
                name: config.name.clone(),
                config: Some(config),
                backend: None,
                snapshot: Snapshot {
                    connection: Connection::Closed,
                    message: Some(
                        crate::t!(
                            "ws.start_failed",
                            error = "Fixture SSH connection was closed."
                        )
                        .into_owned(),
                    ),
                    ..Default::default()
                },
                pending: Vec::new(),
                views: BTreeMap::new(),
                read_notices: BTreeMap::new(),
                collapsed_sessions: BTreeSet::new(),
                viewport: None,
            });
            cx.notify();
            original
        });
        capture(&format!("{name}-connection-failure"), cx)?;
        app.update(cx, |app, cx| {
            app.sidebar_collapsed = true;
            cx.notify();
        });
        capture(&format!("{name}-connection-failure-no-sidebar"), cx)?;
        app.update(cx, |app, cx| {
            app.active = original_host;
            app.hosts.pop();
            app.message = original_message;
            app.sidebar_collapsed = false;
            cx.notify();
        });
        cx.update_window(handle.into(), |_, _, cx| {
            app.update(cx, |app, cx| {
                app.act(Action::NewWindow, cx);
                for axis in [
                    SplitAxis::Horizontal,
                    SplitAxis::Vertical,
                    SplitAxis::Vertical,
                    SplitAxis::Horizontal,
                ] {
                    app.act(Action::Split(axis), cx);
                }
            });
        })?;
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })?;
        cx.background_executor()
            .timer(Duration::from_millis(500))
            .await;
        app.read_with(cx, |app, _cx| {
            let host = &app.hosts[0];
            let window = host.snapshot.window().unwrap();
            for (pane, label) in window.panes.iter().zip(["Build", "Logs", "Tests", "Git", "Shell"]) {
                if let Some(screen) = host.backend.as_ref().and_then(|backend| backend.screen(&pane.id)) {
                    screen.remote_output(format!(
                        "\x1b[2J\x1b[H\x1b[1m{label}\x1b[0m\r\n/tshell\r\n\r\n\x1b[32mReady\x1b[0m\r\n$ "
                    ).as_bytes());
                }
            }
        });
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        capture(&format!("{name}-splits"), cx)?;
        app.read_with(cx, |app, _cx| check_equal_split_bounds(app))?;
        cx.update_window(handle.into(), |_, _, cx| {
            app.update(cx, |app, cx| app.act(Action::CloseWindow, cx));
        })?;
        let mut pages = vec![
            (0, "settings"),
            (8, "themes"),
            (1, "shortcuts"),
            (2, "metrics"),
            (5, "hosts"),
            (6, "keys"),
        ];
        #[cfg(windows)]
        pages.push((7, "updates"));
        for (page, suffix) in pages {
            cx.update_window(handle.into(), |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.settings_ui.page = page;
                    app.show_settings(window, cx);
                });
            })?;
            capture(&format!("{name}-{suffix}"), cx)?;
            if page == 1 {
                app.update(cx, |app, cx| {
                    app.settings_ui.recording = Some(0);
                    app.settings_ui.notice =
                        Some(crate::t!("shortcut.error_reserved").into_owned());
                    cx.notify();
                });
                capture(&format!("{name}-shortcuts-error"), cx)?;
                app.update(cx, |app, cx| {
                    app.settings_ui.recording = None;
                    app.settings_ui.notice = None;
                    cx.notify();
                });
            }
            #[cfg(windows)]
            if page == 7 {
                let original = app.read_with(cx, |app, _| app.updater.state.clone());
                for (suffix, state) in [
                    ("checking", crate::update::State::Checking),
                    ("downloading", crate::update::State::Downloading("1.2.3".into())),
                    ("ready", crate::update::State::Ready("1.2.3".into())),
                    ("failed", crate::update::State::Failed("Fixture download request failed. Please retry when the connection is available.".into())),
                ] {
                    app.update(cx, |app, cx| { app.updater.state = state; cx.notify(); });
                    capture(&format!("{name}-updates-{suffix}"), cx)?;
                }
                app.update(cx, |app, cx| {
                    app.updater.state = original;
                    cx.notify();
                });
            }
            if page == 8 {
                cx.update_window(handle.into(), |_, window, cx| {
                    app.update(cx, |app, cx| app.open_theme_editor(None, window, cx));
                })?;
                capture(&format!("{name}-theme-editor"), cx)?;
                let bounds = app.read_with(cx, |app, cx| {
                    app.settings_ui.theme_editor.read(cx).input_bounds()
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    app.update(cx, |app, cx| {
                        app.settings_ui
                            .theme_editor
                            .update(cx, |editor, cx| editor.set_value("{invalid", window, cx));
                        app.save_theme_editor(window, cx);
                    });
                })?;
                capture(&format!("{name}-theme-editor-error"), cx)?;
                anyhow::ensure!(
                    bounds
                        == app.read_with(cx, |app, cx| app
                            .settings_ui
                            .theme_editor
                            .read(cx)
                            .input_bounds()),
                    "theme error moved editor viewport"
                );
            }
            if page == 0 {
                for (steps, section) in [(3, "font"), (12, "background")] {
                    for _ in 0..steps {
                        cx.update_window(handle.into(), |_, window, cx| {
                            window.dispatch_event(
                                PlatformInput::ScrollWheel(ScrollWheelEvent {
                                    position: point(viewport.width - px(70.), viewport.height / 2.),
                                    delta: ScrollDelta::Lines(point(0., -3.)),
                                    ..Default::default()
                                }),
                                cx,
                            );
                            window.refresh();
                            let _ = window.draw(cx);
                        })?;
                    }
                    capture(&format!("{name}-settings-{section}"), cx)?;
                }
            }
            cx.update_window(handle.into(), |_, window, cx| {
                app.update(cx, |app, cx| app.close_modal(window, cx));
            })?;
        }
        let language = app.read_with(cx, |app, _cx| app.language);
        for page in [0, 2] {
            cx.update_window(handle.into(), |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.set_language(crate::i18n::Language::En, cx);
                    app.settings_ui.page = page;
                    app.show_settings(window, cx);
                });
            })?;
            capture(&format!("{name}-settings-en-{page}"), cx)?;
            cx.update_window(handle.into(), |_, window, cx| {
                app.update(cx, |app, cx| app.close_modal(window, cx));
            })?;
        }
        app.update(cx, |app, cx| app.set_language(language, cx));
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.show_add(window, cx);
            });
        })?;
        capture(&format!("{name}-host-form"), cx)?;
        let host_fields = app.read_with(cx, |app, cx| {
            [
                &app.label,
                &app.destination,
                &app.user,
                &app.port,
                &app.identity_file,
            ]
            .map(|input| input.read(cx).input_bounds())
        });
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.destination
                    .update(cx, |input, cx| input.set_value("example.test", window, cx));
                app.add_host(window, cx);
            });
        })?;
        capture(&format!("{name}-host-error"), cx)?;
        anyhow::ensure!(
            host_fields
                == app.read_with(cx, |app, cx| {
                    [
                        &app.label,
                        &app.destination,
                        &app.user,
                        &app.port,
                        &app.identity_file,
                    ]
                    .map(|input| input.read(cx).input_bounds())
                }),
            "host validation moved form fields"
        );
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.close_modal(window, cx);
                app.edit_host(0, window, cx);
            });
        })?;
        capture(&format!("{name}-host-rename"), cx)?;
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.close_modal(window, cx);
                app.show_new_session(window, cx);
                app.directory_task = None;
                app.directory_candidates = (0..30)
                    .map(|index| format!("/workspace/project-{index:02}"))
                    .collect();
                app.directory_error = None;
                cx.notify();
            });
        })?;
        capture(&format!("{name}-new-session"), cx)?;
        let directory_bounds = app.read_with(cx, |app, cx| app.tab_path.read(cx).input_bounds());
        app.update(cx, |app, cx| {
            app.directory_error = Some(crate::t!("ws.directory_missing").into_owned());
            cx.notify();
        });
        capture(&format!("{name}-new-session-error"), cx)?;
        anyhow::ensure!(
            directory_bounds == app.read_with(cx, |app, cx| app.tab_path.read(cx).input_bounds()),
            "directory validation moved session controls"
        );
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.close_modal(window, cx);
                app.page = Page::Git;
                let change = git::GitChange {
                    status: " M".into(),
                    path: app.cwd.join("src/workspace.rs"),
                    original: None,
                    root: app.cwd.clone(),
                    stats: Some(git::Stats {
                        added: 2,
                        removed: 1,
                    }),
                };
                app.git_entity().unwrap().update(cx, |git, _cx| {
                    git.git_changes = vec![
                        change.clone(),
                        git::GitChange {
                            path: app.cwd.join("docs/git-workspace.md"),
                            stats: Some(git::Stats {
                                added: 38,
                                removed: 12,
                            }),
                            ..change.clone()
                        },
                        git::GitChange {
                            status: "??".into(),
                            path: app.cwd.join("notes.md"),
                            stats: None,
                            ..change.clone()
                        },
                        git::GitChange {
                            path: app.cwd.join("locales/en.yml"),
                            stats: Some(git::Stats {
                                added: 12_345,
                                removed: 9_876,
                            }),
                            ..change.clone()
                        },
                    ];
                });
                app.git_entity().unwrap().update(cx, |git, _cx| {
                    git.git_error = None;
                });
                app.git_entity().unwrap().update(cx, |git, _cx| {
                    git.git_diff_mode = git_view::DiffMode::SideBySide;
                });
                app.git_entity().unwrap().update(cx, |git, _cx| {
                    git.git_diff_compact = true;
                });
                app.git_entity().unwrap().update(cx, |git, cx| {
                    git.git_diff = Some(git_view::DiffView {
                        change,
                        scroll: UniformListScrollHandle::new(),
                        horizontal: std::array::from_fn(|_| ScrollHandle::new()),
                        selection: std::array::from_fn(|_| git_selection::DiffSelection::new(cx)),
                        content: Ok(git_view::DiffContent::new(vec![
                            git::DiffLine {
                                old: Some(1),
                                new: Some(1),
                                kind: git::LineKind::Context,
                                text: " use gpui_kit::component::*;".into(),
                            },
                            git::DiffLine {
                                old: Some(2),
                                new: None,
                                kind: git::LineKind::Removed,
                                text: "-const ROW_HEIGHT: f32 = 26.;".into(),
                            },
                            git::DiffLine {
                                old: None,
                                new: Some(2),
                                kind: git::LineKind::Added,
                                text: "+const ROW_HEIGHT: f32 = 30.;".into(),
                            },
                            git::DiffLine {
                                old: None,
                                new: Some(3),
                                kind: git::LineKind::Added,
                                text: "+const CONTROL_HEIGHT: f32 = 28.;".into(),
                            },
                        ])),
                    });
                });
                cx.notify();
            });
        })?;
        capture(&format!("{name}-git"), cx)?;
        let git = app.read_with(cx, |app, _cx| app.git_entity().unwrap());
        let (original_cwd, selected_path, change_count) = git.update(cx, |git, cx| {
            let original = (
                git.cwd.clone(),
                git.git_diff.as_ref().unwrap().change.path.clone(),
                git.git_changes.len(),
            );
            git.cwd = Preferences::path().with_file_name("missing-git-directory");
            git.refresh_git(cx);
            original
        });
        anyhow::ensure!(
            git.read_with(cx, |git, _| git
                .git_diff
                .as_ref()
                .is_some_and(|diff| diff.change.path == selected_path)),
            "refresh removed selected diff before response"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while git.read_with(cx, |git, _| git.git_loading) {
            anyhow::ensure!(Instant::now() < deadline, "Git failure fixture timed out");
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
        }
        anyhow::ensure!(
            git.read_with(cx, |git, _| git.git_error.is_some()
                && git.git_changes.len() == change_count
                && git
                    .git_diff
                    .as_ref()
                    .is_some_and(|diff| diff.change.path == selected_path)),
            "failed refresh discarded existing Git content"
        );
        git.update(cx, |git, cx| {
            git.cwd = original_cwd;
            git.git_error = None;
            cx.notify();
        });
        for state in [1, 2, 0] {
            app.update(cx, |app, cx| {
                app.git_entity().unwrap().update(cx, |git, cx| {
                    git.git_loading = state == 1;
                    git.diff_loading = state == 1;
                    git.git_error = (state == 2).then(|| {
                        "Fixture Git refresh failed. Existing changes remain visible.".to_owned()
                    });
                    cx.notify();
                });
                cx.notify();
            });
            capture(&format!("{name}-git-refresh-{state}"), cx)?;
        }
        let (saved_changes, saved_diff) = app.update(cx, |app, cx| {
            app.git_entity().unwrap().update(cx, |git, cx| {
                let saved = (std::mem::take(&mut git.git_changes), git.git_diff.take());
                cx.notify();
                saved
            })
        });
        for state in [1, 2] {
            app.update(cx, |app, cx| {
                app.git_entity().unwrap().update(cx, |git, cx| {
                    git.git_loading = state == 1;
                    git.git_error = (state == 2)
                        .then(|| "Fixture Git request failed. Retry using Refresh.".to_owned());
                    cx.notify();
                });
            });
            capture(&format!("{name}-git-empty-{state}"), cx)?;
        }
        app.update(cx, |app, cx| {
            app.git_entity().unwrap().update(cx, |git, cx| {
                git.git_loading = false;
                git.git_error = None;
                git.git_changes = saved_changes;
                git.git_diff = saved_diff;
                cx.notify();
            });
        });
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.page = Page::Explorer;
                app.explorer_entity().unwrap().update(cx,|explorer,_cx|{explorer.open_file = Some(app.cwd.join("src/workspace.rs"));});
                app.explorer_entity().unwrap().update(cx,|explorer,_cx|{explorer.editor_language = "rust".into();});
                app.explorer_entity().unwrap().update(cx,|explorer,_cx|{explorer.image_preview = None;});
                app.explorer_entity().unwrap().update(cx,|explorer,_cx|{explorer.file_preview = None;});
                app.explorer_entity().unwrap().update(cx,|explorer,_cx|{explorer.preview_mode = false;});
                app.check_explorer(cx).file_editor.clone().update(cx, |editor, cx| {
                    editor.set_value("use gpui_kit::component::*;\n\nconst ROW_HEIGHT: f32 = 30.;\nconst CONTROL_HEIGHT: f32 = 28.;\n", window, cx);
                    editor.open_search(true, cx);
                });
                cx.notify();
            });
        })?;
        capture(&format!("{name}-editor"), cx)?;
        let bounds = app.read_with(cx, |app, cx| {
            app.check_explorer(cx).file_editor.read(cx).input_bounds()
        });
        for empty in [false, true] {
            let saved_path = app.update(cx, |app, cx| {
                app.explorer_entity().unwrap().update(cx, |explorer, _| {
                    if empty {
                        explorer.open_file.take()
                    } else {
                        explorer.open_file.clone()
                    }
                })
            });
            for state in [1, 2, 0] {
                app.update(cx, |app, cx| {
                    app.explorer_entity().unwrap().update(cx, |explorer, cx| {
                        explorer.file_loading =
                            (state == 1).then(|| crate::t!("file.reading_file").into_owned());
                        explorer.message = (state == 2).then(|| {
                            "Fixture file request failed. The open document remains available."
                                .to_owned()
                        });
                        cx.notify();
                    });
                    cx.notify();
                });
                capture(
                    &format!(
                        "{name}-editor-{}-{state}",
                        if empty { "empty" } else { "refresh" }
                    ),
                    cx,
                )?;
                if !empty {
                    anyhow::ensure!(
                        bounds
                            == app.read_with(cx, |app, cx| app
                                .check_explorer(cx)
                                .file_editor
                                .read(cx)
                                .input_bounds()),
                        "file feedback moved editor viewport"
                    );
                }
            }
            app.update(cx, |app, cx| {
                app.explorer_entity().unwrap().update(cx, |explorer, cx| {
                    explorer.open_file = saved_path;
                    cx.notify();
                })
            });
        }
    }
    Ok(())
}

fn check_docker_feedback(
    app: &Entity<AppView>,
    window: &mut Window,
    cx: &mut App,
    name: &str,
) -> anyhow::Result<()> {
    let docker = app.read(cx).docker.clone();
    let baseline = docker.read(cx).check_feedback_bounds(cx);
    for state in [0, 1, 2] {
        if !docker.update(cx, |docker, cx| {
            docker.check_config_document_state(state, window, cx)
        }) {
            break;
        }
        window.refresh();
        let _ = window.draw(cx);
        anyhow::ensure!(
            docker.read(cx).check_feedback_bounds(cx) == baseline,
            "Docker {name} document arrival or dirty state moved its editor"
        );
    }
    for state in [1, 2, 0] {
        docker.update(cx, |docker, cx| docker.check_feedback_state(state, cx));
        window.refresh();
        let _ = window.draw(cx);
        anyhow::ensure!(
            docker.read(cx).check_feedback_bounds(cx) == baseline,
            "Docker {name} state {state} shifted fixed controls or content bounds"
        );
        #[cfg(feature = "ui-check-screenshots")]
        if state != 0
            && let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR")
        {
            window
                .render_to_image()?
                .save(PathBuf::from(directory).join(format!("{name}-state-{state}.png")))?;
        }
    }
    if docker.update(cx, |docker, cx| {
        docker.check_feedback_empty_collection(true, window, cx)
    }) {
        window.refresh();
        let _ = window.draw(cx);
        let empty_baseline = docker.read(cx).check_feedback_bounds(cx);
        anyhow::ensure!(
            empty_baseline
                .first()
                .map(|bounds| (bounds.origin.y, bounds.size.height))
                == baseline
                    .first()
                    .map(|bounds| (bounds.origin.y, bounds.size.height)),
            "Docker empty {name} shifted search input vertically"
        );
        for state in [1, 2, 0] {
            docker.update(cx, |docker, cx| docker.check_feedback_state(state, cx));
            window.refresh();
            let _ = window.draw(cx);
            anyhow::ensure!(
                docker.read(cx).check_feedback_bounds(cx).first() == empty_baseline.first(),
                "Docker empty {name} state {state} shifted search input"
            );
            #[cfg(feature = "ui-check-screenshots")]
            if state != 0
                && let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR")
            {
                window.render_to_image()?.save(
                    PathBuf::from(directory).join(format!("{name}-empty-state-{state}.png")),
                )?;
            }
        }
        docker.update(cx, |docker, cx| {
            docker.check_feedback_empty_collection(false, window, cx);
        });
        window.refresh();
        let _ = window.draw(cx);
    }
    Ok(())
}

async fn check_docker_workspace(
    handle: WindowHandle<Root>,
    app: Entity<AppView>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let (previous, remote, remote_id) = cx.update_window(handle.into(), |_, _, cx| {
        app.update(cx, |app, _| {
            let config = HostDefinition {
                id: Default::default(),
                ssh: tshell_host::SshConfig {
                    destination: "docker-ui-check.invalid".into(),
                    user: "test".into(),
                    port: None,
                    identity_file: None,
                },
                name: "Docker host fixture".into(),
                tmux: true,
                socket: None,
            };
            let remote_id = config.id.clone();
            let remote = app.hosts.len();
            app.hosts.push(Host {
                name: config.name.clone(),
                config: Some(config),
                backend: None,
                snapshot: Snapshot::default(),
                pending: Vec::new(),
                views: BTreeMap::new(),
                read_notices: BTreeMap::new(),
                collapsed_sessions: BTreeSet::new(),
                viewport: None,
            });
            (app.active, remote, remote_id)
        })
    })?;
    for (_name, appearance, viewport) in [
        ("dark", Appearance::Dark, size(px(1320.), px(840.))),
        ("light-small", Appearance::Light, size(px(860.), px(520.))),
    ] {
        handle.update(cx, |root, window, cx| {
            // Start this fixture with fresh overlays after earlier command-palette hovers.
            *root = Root::new(app.clone(), window, cx).bg(transparent_black());
            root.style().size.width = Some(viewport.width.into());
            root.style().size.height = Some(viewport.height.into());
            window.resize(viewport);
        })?;
        for tab in 0..3 {
            cx.update_window(handle.into(), |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.close_modal(window, cx);
                    app.set_appearance(appearance, window, cx);
                    app.page = Page::Docker;
                    app.sidebar_collapsed = false;
                    let remote_tab = (tab + 1) % 3;
                    let targets =
                        docker::targets(&app.hosts, app.docker_endpoint.clone(), &app.host_order);
                    app.docker.update(cx, |docker, cx| {
                        docker.set_targets(targets, window, cx);
                        docker.check_fixture(remote_id.clone(), remote_tab, window, cx);
                        docker.check_fixture(tshell_host::HostId::local(), tab, window, cx);
                    });
                    app.switch_host(remote, cx);
                    app.prepare_docker(window, cx);
                    assert_eq!(
                        app.docker.read(cx).selected_target(),
                        Some(remote_id.clone())
                    );
                    assert!(app.hosts[remote].backend.is_none());
                    app.docker.read(cx).check_fixture_state(remote_tab, cx);
                    app.switch_host(0, cx);
                    app.prepare_docker(window, cx);
                    assert_eq!(app.page, Page::Docker);
                    let target = Some(tshell_host::HostId::local());
                    assert_eq!(app.docker.read(cx).selected_target(), target);
                    app.docker.read(cx).check_fixture_state(tab, cx);
                    // Hidden pages follow the host without opening a query or losing state.
                    app.page = Page::Terminal;
                    app.prepare_docker(window, cx);
                    app.active = remote;
                    app.sync(cx);
                    app.prepare_docker(window, cx);
                    assert_eq!(
                        app.docker.read(cx).selected_target(),
                        Some(remote_id.clone())
                    );
                    app.page = Page::Docker;
                    app.prepare_docker(window, cx);
                    app.docker.read(cx).check_fixture_state(remote_tab, cx);
                    app.switch_host(0, cx);
                    app.prepare_docker(window, cx);
                    if let Some(session) = app.hosts[0].snapshot.sessions.last() {
                        app.act(Action::SelectSession(session.id.clone()), cx);
                        app.prepare_docker(window, cx);
                        assert_eq!(app.docker.read(cx).selected_target(), target);
                        app.docker.read(cx).check_fixture_state(tab, cx);
                    }
                    let backend = app.hosts[0].backend.take();
                    app.hosts[0].snapshot = Snapshot::default();
                    app.sync(cx);
                    app.prepare_docker(window, cx);
                    assert!(app.hosts[0].snapshot.sessions.is_empty());
                    assert_eq!(app.page, Page::Docker);
                    assert_eq!(app.docker.read(cx).selected_target(), target);
                    app.hosts[0].backend = backend;
                    app.sync(cx);
                });
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(1.), px(1.)),
                        ..Default::default()
                    }),
                    cx,
                );
                window.refresh();
                let _ = window.draw(cx);
                app.read(cx).docker.read(cx).check_layout(cx);
                check_docker_feedback(&app, window, cx, &format!("docker-{_name}-{tab}"))?;
                #[cfg(feature = "ui-check-screenshots")]
                if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
                    window
                        .render_to_image()?
                        .save(PathBuf::from(directory).join(format!("docker-{_name}-{tab}.png")))?;
                }
                Ok::<_, anyhow::Error>(())
            })??;
        }
        for resource in 0..11 {
            cx.update_window(handle.into(), |_, window, cx| {
                window.dispatch_action(Box::new(gpui_kit::component::dialog::Cancel), cx);
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(1.), px(1.)),
                        ..Default::default()
                    }),
                    cx,
                );
                app.update(cx, |app, cx| {
                    app.page = Page::Docker;
                    app.sidebar_collapsed = false;
                    app.docker.update(cx, |docker, cx| {
                        docker.check_resource_fixture(resource, window, cx)
                    });
                    app.switch_host(remote, cx);
                    app.prepare_docker(window, cx);
                    assert!(app.hosts[remote].backend.is_none());
                    app.docker.update(cx, |docker, cx| {
                        docker.check_resource_fixture(resource, window, cx)
                    });
                    app.switch_host(0, cx);
                    app.prepare_docker(window, cx);
                    app.docker.read(cx).check_resource_state(resource, cx);
                });
                window.refresh();
                let _ = window.draw(cx);
                app.read(cx).docker.read(cx).check_layout(cx);
                check_docker_feedback(
                    &app,
                    window,
                    cx,
                    &format!("docker-{_name}-resource-{resource}"),
                )?;
                let position = point(px(420.), px(if resource == 5 { 80. } else { 400. }));
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                if resource == 2 {
                    app.read(cx)
                        .docker
                        .clone()
                        .update(cx, |docker, cx| docker.check_compact_sidebar(true, cx));
                    window.refresh();
                    let _ = window.draw(cx);
                    let focus = app
                        .read(cx)
                        .docker
                        .clone()
                        .update(cx, |docker, cx| docker.check_image_tree_focus(window, cx));
                    window.refresh();
                    let _ = window.draw(cx);
                    focus.focus(window, cx);
                    window.dispatch_keystroke(Keystroke::parse("down").unwrap(), cx);
                    app.read(cx).docker.read(cx).check_image_selection(0);
                    window.refresh();
                    let _ = window.draw(cx);
                    window.dispatch_keystroke(Keystroke::parse("down").unwrap(), cx);
                    app.read(cx).docker.read(cx).check_image_selection(1);
                    window.refresh();
                    let _ = window.draw(cx);
                    window.dispatch_keystroke(Keystroke::parse("up").unwrap(), cx);
                    app.read(cx).docker.read(cx).check_image_selection(0);
                }
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(1.), px(1.)),
                        ..Default::default()
                    }),
                    cx,
                );
                window.refresh();
                let _ = window.draw(cx);
                Ok::<_, anyhow::Error>(())
            })??;
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            cx.update_window(handle.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
                #[cfg(feature = "ui-check-screenshots")]
                if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
                    window.render_to_image()?.save(
                        PathBuf::from(directory)
                            .join(format!("docker-{_name}-resource-{resource}.png")),
                    )?;
                }
                Ok::<_, anyhow::Error>(())
            })??;
        }
        cx.update_window(handle.into(), |_, window, cx| {
            app.read(cx).docker.clone().update(cx, |docker, cx| {
                docker.check_action_form(window, cx);
            });
            window.refresh();
            let _ = window.draw(cx);
            #[cfg(feature = "ui-check-screenshots")]
            if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
                window.render_to_image()?.save(
                    PathBuf::from(directory).join(format!("docker-{_name}-action-form.png")),
                )?;
            }
            window.close_dialog(cx);
            Ok::<_, anyhow::Error>(())
        })??;
        for collection in 0..5 {
            cx.update_window(handle.into(), |_, window, cx| {
                let focus = app.update(cx, |app, cx| {
                    app.sidebar_collapsed = false;
                    app.docker.update(cx, |docker, cx| {
                        docker.check_browser_fixture(collection, window, cx)
                    })
                });
                window.refresh();
                let _ = window.draw(cx);
                #[cfg(feature = "ui-check-screenshots")]
                if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
                    window.render_to_image()?.save(
                        PathBuf::from(directory)
                            .join(format!("docker-{_name}-table-{collection}.png")),
                    )?;
                }
                focus.focus(window, cx);
                check_docker_feedback(
                    &app,
                    window,
                    cx,
                    &format!("docker-{_name}-table-{collection}"),
                )?;
                window.dispatch_keystroke(Keystroke::parse("home").unwrap(), cx);
                app.read(cx)
                    .docker
                    .read(cx)
                    .check_browser_navigation(collection, false, true);
                if collection == 2 {
                    app.read(cx)
                        .docker
                        .read(cx)
                        .check_image_reference(0, Some("caddy:latest"));
                    window.dispatch_keystroke(Keystroke::parse("end").unwrap(), cx);
                    app.read(cx)
                        .docker
                        .read(cx)
                        .check_image_reference(10_000, None);
                    window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
                    window.refresh();
                    let _ = window.draw(cx);
                    #[cfg(feature = "ui-check-screenshots")]
                    if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
                        window.render_to_image()?.save(
                            PathBuf::from(directory)
                                .join(format!("docker-{_name}-untagged-images.png")),
                        )?;
                    }
                    window.dispatch_keystroke(Keystroke::parse("home").unwrap(), cx);
                    app.read(cx)
                        .docker
                        .read(cx)
                        .check_image_reference(0, Some("caddy:latest"));
                    window.dispatch_keystroke(Keystroke::parse("down").unwrap(), cx);
                    app.read(cx)
                        .docker
                        .read(cx)
                        .check_image_reference(0, Some("caddy:stable"));
                    window.dispatch_keystroke(Keystroke::parse("down").unwrap(), cx);
                    app.read(cx)
                        .docker
                        .read(cx)
                        .check_image_reference(1, Some("redis:7-alpine"));
                    window.dispatch_keystroke(Keystroke::parse("down").unwrap(), cx);
                    app.read(cx)
                        .docker
                        .read(cx)
                        .check_image_reference(1, Some("redis:7.4-alpine"));
                    window.dispatch_keystroke(Keystroke::parse("home").unwrap(), cx);
                }
                window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
                app.read(cx)
                    .docker
                    .read(cx)
                    .check_browser_navigation(collection, false, false);
                window.dispatch_keystroke(Keystroke::parse("home").unwrap(), cx);
                window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
                app.read(cx)
                    .docker
                    .read(cx)
                    .check_browser_navigation(collection, true, true);
                app.read(cx).docker.clone().update(cx, |docker, cx| {
                    docker.check_browser_return(collection, window, cx)
                });
                window.dispatch_keystroke(Keystroke::parse("home").unwrap(), cx);
                app.update(cx, |app, cx| {
                    app.switch_host(remote, cx);
                    app.prepare_docker(window, cx);
                    assert!(app.hosts[remote].backend.is_none());
                    app.switch_host(0, cx);
                    app.prepare_docker(window, cx);
                });
                app.read(cx)
                    .docker
                    .read(cx)
                    .check_browser_navigation(collection, false, true);
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(1.), px(1.)),
                        ..Default::default()
                    }),
                    cx,
                );
                window.refresh();
                let _ = window.draw(cx);
                #[cfg(feature = "ui-check-screenshots")]
                if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
                    window.render_to_image()?.save(
                        PathBuf::from(directory)
                            .join(format!("docker-{_name}-table-detail-{collection}.png")),
                    )?;
                }
                Ok::<_, anyhow::Error>(())
            })??;
        }
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.sidebar_collapsed = true;
                cx.notify();
            });
            window.refresh();
            let _ = window.draw(cx);
        })?;
    }
    docker::check_terminal(handle, app.clone(), remote, cx).await?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.active = previous;
            app.hosts.remove(remote);
            app.page = Page::Terminal;
            app.sync(cx);
            app.prepare_docker(window, cx);
        });
    })?;
    Ok(())
}
