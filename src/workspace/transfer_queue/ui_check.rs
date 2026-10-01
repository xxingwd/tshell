use super::*;

fn sample_jobs() -> Vec<TransferJob> {
    let remote = remote_files::Session::new(HostConfig {
        name: "production".into(),
        destination: "example.invalid".into(),
        user: "test".into(),
        port: None,
        identity_file: None,
        tmux: false,
        socket: None,
    });
    [
        (
            "backup.tar.gz",
            TransferState::Failed,
            0,
            Some("Permission denied: /srv/archive/backup.tar.gz"),
        ),
        ("config.toml", TransferState::Done, 4096, None),
        ("logs-2026-09", TransferState::Queued, 0, None),
        (
            "release-assets",
            TransferState::Running,
            12 * 1024 * 1024,
            None,
        ),
        (
            "tshell-0.1.1-windows-x86_64.zip",
            TransferState::Running,
            31 * 1024 * 1024,
            None,
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (name, state, transferred, error))| TransferJob {
        id: index as u64,
        host: "production".into(),
        session: None,
        name: name.into(),
        request: if index == 3 {
            TransferRequest::Upload {
                remote: remote.clone(),
                sources: vec![name.into()],
                target: "/srv/releases".into(),
            }
        } else {
            TransferRequest::Download {
                remote: remote.clone(),
                source: format!("/srv/releases/{name}").into(),
                destination: name.into(),
            }
        },
        control: remote_files::TransferControl::new(),
        progress: remote_files::TransferProgress {
            scanning: false,
            transferred,
            total: if index == 1 {
                Some(4096)
            } else if index >= 3 {
                Some(64 * 1024 * 1024)
            } else {
                None
            },
            current_file: if index == 3 {
                "assets/themes/vscode-modern-dark.json".into()
            } else {
                String::new()
            },
        },
        state,
        error: error.map(str::to_owned),
        started: None,
        rate: {
            let mut rate = TransferRate::default();
            rate.sample(Duration::ZERO, 0);
            rate.sample(Duration::from_secs(3), transferred);
            rate
        },
    })
    .collect()
}

