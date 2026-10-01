use super::*;
use gpui_kit::base::TextSelection;
use std::time::Instant;

pub(super) async fn check(
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
    let pointer =
        |position, pressed: Option<MouseButton>, cx: &mut AsyncApp| -> anyhow::Result<()> {
            cx.update_window(handle.into(), |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: pressed,
                        ..Default::default()
                    }),
                    cx,
                );
            })?;
            Ok(())
        };
    let button = |position, down, cx: &mut AsyncApp| -> anyhow::Result<()> {
        cx.update_window(handle.into(), |_, window, cx| {
            let event = if down {
                PlatformInput::MouseDown(MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 1,
                    ..Default::default()
                })
            } else {
                PlatformInput::MouseUp(MouseUpEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 1,
                    ..Default::default()
                })
            };
            window.dispatch_event(event, cx);
        })?;
        Ok(())
    };
    handle.update(cx, |root, window, _| {
        root.style().size.width = Some(px(1320.).into());
        root.style().size.height = Some(px(840.).into());
        window.resize(size(px(1320.), px(840.)));
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(30))
        .await;
    let identity = app.update(cx, |app, cx| {
        app.switch_host(0, cx);
        app.sync(cx);
        app.editor_dirty = false;
        let host = &app.hosts[0];
        host.backend
            .as_ref()
            .unwrap()
            .screen(&host.snapshot.window().unwrap().active_pane)
            .unwrap()
            .identity
    });
    let original = app.read_with(cx, |app, _| {
        app.hosts[0].snapshot.window().unwrap().active_pane.clone()
    });
    app.update(cx, |app, cx| app.act(Action::NewWindow, cx));
    let other_tab = app.read_with(cx, |app, _| {
        app.hosts[0]
            .snapshot
            .session()
            .unwrap()
            .active_window
            .clone()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.focus_terminal_notice(Some(identity), window, cx)
        });
    })?;
    app.read_with(cx, |app, _| {
        anyhow::ensure!(
            app.hosts[0].snapshot.window().unwrap().active_pane == original,
            "notification selected the wrong pane"
        );
        anyhow::ensure!(
            app.workspace_mode == WorkspaceMode::Terminal,
            "notification did not open terminal"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    app.update(cx, |app, cx| {
        app.act(Action::SelectWindow(other_tab), cx);
        app.act(Action::CloseWindow, cx);
    });
    let before_closed = app.read_with(cx, |app, _| {
        app.hosts[0].snapshot.window().unwrap().active_pane.clone()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.focus_terminal_notice(Some(u64::MAX), window, cx)
        });
    })?;
    app.read_with(cx, |app, _| {
        anyhow::ensure!(
            app.hosts[0].snapshot.window().unwrap().active_pane == before_closed,
            "closed notification changed selection"
        );
        Ok::<_, anyhow::Error>(())
    })?;

    let directory = PathBuf::from(std::env::var_os("TSHELL_DATA_DIR").unwrap());
    let path = directory.join("link-check.rs");
    std::fs::write(&path, "zero\n中文 source\nlast")?;
    let open_link =
        |path: &std::path::Path, line, column, cx: &mut AsyncApp| -> anyhow::Result<()> {
            cx.update_window(handle.into(), |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.open_terminal_link(
                        crate::terminal::links::OpenLink {
                            identity,
                            directory: None,
                            target: crate::terminal::links::LinkTarget::File {
                                path: path.to_string_lossy().into_owned(),
                                host: None,
                                line,
                                column,
                            },
                        },
                        window,
                        cx,
                    )
                });
            })?;
            Ok(())
        };
    open_link(&path, Some(2), Some(3), cx)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    while app.read_with(cx, |app, _| app.file_loading.is_some()) {
        anyhow::ensure!(
            Instant::now() < deadline,
            "terminal file link did not finish opening"
        );
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    draw(cx)?;
    app.read_with(cx, |app, cx| {
        anyhow::ensure!(
            app.open_file.as_ref() == Some(&path) && app.workspace_mode == WorkspaceMode::Files,
            "file link did not open the editor"
        );
        anyhow::ensure!(
            app.file_editor.read(cx).cursor_position()
                == gpui_kit::component::input::Position::new(1, 2),
            "file link lost its line/column"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.file_editor.update(cx, |editor, cx| {
                editor.set_value("unsaved link draft", window, cx)
            });
            app.editor_dirty = true;
        });
    })?;
    open_link(&path, Some(1), Some(1), cx)?;
    draw(cx)?;
    app.read_with(cx, |app, cx| {
        anyhow::ensure!(
            app.file_editor.read(cx).value().as_ref() == "unsaved link draft" && app.editor_dirty,
            "same-file link replaced unsaved edits"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    open_link(&directory, None, None, cx)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    while app.read_with(cx, |app, _| app.file_loading.is_some()) {
        anyhow::ensure!(
            Instant::now() < deadline,
            "directory link did not finish opening"
        );
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    app.read_with(cx, |app, cx| {
        anyhow::ensure!(
            app.cwd == directory
                && app.file_editor.read(cx).value().as_ref() == "unsaved link draft",
            "directory link lost the root or file draft"
        );
        Ok::<_, anyhow::Error>(())
    })?;
    app.update(cx, |app, _| app.editor_dirty = false);

    for mode in [git_view::DiffMode::Inline, git_view::DiffMode::SideBySide] {
        app.update(cx, |app, cx| {
            app.workspace_mode = WorkspaceMode::Git;
            app.git_diff_mode = mode;
            app.git_diff_compact = false;
            app.git_diff = Some(git_view::DiffView {
                change: git::GitChange {
                    status: " M".into(),
                    path: app.cwd.join("selection.rs"),
                    original: None,
                    root: app.cwd.clone(),
                    stats: None,
                },
                content: Ok(git_view::DiffContent::new(
                    (0..300)
                        .flat_map(|index| {
                            [
                                git::DiffLine {
                                    old: Some(index + 1),
                                    new: None,
                                    kind: git::LineKind::Removed,
                                    text: format!("-\told 中文 {index}"),
                                },
                                git::DiffLine {
                                    old: None,
                                    new: Some(index + 1),
                                    kind: git::LineKind::Added,
                                    text: format!("+\tnew 中文 {index}"),
                                },
                            ]
                        })
                        .collect(),
                )),
                scroll: UniformListScrollHandle::new(),
                horizontal: std::array::from_fn(|_| ScrollHandle::new()),
                selection: std::array::from_fn(|_| git_selection::DiffSelection::new(cx)),
            });
            cx.notify();
        });
        draw(cx)?;
        let panes = if mode == git_view::DiffMode::Inline {
            vec![2]
        } else {
            vec![0, 1]
        };
        for pane in panes {
            app.update(cx, |app, cx| {
                app.git_diff
                    .as_ref()
                    .unwrap()
                    .scroll
                    .scroll_to_item_strict(0, ScrollStrategy::Top);
                cx.notify();
            });
            draw(cx)?;
            let (start, end) = app
                .read_with(cx, |app, cx| {
                    app.git_diff.as_ref().unwrap().selection[pane]
                        .read(cx)
                        .check_points(0)
                })
                .ok_or_else(|| anyhow::anyhow!("diff text did not paint"))?;
            pointer(start, None, cx)?;
            button(start, true, cx)?;
            pointer(end, Some(MouseButton::Left), cx)?;
            button(end, false, cx)?;
            draw(cx)?;
            let expected = if pane == 1 {
                "\tnew 中文 0"
            } else {
                "\told 中文 0"
            };
            let copied = cx.update_window(handle.into(), |_, window, cx| {
                TextSelection::selected_text(window, cx)
            })?;
            let debug = cx.update_window(handle.into(), |_, window, cx| {
                app.read(cx).git_diff.as_ref().unwrap().selection[pane]
                    .read(cx)
                    .check_debug(window, cx)
            })?;
            anyhow::ensure!(
                copied == expected,
                "diff selection includes gutters or loses Unicode/tabs: pane={pane}, copied={copied:?}; {debug}"
            );
            cx.update_window(handle.into(), |_, window, cx| {
                app.update(cx, |app, cx| {
                    let selected = TextSelection::selected_text(window, cx);
                    anyhow::ensure!(selected == expected, "Git copy command lost selection: {selected:?}");
                    app.execute_shortcut(Shortcut::Copy, window, cx);
                    Ok::<_, anyhow::Error>(())
                })?;
                if std::env::var_os("TSHELL_UI_CHECK_SKIP_CLIPBOARD").is_none() {
                    let copied = cx.read_from_clipboard().and_then(|item| item.text());
                    anyhow::ensure!(copied.as_deref() == Some(expected), "diff Copy did not use selected text: copied={copied:?}, expected={expected:?}");
                }
                Ok::<_, anyhow::Error>(())
            })??;

            let position = app
                .read_with(cx, |app, cx| {
                    app.git_diff.as_ref().unwrap().selection[pane]
                        .read(cx)
                        .check_word_point(0)
                })
                .unwrap();
            pointer(position, None, cx)?;
            for count in [2, 3] {
                cx.update_window(handle.into(), |_, window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            position,
                            button: MouseButton::Left,
                            click_count: count,
                            ..Default::default()
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::MouseUp(MouseUpEvent {
                            position,
                            button: MouseButton::Left,
                            click_count: count,
                            ..Default::default()
                        }),
                        cx,
                    );
                })?;
                draw(cx)?;
                let copied = cx.update_window(handle.into(), |_, window, cx| {
                    TextSelection::selected_text(window, cx)
                })?;
                let expected = if count == 3 {
                    expected
                } else if pane == 1 {
                    "new"
                } else {
                    "old"
                };
                anyhow::ensure!(
                    copied == expected,
                    "diff multi-click selection failed: count={count}, copied={copied:?}"
                );
            }

            // Keep the original anchor after its row has left the virtual list.
            pointer(start, None, cx)?;
            button(start, true, cx)?;
            app.update(cx, |app, cx| {
                app.git_diff
                    .as_ref()
                    .unwrap()
                    .scroll
                    .scroll_to_item_strict(100, ScrollStrategy::Top);
                cx.notify();
            });
            draw(cx)?;
            let (_, end) = app
                .read_with(cx, |app, cx| {
                    app.git_diff.as_ref().unwrap().selection[pane]
                        .read(cx)
                        .check_points(100)
                })
                .ok_or_else(|| anyhow::anyhow!("scrolled diff text did not paint"))?;
            pointer(end, Some(MouseButton::Left), cx)?;
            button(end, false, cx)?;
            draw(cx)?;
            let expected = if pane == 2 {
                (0..=100)
                    .map(|index| {
                        format!(
                            "\t{} 中文 {}",
                            if index % 2 == 0 { "old" } else { "new" },
                            index / 2
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                (0..=100)
                    .map(|index| {
                        format!("\t{} 中文 {index}", if pane == 0 { "old" } else { "new" })
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let copied = cx.update_window(handle.into(), |_, window, cx| {
                TextSelection::selected_text(window, cx)
            })?;
            anyhow::ensure!(
                copied == expected,
                "virtual diff selection lost its off-screen anchor: pane={pane}, lines={}",
                copied.lines().count()
            );
            cx.update_window(handle.into(), |_, window, cx| {
                TextSelection::clear(window, cx)
            })?;
        }
        #[cfg(feature = "ui-check-screenshots")]
        if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
            app.update(cx, |app, cx| {
                app.git_diff
                    .as_ref()
                    .unwrap()
                    .scroll
                    .scroll_to_item_strict(0, ScrollStrategy::Top);
                cx.notify();
            });
            draw(cx)?;
            cx.update_window(handle.into(), |_, window, _| {
                window
                    .render_to_image()?
                    .save(
                        PathBuf::from(directory).join(if mode == git_view::DiffMode::Inline {
                            "git-inline-highlight.png"
                        } else {
                            "git-paired-highlight.png"
                        }),
                    )?;
                Ok::<_, anyhow::Error>(())
            })??;
        }
    }
    crate::terminal_view::check_search_links(cx).await?;
    Ok(())
}
