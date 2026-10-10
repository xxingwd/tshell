use super::{
    DockerView, Endpoint, Target, i18n,
    panel::{Tab, container_name, is_running},
    t,
};
use gpui_kit::assets::IconName;
use gpui_kit::base::input::Editor;
use gpui_kit::component::{Disableable, Selectable, Sizable, input::Input};
use gpui_kit::{prelude::*, *};
use tshell_ui::controls::icon_button;
impl DockerView {
    pub fn status(&self) -> String {
        self.target()
            .map(|target| source(target))
            .unwrap_or_else(|| t!("docker.choose_engine").into_owned())
    }
    fn overview(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(engine) = self.engine() else {
            return div().into_any_element();
        };
        let Some(detail) = &engine.detail else {
            return tshell_ui::controls::empty_state(
                IconName::Box,
                t!("docker.loading_details").into_owned(),
                self.palette,
            )
            .into_any_element();
        };
        let state = detail.state.as_ref();
        let config = detail.config.as_ref();
        let mut fields = vec![
            (
                t!("docker.id").into_owned(),
                detail.id.clone().unwrap_or_default(),
            ),
            (
                t!("docker.image").into_owned(),
                config
                    .and_then(|config| config.image.clone())
                    .unwrap_or_default(),
            ),
            (
                t!("docker.state").into_owned(),
                state
                    .and_then(|state| state.status)
                    .map(|state| state.to_string())
                    .unwrap_or_default(),
            ),
            (
                t!("docker.health").into_owned(),
                state
                    .and_then(|state| state.health.as_ref())
                    .and_then(|health| health.status)
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                t!("docker.created").into_owned(),
                detail.created.clone().unwrap_or_default(),
            ),
            (
                t!("docker.started").into_owned(),
                state
                    .and_then(|state| state.started_at.clone())
                    .unwrap_or_default(),
            ),
        ];
        let ports = detail
            .network_settings
            .as_ref()
            .and_then(|network| network.ports.as_ref())
            .map(|ports| {
                let mut values = ports
                    .iter()
                    .map(|(port, bindings)| {
                        format!(
                            "{port}  {}",
                            bindings
                                .as_ref()
                                .map(|bindings| bindings
                                    .iter()
                                    .map(|binding| format!(
                                        "{}:{}",
                                        binding.host_ip.as_deref().unwrap_or_default(),
                                        binding.host_port.as_deref().unwrap_or_default()
                                    ))
                                    .collect::<Vec<_>>()
                                    .join(", "))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>();
                values.sort();
                values.join("\n")
            })
            .unwrap_or_default();
        fields.push((t!("docker.ports").into_owned(), ports));
        let mounts = detail
            .mounts
            .as_ref()
            .map(|mounts| {
                mounts
                    .iter()
                    .map(|mount| {
                        format!(
                            "{} → {}{}",
                            mount.source.as_deref().unwrap_or_default(),
                            mount.destination.as_deref().unwrap_or_default(),
                            if mount.rw == Some(false) { " (ro)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        fields.push((t!("docker.mounts").into_owned(), mounts));
        if let Some(config) = &detail.host_config {
            if let Some(cpus) = config.nano_cpus.filter(|value| *value > 0) {
                fields.push((
                    t!("docker.cpu_limit").into_owned(),
                    format!("{:.2}", cpus as f64 / 1_000_000_000.),
                ));
            }
            if let Some(memory) = config.memory.filter(|value| *value > 0) {
                fields.push((
                    t!("docker.memory_limit").into_owned(),
                    crate::resources::format_bytes(memory as u64),
                ));
            }
        }
        if let Some(stats) = &engine.stats {
            for (label, path) in [
                ("docker.memory_usage", "/memory_stats/usage"),
                ("docker.cpu_time", "/cpu_stats/cpu_usage/total_usage"),
                ("docker.pids", "/pids_stats/current"),
            ] {
                if let Some(value) = stats.pointer(path).and_then(serde_json::Value::as_u64) {
                    fields.push((
                        t!(label).into_owned(),
                        match label {
                            "docker.memory_usage" => crate::resources::format_bytes(value),
                            "docker.cpu_time" => format!("{:.2} s", value as f64 / 1_000_000_000.),
                            _ => value.to_string(),
                        },
                    ));
                }
            }
        }
        let mut links = Vec::new();
        if let Some(image) = &detail.image {
            links.push((
                crate::resources::Resource::Image(image.clone()),
                config
                    .and_then(|config| config.image.clone())
                    .unwrap_or_else(|| crate::resources::short_id(image).into()),
            ));
        }
        for mount in detail.mounts.as_deref().unwrap_or_default() {
            if let Some(name) = &mount.name {
                links.push((
                    crate::resources::Resource::Volume(name.clone()),
                    name.clone(),
                ));
            }
        }
        if let Some(networks) = detail
            .network_settings
            .as_ref()
            .and_then(|settings| settings.networks.as_ref())
        {
            for (name, settings) in networks {
                if let Some(id) = &settings.network_id {
                    links.push((
                        crate::resources::Resource::Network(id.clone()),
                        name.clone(),
                    ));
                }
            }
        }
        div()
            .id("docker-overview")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .py_3()
            .children(
                links
                    .into_iter()
                    .enumerate()
                    .map(|(index, (resource, name))| {
                        tshell_ui::controls::button(
                            ("docker-related", index),
                            match resource {
                                crate::resources::Resource::Image(_) => IconName::Image,
                                crate::resources::Resource::Volume(_) => IconName::HardDrive,
                                _ => IconName::Network,
                            },
                            name,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.select_resource(resource.clone(), window, cx)
                            },
                        ))
                    }),
            )
            .children(fields.into_iter().map(|(label, value)| {
                div()
                    .min_w_0()
                    .flex()
                    .gap_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(self.palette.border))
                    .text_size(px(12.))
                    .child(
                        div()
                            .w(px(95.))
                            .flex_shrink_0()
                            .text_color(rgb(self.palette.muted))
                            .child(label),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_color(rgb(self.palette.text))
                            .whitespace_normal()
                            .child(if value.is_empty() {
                                "—".to_owned()
                            } else {
                                value
                            }),
                    )
            }))
            .into_any_element()
    }
    fn surface(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        if self
            .engine()
            .is_some_and(|engine| engine.resources.page != crate::ResourcePage::Containers)
        {
            return self.resource_surface(cx);
        }
        let mut surface = div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(rgb(p.text))
            .bg(rgb(p.panel));
        let Some(engine) = self.engine() else {
            return surface
                .child(tshell_ui::controls::empty_state(
                    IconName::Server,
                    t!("docker.choose_engine").into_owned(),
                    p,
                ))
                .into_any_element();
        };
        if let Some(error) = &engine.error {
            surface = surface.child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(rgb(p.ansi[1]))
                    .child(i18n::format_failure(error)),
            );
        }
        if engine.loading {
            surface = surface.child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(rgb(p.muted))
                    .child(t!("docker.loading").into_owned()),
            );
        }
        let Some(id) = &engine.selected else {
            if engine.resources.output_visible && engine.resources.output_resource.is_none() {
                return surface
                    .child(Editor::new(&engine.resources.output))
                    .into_any_element();
            }
            return surface
                .child(tshell_ui::controls::empty_state(
                    IconName::Box,
                    t!("docker.select_container").into_owned(),
                    p,
                ))
                .into_any_element();
        };
        let selected = engine
            .containers
            .iter()
            .find(|container| container.id.as_ref() == Some(id));
        let running = selected.is_some_and(is_running);
        let ready = engine.detail.is_some() && engine.busy.is_none();
        let name = selected.map(container_name).unwrap_or_else(|| id.clone());
        let tab = engine.tab;
        surface = surface.child(
            div()
                .min_h(px(36.))
                .flex_shrink_0()
                .px_2()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(name),
                )
                .children(
                    [
                        (Tab::Overview, "docker.overview", IconName::Info),
                        (Tab::Logs, "docker.logs", IconName::ScrollText),
                        (Tab::Inspect, "docker.inspect", IconName::Braces),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (value, key, icon))| {
                        icon_button(("docker-tab", index), icon, t!(key))
                            .selected(tab == value)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_tab(value, cx)))
                    }),
                )
                .children(
                    [
                        (
                            if running {
                                crate::actions::Action::Stop
                            } else {
                                crate::actions::Action::Start
                            },
                            if running {
                                IconName::Square
                            } else {
                                IconName::Play
                            },
                            true,
                        ),
                        (
                            crate::actions::Action::RestartContainer,
                            IconName::RefreshCw,
                            running,
                        ),
                        (
                            crate::actions::Action::RemoveContainer,
                            IconName::Trash,
                            !running,
                        ),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (operation, icon, enabled))| {
                        let target = engine.target.clone();
                        let resource = crate::resources::Resource::Container(id.clone());
                        icon_button(("docker-operation", index), icon, t!(operation.key()))
                            .disabled(!ready || !enabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_action(
                                    target.clone(),
                                    Some(resource.clone()),
                                    operation,
                                    window,
                                    cx,
                                )
                            }))
                    }),
                )
                .child(
                    icon_button("docker-terminal", IconName::Terminal, t!("docker.terminal"))
                        .disabled(!ready || !running)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_terminal(window, cx)),
                        ),
                )
                .child(self.action_button(
                    vec![
                            crate::actions::Action::Rename,
                            crate::actions::Action::Update,
                            crate::actions::Action::Recreate,
                            if engine
                                .detail
                                .as_ref()
                                .and_then(|detail| detail.state.as_ref())
                                .is_some_and(|state| state.paused == Some(true))
                            {
                                crate::actions::Action::Resume
                            } else {
                                crate::actions::Action::Pause
                            },
                        ],
                    Some(crate::resources::Resource::Container(id.clone())),
                    cx,
                ))
                .when(
                    !engine.resources.output_text.is_empty()
                        && (engine.resources.output_resource.is_none()
                            || engine.resources.output_resource
                                == Some(crate::resources::Resource::Container(id.clone()))),
                    |bar| {
                        bar.child(
                            icon_button(
                                "docker-container-output",
                                IconName::ScrollText,
                                t!("docker.output"),
                            )
                            .selected(engine.resources.output_visible)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(engine) = this.engine_mut() {
                                    engine.resources.output_visible =
                                        !engine.resources.output_visible;
                                }
                                cx.notify();
                            })),
                        )
                    },
                ),
        );
        let content = if engine.resources.output_visible
            && (engine.resources.output_resource.is_none()
                || engine.resources.output_resource
                    == Some(crate::resources::Resource::Container(id.clone())))
        {
            div()
                .size_full()
                .min_h_0()
                .child(Editor::new(&engine.resources.output))
                .into_any_element()
        } else {
            match tab {
                Tab::Overview => self.overview(cx),
                Tab::Inspect => div()
                    .size_full()
                    .font_family(self.font_family.clone())
                    .text_size(px(self.font_size.clamp(12., 17.)))
                    .child(Editor::new(&engine.inspect_editor))
                    .into_any_element(),
                Tab::Logs => div()
                    .size_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .min_h(px(34.))
                            .flex_shrink_0()
                            .px_2()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .min_w(px(100.))
                                    .flex_1()
                                    .child(Input::new(&engine.logs_query).small()),
                            )
                            .child(
                                icon_button(
                                    "docker-log-pause",
                                    if engine.paused {
                                        IconName::Play
                                    } else {
                                        IconName::Pause
                                    },
                                    t!(if engine.paused {
                                        "docker.resume"
                                    } else {
                                        "docker.pause"
                                    }),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.pause_logs(cx))),
                            )
                            .child(
                                icon_button(
                                    "docker-log-scroll",
                                    IconName::ArrowDownToLine,
                                    t!("docker.auto_scroll"),
                                )
                                .selected(engine.auto_scroll)
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if let Some(engine) = this.engine_mut() {
                                            engine.auto_scroll = !engine.auto_scroll;
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                icon_button("docker-log-copy", IconName::Copy, t!("editor.copy"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(engine) = this.engine() {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                engine.log.text(),
                                            ));
                                        }
                                    })),
                            )
                            .child(
                                icon_button(
                                    "docker-log-refresh",
                                    IconName::RefreshCw,
                                    t!("ws.refresh"),
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.logs_worker_reset(cx);
                                    },
                                )),
                            ),
                    )
                    .when(engine.logs_loading, |view| {
                        view.child(
                            div()
                                .px_3()
                                .text_size(px(12.))
                                .child(t!("docker.loading").into_owned()),
                        )
                    })
                    .when_some(engine.logs_error.as_ref(), |view, error| {
                        view.child(
                            div()
                                .px_3()
                                .py_1()
                                .text_size(px(12.))
                                .text_color(rgb(p.ansi[1]))
                                .child(i18n::format_failure(error)),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .font_family(self.font_family.clone())
                            .text_size(px(self.font_size.clamp(12., 17.)))
                            .child(Editor::new(&engine.logs_editor)),
                    )
                    .into_any_element(),
            }
        };
        surface
            .child(div().flex_1().min_h_0().flex().flex_col().child(content))
            .into_any_element()
    }
}
fn source(target: &Target) -> String {
    match &target.endpoint {
        Endpoint::Local { path } => format!(
            "{} · {}",
            target.name,
            path.as_deref().unwrap_or(if cfg!(windows) {
                "//./pipe/docker_engine"
            } else {
                "/var/run/docker.sock"
            })
        ),
        Endpoint::Ssh(host) => format!(
            "{} · SSH {}{}:{}",
            target.name,
            if host.user.is_empty() {
                String::new()
            } else {
                format!("{}@", host.user)
            },
            host.destination,
            host.port.unwrap_or(22)
        ),
    }
}
impl Render for DockerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.surface(cx)
    }
}
