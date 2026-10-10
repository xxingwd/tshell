use super::*;
use tshell_docker::{Endpoint, Event, Target};
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
                let source = format!(
                    "{}:{}:{}",
                    target.id,
                    serde_json::to_string(&target.endpoint).unwrap_or_default(),
                    container
                );
                cx.spawn_in(window, async move |view, cx| {
                    let result = crate::ssh_pool::runtime()
                        .spawn(async move {
                            tshell_docker::terminal::connect(docker, container, command, 28, 100)
                                .await
                        })
                        .await;
                    let _ = view.update_in(cx, |this, window, cx| {
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
                            return;
                        }
                        let io = match result {
                            Ok(Ok(io)) => io,
                            Ok(Err(error)) => {
                                window.push_notification(
                                    Notification::error(crate::i18n::format_error(&error)),
                                    cx,
                                );
                                return;
                            }
                            Err(error) => {
                                window
                                    .push_notification(Notification::error(error.to_string()), cx);
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
                        if this.hosts[0].backend.is_none() {
                            match LocalBackend::restore(
                                this.default_cwd.clone(),
                                None,
                                Some(Vec::new()),
                            ) {
                                Ok(backend) => {
                                    this.hosts[0].backend = Some(Backend::Local(backend))
                                }
                                Err(error) => {
                                    window.push_notification(
                                        Notification::error(crate::i18n::format_error(&error)),
                                        cx,
                                    );
                                    return;
                                }
                            }
                        }
                        let Some(Backend::Local(backend)) = &mut this.hosts[0].backend else {
                            return;
                        };
                        let session = backend.insert_external(source, title, screen);
                        this.switch_host(0, cx);
                        this.act(Action::SelectSession(session), cx);
                        this.show_terminal(cx);
                        this.sync(cx);
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }
}
