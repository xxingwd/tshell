use super::*;

pub(super) async fn check(
    handle: WindowHandle<Root>,
    app: Entity<AppView>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let (index, previous_active) = cx.update_window(handle.into(), |_, _, cx| {
        app.update(cx, |app, _| {
            let index = app.hosts.len();
            for (name, destination) in [
                ("Development", "fixture.invalid"),
                ("Database", "database.invalid"),
            ] {
                let config = HostConfig {
                    destination: destination.into(),
                    name: name.into(),
                    user: "deploy".into(),
                    port: Some(2222),
                    identity_file: None,
                    tmux: false,
                    socket: None,
                };
                app.hosts.push(Host {
                    pending: Vec::new(),
                    name: config.name.clone(),
                    config: Some(config),
                    backend: None,
                    snapshot: Snapshot::default(),
                    views: BTreeMap::new(),
                    read_notices: BTreeMap::new(),
                    collapsed_sessions: BTreeSet::new(),
                    viewport: None,
                });
            }
            let previous_active = app.active;
            app.active = 0;
            (index, previous_active)
        })
    })?;
    for (_name, appearance, viewport) in [
        ("dark", Appearance::Dark, size(px(1320.), px(840.))),
        ("light-small", Appearance::Light, size(px(860.), px(520.))),
    ] {
        handle.update(cx, |root, window, _| {
            root.style().size.width = Some(viewport.width.into());
            root.style().size.height = Some(viewport.height.into());
            window.resize(viewport);
        })?;
        for page in ["summary", "forward", "forward-add", "forward-socks"] {
            cx.update_window(handle.into(), |_, window, cx| -> anyhow::Result<()> {
                app.update(cx, |app, cx| -> anyhow::Result<()> {
                    app.close_modal(window, cx);
                    app.select_theme(ThemeMode::Light, "vscode-light", window, cx);
                    app.select_theme(ThemeMode::Dark, "vscode-dark", window, cx);
                    app.set_appearance(appearance, window, cx);
                    match page {
                        "summary" => {
                            anyhow::ensure!(
                                app.forward_status_button(cx).is_some(),
                                "forward summary missing"
                            );
                        }
                        "forward" => {
                            app.open_forward_editor(window, cx);
                            app.forward_host_index = Some(index);
                            app.forward_kind = crate::ssh_forward::ForwardKind::Dynamic;
                            app.add_forward(window, cx);
                            app.forward_kind = crate::ssh_forward::ForwardKind::Local;
                            app.add_forward(window, cx);
                            app.forward_host_index = Some(index + 1);
                            app.forward_kind = crate::ssh_forward::ForwardKind::Remote;
                            app.add_forward(window, cx);
                            anyhow::ensure!(!app.forward_add_form, "add form did not collapse");
                            let key =
                                forward_storage_key(app.hosts[index].config.as_ref().unwrap());
                            anyhow::ensure!(
                                app.forwards[&key].len() == 2,
                                "first host rules missing"
                            );
                            let other_key =
                                forward_storage_key(app.hosts[index + 1].config.as_ref().unwrap());
                            anyhow::ensure!(
                                app.forwards[&other_key].len() == 1,
                                "second host rule missing"
                            );
                            anyhow::ensure!(
                                app.forward_processes.contains_key(&(other_key, 0)),
                                "saved rule did not start"
                            );
                        }
                        "forward-add" | "forward-socks" => {
                            app.open_forward_editor(window, cx);
                            app.forward_host_index = Some(index);
                            app.forward_add_form = true;
                            app.refresh_forward_dialog_height();
                            app.forward_kind = if page == "forward-socks" {
                                crate::ssh_forward::ForwardKind::Dynamic
                            } else {
                                crate::ssh_forward::ForwardKind::Local
                            };
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                })
            })??;
            cx.background_executor()
                .timer(Duration::from_millis(40))
                .await;
            cx.update_window(handle.into(), |_, window, cx| -> anyhow::Result<()> {
                window.refresh();
                let _ = window.draw(cx);
                #[cfg(feature = "ui-check-screenshots")]
                if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
                    let directory = PathBuf::from(directory);
                    std::fs::create_dir_all(&directory)?;
                    window
                        .render_to_image()?
                        .save(directory.join(format!("ssh-{_name}-{page}.png")))?;
                }
                Ok(())
            })??;
        }
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.close_modal(window, cx);
                app.open_forward_editor(window, cx);
            });
        })?;
        let left = (viewport.width - px(780.)) / 2.;
        let click = |position, cx: &mut AsyncApp| -> anyhow::Result<()> {
            cx.update_window(handle.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        position,
                        button: MouseButton::Left,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        position,
                        button: MouseButton::Left,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
            })?;
            Ok(())
        };
        click(point(left + px(736.), px(126.)), cx)?;
        anyhow::ensure!(
            app.read_with(cx, |app, _| app.forward_add_form),
            "Add click did not open the draft row"
        );
        let count = app.read_with(cx, |app, _| {
            app.forwards.values().map(Vec::len).sum::<usize>()
        });
        click(point(left + px(742.), px(190. + count as f32 * 48.)), cx)?;
        anyhow::ensure!(
            app.read_with(cx, |app, _| app.forward_form && !app.forward_add_form),
            "Cancel draft click did not return to the forwarding list"
        );
        click(point(left + px(758.), px(94.)), cx)?;
        cx.update_window(handle.into(), |_, window, cx| {
            anyhow::ensure!(
                !window.has_active_dialog(cx) && !app.read(cx).forward_form,
                "Dialog X did not close forwarding after cancelling Add"
            );
            Ok::<_, anyhow::Error>(())
        })??;
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.open_forward_editor(window, cx);
                app.forward_host_index = Some(index);
            });
        })?;
        click(point(left + px(736.), px(126.)), cx)?;
        anyhow::ensure!(
            app.read_with(cx, |app, _| app.forward_add_form),
            "Add click did not reopen the draft row"
        );
        click(point(left + px(696.), px(190. + count as f32 * 48.)), cx)?;
        anyhow::ensure!(
            app.read_with(cx, |app, _| app.forward_form && !app.forward_add_form),
            "Save click did not return to the forwarding list"
        );
        click(point(left + px(758.), px(94.)), cx)?;
        cx.update_window(handle.into(), |_, window, cx| {
            anyhow::ensure!(
                !window.has_active_dialog(cx) && !app.read(cx).forward_form,
                "Dialog X did not close forwarding after saving Add"
            );
            Ok::<_, anyhow::Error>(())
        })??;
    }
    cx.update_window(handle.into(), |_, window, cx| -> anyhow::Result<()> {
        app.update(cx, |app, cx| -> anyhow::Result<()> {
            app.close_modal(window, cx);
            window.close_all_dialogs(cx);
            let first_key = forward_storage_key(app.hosts[index].config.as_ref().unwrap());
            let second_key = forward_storage_key(app.hosts[index + 1].config.as_ref().unwrap());
            app.remove_forward(index, 0, cx);
            anyhow::ensure!(
                app.forwards[&first_key].len() == 1,
                "wrong first-host rule removed"
            );
            anyhow::ensure!(
                app.forward_processes.contains_key(&(first_key.clone(), 0)),
                "remaining tunnel lost during reindex"
            );
            anyhow::ensure!(
                app.forward_processes.contains_key(&(second_key.clone(), 0)),
                "other host tunnel stopped"
            );
            app.stop_forwards_for(&app.hosts[index + 1].config.as_ref().unwrap().clone());
            app.toggle_forward(index + 1, 0, window, cx);
            anyhow::ensure!(
                app.forward_processes.contains_key(&(second_key.clone(), 0)),
                "row did not restart"
            );
            for host_index in [index + 1, index] {
                let config = app.hosts[host_index].config.as_ref().unwrap().clone();
                app.stop_forwards_for(&config);
                app.forwards.remove(&forward_storage_key(&config));
                app.hosts.remove(host_index);
            }
            app.active = previous_active;
            app.set_appearance(Appearance::Dark, window, cx);
            Ok(())
        })
    })??;
    Ok(())
}
