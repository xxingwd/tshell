use super::*;

struct OrderingProbe {
    owner: Entity<AppView>,
}

impl Render for OrderingProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (palette, list) = self
            .owner
            .update(cx, |app, cx| (app.palette, app.session_list(cx)));
        div()
            .w(px(212.))
            .h(px(240.))
            .bg(rgb(palette.panel))
            .child(list)
    }
}

struct HostOrderingProbe {
    owner: Entity<AppView>,
}

impl Render for HostOrderingProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (palette, list) = self
            .owner
            .update(cx, |app, cx| (app.palette, app.host_settings(cx)));
        div()
            .w(px(640.))
            .h(px(240.))
            .bg(rgb(palette.panel))
            .child(list)
    }
}

pub(in crate::workspace) async fn check(
    app: Entity<AppView>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let original = app.read_with(cx, |app, _| {
        (
            app.active,
            app.host_order.clone(),
            app.session_order.clone(),
        )
    });
    let host_id = tshell_host::HostId::default();
    let sessions: Vec<_> = ["Alpha", "Beta", "Gamma"]
        .iter()
        .enumerate()
        .map(|(index, name)| SessionInfo {
            id: format!("${index}"),
            order_id: format!("tmux:${index}:100"),
            name: (*name).into(),
            ..Default::default()
        })
        .collect();
    app.update(cx, |app, cx| {
        let config = HostDefinition {
            id: host_id.clone(),
            name: "Ordering fixture".into(),
            tmux: true,
            socket: None,
            ssh: tshell_host::SshConfig {
                destination: "example.invalid".into(),
                user: "test".into(),
                port: None,
                identity_file: None,
            },
        };
        app.hosts.push(Host {
            name: config.name.clone(),
            config: Some(config),
            backend: None,
            snapshot: Snapshot {
                sessions: sessions.clone(),
                connection: Connection::Ready,
                ..Default::default()
            },
            pending: Vec::new(),
            views: BTreeMap::new(),
            read_notices: BTreeMap::new(),
            collapsed_sessions: BTreeSet::new(),
            viewport: None,
        });
        app.active = app.hosts.len() - 1;
        let index = app.active;
        for _ in 0..index {
            app.move_host(index, -1, cx);
        }
        assert_eq!(app.ordered_host_indices()[0], index);
        assert_eq!(app.active, index);
        assert_eq!(Preferences::load().host_order[0], host_id.to_string());
        app.host_order = original.1.clone();
        cx.notify();
    });
    let host_probe = cx.open_window(
        WindowOptions {
            show: false,
            focus: false,
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(640.), px(240.)),
            ))),
            ..Default::default()
        },
        |window, cx| {
            window.resize(size(px(640.), px(240.)));
            cx.new(|_| HostOrderingProbe { owner: app.clone() })
        },
    )?;
    cx.background_executor()
        .timer(Duration::from_millis(25))
        .await;
    for (position, expected) in [
        (point(px(466.), px(72.)), vec![1, 0]),
        (point(px(506.), px(24.)), vec![0, 1]),
    ] {
        cx.update_window(host_probe.into(), |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    ..Default::default()
                }),
                cx,
            );
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
        })?;
        app.read_with(cx, |app, _| {
            anyhow::ensure!(
                app.ordered_host_indices() == expected,
                "native host up/down button did not reorder"
            );
            anyhow::ensure!(app.active == 1, "host reorder changed active host");
            Ok::<_, anyhow::Error>(())
        })?;
    }
    #[cfg(feature = "ui-check-screenshots")]
    if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
        cx.update_window(host_probe.into(), |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(1.), px(200.)),
                    ..Default::default()
                }),
                cx,
            );
            window.refresh();
            let _ = window.draw(cx);
            window
                .render_to_image()?
                .save(PathBuf::from(directory).join("host-order-controls.png"))?;
            Ok::<_, anyhow::Error>(())
        })??;
    }
    cx.update_window(host_probe.into(), |_, window, _| window.remove_window())?;
    let probe = cx.open_window(
        WindowOptions {
            show: false,
            focus: false,
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(212.), px(240.)),
            ))),
            ..Default::default()
        },
        |window, cx| {
            window.resize(size(px(212.), px(240.)));
            cx.new(|_| OrderingProbe { owner: app.clone() })
        },
    )?;
    cx.background_executor()
        .timer(Duration::from_millis(25))
        .await;
    let draw = |cx: &mut AsyncApp| -> anyhow::Result<()> {
        cx.update_window(probe.into(), |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })?;
        Ok(())
    };
    let send = |event, cx: &mut AsyncApp| -> anyhow::Result<()> {
        cx.update_window(probe.into(), |_, window, cx| {
            window.dispatch_event(event, cx);
        })?;
        Ok(())
    };
    draw(cx)?;
    for (from, to, expected) in [
        (16., 82., vec!["$1", "$2", "$0"]),
        (82., 16., vec!["$0", "$1", "$2"]),
    ] {
        let origin = point(px(60.), px(from));
        let destination = point(px(60.), px(to));
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
        send(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: origin + point(px(8.), px(0.)),
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }),
            cx,
        )?;
        draw(cx)?;
        send(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: destination,
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }),
            cx,
        )?;
        draw(cx)?;
        #[cfg(feature = "ui-check-screenshots")]
        if from < to
            && let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR")
        {
            cx.update_window(probe.into(), |_, window, _| {
                window
                    .render_to_image()?
                    .save(PathBuf::from(directory).join("session-reorder-drag.png"))?;
                Ok::<_, anyhow::Error>(())
            })??;
        }
        send(
            PlatformInput::MouseUp(MouseUpEvent {
                position: destination,
                click_count: 1,
                ..Default::default()
            }),
            cx,
        )?;
        draw(cx)?;
        app.read_with(cx, |app, _| {
            anyhow::ensure!(
                app.hosts[app.active].collapsed_sessions.is_empty(),
                "session drag also toggled collapse"
            );
            anyhow::ensure!(
                app.hosts[app.active]
                    .snapshot
                    .sessions
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    == expected,
                "native session drag did not reorder: from={from}, to={to}"
            );
            let prefs = Preferences::load();
            let mut refreshed = sessions.clone();
            apply_session_order(&mut refreshed, &prefs.session_order[&host_id.to_string()]);
            anyhow::ensure!(
                refreshed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>() == expected,
                "session refresh lost persisted drag order"
            );
            Ok::<_, anyhow::Error>(())
        })?;
    }
    app.update(cx, |app, cx| {
        app.move_session_relative(&host_id, "$2", -1, cx);
        assert_eq!(app.hosts[app.active].snapshot.sessions[1].id, "$2");
        let host_key = host_id.to_string();
        let saved = app.session_order[&host_key].clone();
        app.hosts[app.active].snapshot.connection = Connection::Closed;
        app.sync(cx);
        assert_eq!(app.session_order[&host_key], saved);
        let root = std::env::temp_dir();
        let profiles = [0, 2]
            .into_iter()
            .map(|index| crate::backend::SessionProfile {
                id: sessions[index].order_id.clone(),
                name: sessions[index].name.clone(),
                path: root.to_string_lossy().into_owned(),
            })
            .collect();
        app.hosts[app.active].backend = Some(Backend::Local(
            LocalBackend::restore(root, None, Some(profiles)).unwrap(),
        ));
        app.sync(cx);
        let surviving: Vec<_> = saved
            .iter()
            .filter(|key| *key != &sessions[1].order_id)
            .cloned()
            .collect();
        assert_eq!(app.session_order[&host_key], surviving);
        assert_eq!(Preferences::load().session_order[&host_key], surviving);
        app.hosts[app.active].backend = Some(Backend::Local(
            LocalBackend::restore(std::env::temp_dir(), None, Some(Vec::new())).unwrap(),
        ));
        app.sync(cx);
        assert!(!app.session_order.contains_key(&host_key));
        assert!(!Preferences::load().session_order.contains_key(&host_key));
        app.session_order.insert(host_key.clone(), saved);
        app.host_order.push(host_key.clone());
        let removed = app.active;
        app.remove_host(removed, cx);
        assert!(!app.host_order.contains(&host_key));
        assert!(!app.session_order.contains_key(&host_key));
        let prefs = Preferences::load();
        assert!(!prefs.host_order.contains(&host_key));
        assert!(!prefs.session_order.contains_key(&host_key));
        app.active = original.0;
        app.host_order = original.1;
        app.session_order = original.2;
        app.save(cx);
        app.sync(cx);
    });
    cx.update_window(probe.into(), |_, window, _| window.remove_window())?;
    Ok(())
}
