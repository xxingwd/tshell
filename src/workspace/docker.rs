use super::*;
#[cfg(debug_assertions)]
use std::time::Instant;
use tshell_docker::{Endpoint, Event, Target};
pub(super) struct Terminal {
    target: Target,
    request: u64,
    title: String,
    state: TerminalState,
    expanded: bool,
}

#[cfg(debug_assertions)]
pub(super) async fn check_terminal(
    handle: WindowHandle<Root>,
    app: Entity<AppView>,
    remote: usize,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let mut source = None;
    let mut original = None;
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            original = Some((app.active, app.page, app.sidebar_collapsed));
            app.switch_host(0, cx);
            app.page = Page::Docker;
            app.sidebar_collapsed = false;
            app.need_focus = false;
            let target = Target::local(
                crate::t!("docker.local").into_owned(),
                app.docker_endpoint.clone(),
            );
            let session = crate::terminal::Session::remote_with_resize(
                "Docker exec fixture".into(),
                28,
                100,
                std::sync::Arc::new(|_| Ok(())),
                None,
            );
            session.remote_output(b"\x1b[32mcontainer exec\x1b[0m\r\n/app # ");
            source = Some(std::sync::Arc::downgrade(&session));
            let view = cx.new(|cx| {
                TerminalView::from_session(session, app.font_size, app.terminal_palette, cx)
            });
            app.docker_terminal = Some(Terminal {
                target,
                request: 0,
                title: "Docker · Local · alpine".into(),
                state: TerminalState::Attached(view.clone()),
                expanded: false,
            });
            window.focus(&view.read(cx).focus_handle(cx), cx);
            cx.notify();
        });
    })?;
    for expanded in [false, true] {
        cx.update_window(handle.into(), |_, window, cx| {
            app.update(cx, |app, cx| {
                app.docker_terminal.as_mut().unwrap().expanded = expanded;
                cx.notify();
            });
            window.refresh();
            let _ = window.draw(cx);
        })?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let ready = cx.update_window(handle.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
                app.read(cx)
                    .docker_terminal_view()
                    .unwrap()
                    .read(cx)
                    .snapshot_contains("container exec")
            })?;
            if ready {
                break;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "embedded exec output did not render"
            );
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
        }
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
            let terminal = app.read(cx).docker_terminal_view().unwrap();
            let bounds = terminal.read(cx).rendered_bounds();
            anyhow::ensure!(
                bounds.size.height > px(100.) && bounds.size.width > px(400.),
                "embedded exec lost its content rectangle"
            );
            anyhow::ensure!(
                app.read(cx).page == Page::Docker && app.read(cx).active == 0,
                "embedded exec changed the host or page"
            );
            #[cfg(feature = "ui-check-screenshots")]
            if let Some(directory) = std::env::var_os("TSHELL_UI_SCREENSHOT_DIR") {
                window
                    .render_to_image()?
                    .save(PathBuf::from(directory).join(if expanded {
                        "docker-terminal-expanded.png"
                    } else {
                        "docker-terminal-drawer.png"
                    }))?;
            }
            app.update(cx, |app, cx| {
                app.switch_host(remote, cx);
                assert!(
                    app.docker_terminal_view().is_none(),
                    "exec from a different host was shown"
                );
                app.switch_host(0, cx);
                assert!(
                    app.docker_terminal_view().is_some(),
                    "exec was retargeted on host switch"
                );
            });
            Ok::<_, anyhow::Error>(())
        })??;
    }
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            let view = app.docker_terminal_view().unwrap();
            window.focus(&view.read(cx).focus_handle(cx), cx);
            app.execute_shortcut(Shortcut::TerminalSearch, window, cx);
            assert_eq!(
                app.page,
                Page::Docker,
                "exec search switched the global page"
            );
            app.execute_shortcut(Shortcut::ClosePane, window, cx);
            assert!(app.docker_terminal.is_none(), "exec close kept its view");
            let (host, page, collapsed) = original.unwrap();
            app.active = host;
            app.page = page;
            app.sidebar_collapsed = collapsed;
            cx.notify();
        });
    })?;
    // Snapshot/search jobs can hold the source until their cancelled task is dropped.
    for _ in 0..20 {
        if source.as_ref().unwrap().upgrade().is_none() {
            return Ok(());
        }
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
    anyhow::bail!("closed Docker exec retained its terminal source")
}
enum TerminalState {
    Connecting,
    Attached(Entity<TerminalView>),
    Failed(String),
}
pub(super) fn targets(hosts: &[Host], endpoint: Option<String>) -> Vec<Target> {
    let mut targets = vec![Target::local(
        crate::t!("docker.local").into_owned(),
        endpoint,
    )];
    targets.extend(hosts.iter().filter_map(|host| {
        host.config.as_ref().map(|config| Target {
            id: config.id.clone(),
            name: host.name.clone(),
            endpoint: Endpoint::Ssh(config.ssh.clone()),
        })
    }));
    targets
}
impl AppView {
    fn docker_terminal_current(&self) -> Option<&Terminal> {
        self.docker_terminal.as_ref().filter(|terminal| {
            terminal.target.id == self.hosts[self.active].identity()
                && targets(&self.hosts, self.docker_endpoint.clone())
                    .iter()
                    .any(|target| {
                        target.id == terminal.target.id
                            && target.endpoint == terminal.target.endpoint
                    })
        })
    }
    pub(super) fn prepare_docker_terminal(&self, cx: &mut Context<Self>) {
        let Some(terminal) = &self.docker_terminal else {
            return;
        };
        if let TerminalState::Attached(view) = &terminal.state {
            let visible = self.page == Page::Docker && self.docker_terminal_current().is_some();
            view.update(cx, |view, cx| {
                view.font_size = self.font_size;
                view.font_family = self.font_family.clone();
                view.line_height_scale = self.line_height_scale;
                view.ligatures = self.ligatures;
                view.palette = self.terminal_palette;
                if let Some(session) = &view.session {
                    session.set_theme(self.terminal_palette.terminal_theme());
                }
                view.set_visible(visible, cx);
            });
        }
    }
    pub(super) fn docker_terminal_view(&self) -> Option<Entity<TerminalView>> {
        match &self.docker_terminal_current()?.state {
            TerminalState::Attached(view) => Some(view.clone()),
            _ => None,
        }
    }
    pub(super) fn close_docker_terminal(&mut self, cx: &mut Context<Self>) {
        self.docker_terminal = None;
        self.need_focus = true;
        cx.notify();
    }
    pub(super) fn docker_surface(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut surface = div().size_full().flex().flex_col();
        let terminal = self.docker_terminal_current();
        if terminal.is_none_or(|terminal| !terminal.expanded) {
            surface = surface.child(div().flex_1().min_h_0().child(self.docker.clone()));
        }
        let Some(terminal) = terminal else {
            return surface.into_any_element();
        };
        let content = match &terminal.state {
            TerminalState::Attached(view) => view.clone().into_any_element(),
            TerminalState::Connecting => style::empty_state(
                IconName::Terminal,
                crate::t!("docker.loading").into_owned(),
                self.palette,
            )
            .into_any_element(),
            TerminalState::Failed(error) => {
                style::empty_state(IconName::CircleAlert, error.clone(), self.palette)
                    .text_color(rgb(self.palette.error))
                    .into_any_element()
            }
        };
        surface
            .child(
                div()
                    .id("docker-terminal-drawer")
                    .flex_shrink_0()
                    .when(terminal.expanded, |drawer| drawer.h_full())
                    .when(!terminal.expanded, |drawer| drawer.h(relative(0.44)))
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .bg(rgb(self.terminal_palette.terminal))
                    .border_t_1()
                    .border_color(rgb(self.palette.border))
                    .child(
                        div()
                            .h(px(30.))
                            .flex_shrink_0()
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .bg(rgb(self.palette.panel))
                            .text_color(rgb(self.palette.text))
                            .text_size(px(12.))
                            .child(
                                Icon::new(IconName::Terminal)
                                    .size(px(14.))
                                    .text_color(rgb(self.palette.muted)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .child(terminal.title.clone()),
                            )
                            .child(
                                icon_button(
                                    "docker-expand-terminal",
                                    if terminal.expanded {
                                        IconName::Minimize2
                                    } else {
                                        IconName::Maximize
                                    },
                                    if terminal.expanded {
                                        crate::t!("ws.restore_tiled")
                                    } else {
                                        crate::t!("docker.expand_detail")
                                    },
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if let Some(terminal) = this.docker_terminal.as_mut() {
                                            terminal.expanded = !terminal.expanded;
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                icon_button(
                                    "docker-close-terminal",
                                    IconName::Close,
                                    crate::t!("ws.close_pane"),
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.close_docker_terminal(cx)),
                                ),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(content)),
            )
            .into_any_element()
    }
    pub(super) fn active_is_container(&self) -> bool {
        let host = &self.hosts[self.active];
        match &host.backend {
            Some(Backend::Local(backend)) => host
                .snapshot
                .session()
                .is_some_and(|session| backend.is_external_session(&session.id)),
            _ => false,
        }
    }
    pub(super) fn prepare_docker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let targets = targets(&self.hosts, self.docker_endpoint.clone());
        let host_id = self.hosts[self.active].identity();
        let visible = self.page == Page::Docker;
        self.docker.update(cx, |docker, cx| {
            docker.palette = self.palette;
            docker.terminal_palette = self.terminal_palette;
            docker.font_family = self.font_family.clone();
            docker.font_size = self.font_size;
            docker.set_targets(targets, window, cx);
            docker.set_visible(visible, host_id, window, cx);
        });
    }
    pub(super) fn docker_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::LocalEndpointChanged(path) => {
                if self.docker.read(cx).busy(&tshell_host::HostId::local(), cx) {
                    window.push_notification(
                        Notification::warning(crate::t!("docker.save_first")),
                        cx,
                    );
                    return;
                }
                self.docker_endpoint = path;
                self.save(cx);
                self.prepare_docker(window, cx);
            }
            Event::OpenTerminal {
                target,
                docker,
                container,
                name,
                command,
            } => {
                let title = format!("Docker · {} · {name}", target.name);
                self.docker_terminal_request += 1;
                let request = self.docker_terminal_request;
                self.docker_terminal = Some(Terminal {
                    target: target.clone(),
                    request,
                    title: title.clone(),
                    state: TerminalState::Connecting,
                    expanded: false,
                });
                cx.notify();
                cx.spawn_in(window, async move |view, cx| {
                    let result = crate::ssh_pool::runtime()
                        .spawn(async move {
                            tshell_docker::terminal::connect(docker, container, command, 28, 100)
                                .await
                        })
                        .await;
                    let _ = view.update_in(cx, |this, window, cx| {
                        if this
                            .docker_terminal
                            .as_ref()
                            .is_none_or(|terminal| terminal.request != request)
                        {
                            return;
                        }
                        if !targets(&this.hosts, this.docker_endpoint.clone())
                            .iter()
                            .any(|current| {
                                current.id == target.id && current.endpoint == target.endpoint
                            })
                        {
                            window.push_notification(
                                Notification::warning(crate::t!("docker.target_changed")),
                                cx,
                            );
                            this.docker_terminal = None;
                            return;
                        }
                        let io = match result {
                            Ok(Ok(io)) => io,
                            Ok(Err(error)) => {
                                if let Some(terminal) = this.docker_terminal.as_mut() {
                                    terminal.state =
                                        TerminalState::Failed(crate::i18n::format_error(&error));
                                }
                                cx.notify();
                                return;
                            }
                            Err(error) => {
                                if let Some(terminal) = this.docker_terminal.as_mut() {
                                    terminal.state = TerminalState::Failed(error.to_string());
                                }
                                cx.notify();
                                return;
                            }
                        };
                        let input = io.input.clone();
                        let resize = io.input;
                        let screen = crate::terminal::Session::remote_with_resize(
                            title.clone(),
                            28,
                            100,
                            std::sync::Arc::new(move |bytes| input.send(bytes)),
                            Some(std::sync::Arc::new(move |rows, cols| {
                                resize.resize(rows, cols)
                            })),
                        );
                        screen.set_theme(this.terminal_palette.terminal_theme());
                        let reader = std::sync::Arc::downgrade(&screen);
                        crate::ssh_pool::runtime().spawn(async move {
                            while let Ok(chunk) = io.output.recv().await {
                                let Some(screen) = reader.upgrade() else {
                                    break;
                                };
                                match chunk {
                                    Ok(bytes) => screen.remote_output(&bytes),
                                    Err(error) => {
                                        *screen.error.lock() =
                                            Some(tshell_ui::i18n::format_failure(&error));
                                        break;
                                    }
                                }
                            }
                            if let Some(screen) = reader.upgrade() {
                                screen
                                    .exited
                                    .store(true, std::sync::atomic::Ordering::Release);
                                screen.metadata.notify();
                                screen.updates.notify();
                            }
                        });
                        let terminal = cx.new(|cx| {
                            let mut view = TerminalView::from_session(
                                screen,
                                this.font_size,
                                this.terminal_palette,
                                cx,
                            );
                            view.font_family = this.font_family.clone();
                            view.line_height_scale = this.line_height_scale;
                            view.ligatures = this.ligatures;
                            view.background_inherited = true;
                            view.set_visible(true, cx);
                            view
                        });
                        if let Some(drawer) = this.docker_terminal.as_mut() {
                            drawer.state = TerminalState::Attached(terminal.clone());
                        }
                        if this.page == Page::Docker
                            && this.hosts[this.active].identity() == target.id
                        {
                            this.need_focus = false;
                            window.focus(&terminal.read(cx).focus_handle(cx), cx);
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }
}