pub(in crate::workspace) async fn check_transfer_panel(
    handle: WindowHandle<Root>,
    app: Entity<AppView>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let draw = |cx: &mut AsyncApp| -> anyhow::Result<()> {
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })?;
        Ok(())
    };
    let send = |event, cx: &mut AsyncApp| -> anyhow::Result<()> {
        cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_event(event, cx);
        })?;
        Ok(())
    };
    let click = |position, cx: &mut AsyncApp| -> anyhow::Result<()> {
        send(
            PlatformInput::MouseMove(MouseMoveEvent {
                position,
                ..Default::default()
            }),
            cx,
        )?;
        send(
            PlatformInput::MouseDown(MouseDownEvent {
                position,
                click_count: 1,
                ..Default::default()
            }),
            cx,
        )?;
        send(
            PlatformInput::MouseUp(MouseUpEvent {
                position,
                click_count: 1,
                ..Default::default()
            }),
            cx,
        )
    };
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.active = 0;
            app.workspace_mode = WorkspaceMode::Terminal;
            app.transfer_queue = TransferQueue::default();
            app.transfer_queue.jobs = sample_jobs();
            app.transfer_queue.panel_open = true;
            app.themes = ThemeFile::default();
            app.light_theme = "vscode-light".into();
            app.dark_theme = "vscode-dark".into();
            app.background_opacity = 1.;
            app.sidebar_opacity = 1.;
            app.apply_appearance(window, cx);
            cx.notify();
        });
    })?;
    for (name, appearance, viewport) in [
        ("dark", Appearance::Dark, size(px(1320.), px(840.))),
        ("light-small", Appearance::Light, size(px(860.), px(520.))),
    ] {
        handle.update(cx, |root, window, cx| {
            root.style().size.width = Some(viewport.width.into());
            root.style().size.height = Some(viewport.height.into());
            window.resize(viewport);
            app.update(cx, |app, cx| app.set_appearance(appearance, window, cx));
        })?;
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
        draw(cx)?;
        anyhow::ensure!(
            app.read_with(cx, |app, _| app.terminal_palette.terminal_theme().light())
                == (appearance == Appearance::Light),
            "transfer screenshot did not use the requested appearance"
        );
        #[cfg(feature = "ui-check-screenshots")]
        capture(handle, name, cx)?;
        #[cfg(not(feature = "ui-check-screenshots"))]
        let _ = name;
    }
    let bounds = cx.update_window(handle.into(), |_, window, cx| {
        app.read(cx)
            .transfer_queue
            .panel_bounds(window.viewport_size())
    })?;
    let origin = bounds.origin + point(px(80.), px(20.));
    let expected_offset = cx.update_window(handle.into(), |_, window, cx| {
        let queue = &app.read(cx).transfer_queue;
        queue.clamped_offset(
            queue.offset + point(px(180.), px(70.)),
            window.viewport_size(),
        )
    })?;
    for (name, delta) in [("error", -10.), ("restored", 10.)] {
        send(
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: bounds.center(),
                delta: ScrollDelta::Lines(point(0., delta)),
                ..Default::default()
            }),
            cx,
        )?;
        draw(cx)?;
        #[cfg(feature = "ui-check-screenshots")]
        if name == "error" {
            capture(handle, name, cx)?;
        }
        #[cfg(not(feature = "ui-check-screenshots"))]
        let _ = name;
    }
    send(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: origin,
            ..Default::default()
        }),
        cx,
    )?;
    draw(cx)?;
    send(
        PlatformInput::MouseDown(MouseDownEvent {
            position: origin,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    let target = origin - point(px(180.), px(70.));
    send(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: target,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        }),
        cx,
    )?;
    draw(cx)?;
    send(
        PlatformInput::MouseUp(MouseUpEvent {
            position: target,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    app.read_with(cx, |app, _| {
        anyhow::ensure!(
            app.transfer_queue.offset == expected_offset,
            "transfer header did not drag to the clamped position: {:?} != {:?}",
            app.transfer_queue.offset,
            expected_offset
        );
        anyhow::ensure!(
            app.transfer_queue.drag.is_none(),
            "transfer drag was not released"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    draw(cx)?;
    let bounds = cx.update_window(handle.into(), |_, window, cx| {
        app.read(cx)
            .transfer_queue
            .panel_bounds(window.viewport_size())
    })?;
    click(point(bounds.right() - px(21.), bounds.top() + px(20.)), cx)?;
    draw(cx)?;
    anyhow::ensure!(
        !app.read_with(cx, |app, _| app.transfer_queue.panel_open),
        "collapse button did not respond"
    );
    #[cfg(feature = "ui-check-screenshots")]
    capture(handle, "compact", cx)?;
    let compact = cx.update_window(handle.into(), |_, window, cx| {
        app.read(cx)
            .transfer_queue
            .panel_bounds(window.viewport_size())
    })?;
    click(compact.center(), cx)?;
    draw(cx)?;
    anyhow::ensure!(
        app.read_with(cx, |app, _| app.transfer_queue.panel_open),
        "compact launcher did not expand"
    );
    let bounds = cx.update_window(handle.into(), |_, window, cx| {
        app.read(cx)
            .transfer_queue
            .panel_bounds(window.viewport_size())
    })?;
    click(
        point(
            bounds.right() - px(26.),
            bounds.top() + px(PANEL_HEADER_HEIGHT + 22.),
        ),
        cx,
    )?;
    draw(cx)?;
    app.read_with(cx, |app, _| {
        let job = app
            .transfer_queue
            .jobs
            .iter()
            .find(|job| job.id == 4)
            .unwrap();
        anyhow::ensure!(
            job.state == TransferState::Cancelling && job.control.is_cancelled(),
            "cancel button did not cancel its task"
        );
        anyhow::ensure!(
            app.transfer_queue
                .jobs
                .iter()
                .find(|job| job.id == 2)
                .unwrap()
                .state
                == TransferState::Queued,
            "cancelling started a third concurrent transfer"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    click(
        point(
            bounds.right() - px(26.),
            bounds.top() + px(PANEL_HEADER_HEIGHT + TRANSFER_ROW_HEIGHT * 3. + 22.),
        ),
        cx,
    )?;
    draw(cx)?;
    anyhow::ensure!(
        app.read_with(cx, |app, _| !app
            .transfer_queue
            .jobs
            .iter()
            .any(|job| job.id == 1)),
        "single-task clear did not respond"
    );
    let pointer = bounds.origin + point(px(80.), px(20.));
    send(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: pointer,
            ..Default::default()
        }),
        cx,
    )?;
    draw(cx)?;
    cx.background_executor()
        .timer(PANEL_AUTO_COLLAPSE + Duration::from_millis(100))
        .await;
    anyhow::ensure!(
        app.read_with(cx, |app, _| app.transfer_queue.panel_open),
        "hover did not hold transfer panel open"
    );
    send(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(20.), px(100.)),
            ..Default::default()
        }),
        cx,
    )?;
    draw(cx)?;
    cx.background_executor()
        .timer(PANEL_AUTO_COLLAPSE + Duration::from_millis(100))
        .await;
    anyhow::ensure!(
        !app.read_with(cx, |app, _| app.transfer_queue.panel_open),
        "transfer panel did not auto-collapse after pointer left"
    );
    app.update(cx, |app, cx| {
        app.transfer_queue = Default::default();
        cx.notify();
    });
    Ok(())
}

#[cfg(feature = "ui-check-screenshots")]
fn capture(handle: WindowHandle<Root>, name: &str, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") else {
        return Ok(());
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory)?;
    cx.update_window(handle.into(), |_, window, _| {
        window
            .render_to_image()?
            .save(directory.join(format!("transfers-{name}.png")))?;
        Ok::<_, anyhow::Error>(())
    })??;
    Ok(())
}
