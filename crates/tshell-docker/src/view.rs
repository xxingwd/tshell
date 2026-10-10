use super::{
    DockerView, Endpoint, Target, i18n,
    panel::{Filter, Operation, Tab, container_name, is_running},
    t,
};
use gpui_kit::assets::IconName;
use gpui_kit::base::input::Editor;
use gpui_kit::component::{
    Disableable, Icon, Selectable, Sizable,
    button::{Button, ButtonVariants},
    input::Input,
};
use gpui_kit::{prelude::*, *};
use std::collections::BTreeMap;
use tshell_ui::controls::{button, icon_button};
#[derive(Clone)]
enum Row {
    Group(String),
    Container(usize),
}
impl DockerView {
    pub fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = Vec::new();
        if let Some(engine) = self.engine() {
            let query = engine.query.read(cx).value().to_lowercase();
            let mut groups = BTreeMap::<String, Vec<usize>>::new();
            for (index, container) in engine.containers.iter().enumerate() {
                let running = is_running(container);
                if (engine.filter == Filter::Running && !running)
                    || (engine.filter == Filter::Stopped && running)
                {
                    continue;
                }
                let name = container_name(container);
                let image = container.image.as_deref().unwrap_or_default();
                let id = container.id.as_deref().unwrap_or_default();
                if !format!("{name} {image} {id}")
                    .to_lowercase()
                    .contains(&query)
                {
                    continue;
                }
                let group = container
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get("com.docker.compose.project"))
                    .cloned()
                    .unwrap_or_default();
                groups.entry(group).or_default().push(index);
            }
            for (group, mut indices) in groups {
                indices.sort_by_key(|index| container_name(&engine.containers[*index]));
                if !group.is_empty() {
                    rows.push(Row::Group(group));
                }
                rows.extend(indices.into_iter().map(Row::Container));
            }
        }
        let rows = std::sync::Arc::new(rows);
        let source = rows.clone();
        let list = uniform_list(
            "docker-container-list",
            rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .map(|index| match &source[index] {
                        Row::Group(name) => div()
                            .h(px(32.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(12.))
                            .text_color(rgb(this.palette.muted))
                            .child(Icon::new(IconName::Layers).xsmall())
                            .child(name.clone())
                            .into_any_element(),
                        Row::Container(index) => {
                            let Some(engine) = this.engine() else {
                                return div().into_any_element();
                            };
                            let container = &engine.containers[*index];
                            let id = container.id.clone().unwrap_or_default();
                            let name = container_name(container);
                            let active = engine.selected.as_ref() == Some(&id);
                            div()
                                .id(SharedString::from(format!("docker-container-{id}")))
                                .w_full()
                                .h(px(32.))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .text_size(px(12.))
                                .when(active, |row| row.bg(rgba((this.palette.accent << 8) | 32)))
                                .hover(|row| row.bg(rgb(this.palette.row_hover())))
                                .child(Icon::new(IconName::Circle).size(px(8.)).text_color(rgb(
                                    if is_running(container) {
                                        this.palette.ansi[2]
                                    } else {
                                        this.palette.muted
                                    },
                                )))
                                .child(div().min_w_0().flex_1().truncate().child(name))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_container(id.clone(), cx)
                                }))
                                .into_any_element()
                        }
                    })
                    .collect()
            }),
        )
        .size_full();
        let mut sidebar = div().size_full().flex().flex_col().gap_1();
        if let Some(engine) = self.engine() {
            let filter = engine.filter;
            sidebar = sidebar
                .child(div().px_2().child(Input::new(&engine.query).small()))
                .child(
                    div().px_1().flex().gap_1().children(
                        [
                            (Filter::All, "docker.all"),
                            (Filter::Running, "docker.running"),
                            (Filter::Stopped, "docker.stopped"),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (value, key))| {
                            Button::new(("docker-filter", index))
                                .ghost()
                                .small()
                                .h(px(tshell_ui::controls::CONTROL_HEIGHT))
                                .flex_1()
                                .min_w_0()
                                .px_1()
                                .label(t!(key))
                                .selected(filter == value)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(engine) = this.engine_mut() {
                                        engine.filter = value;
                                    }
                                    cx.notify();
                                }))
                        }),
                    ),
                )
                .child(
                    div()
                        .px_2()
                        .text_size(px(11.))
                        .text_color(rgb(self.palette.muted))
                        .child(
                            t!("docker.container_count", count = engine.containers.len())
                                .into_owned(),
                        ),
                );
        }
        sidebar
            .child(div().flex_1().min_h_0().child(if rows.is_empty() {
                tshell_ui::controls::empty_state(
                    IconName::Box,
                    t!("docker.no_containers").into_owned(),
                    self.palette,
                )
                .into_any_element()
            } else {
                list.into_any_element()
            }))
            .into_any_element()
    }
    pub fn status(&self) -> String {
        self.target()
            .map(|target| source(target))
            .unwrap_or_else(|| t!("docker.choose_engine").into_owned())
    }
    fn overview(&self) -> AnyElement {
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
        div()
            .id("docker-overview")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .py_3()
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
        let title = self
            .target()
            .map(|target| source(target))
            .unwrap_or_else(|| t!("docker.choose_engine").into_owned());
        let header = div()
            .h(px(36.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(rgb(p.border))
            .child(Icon::new(IconName::Box).small())
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(12.))
                    .child(title),
            )
            .child(
                icon_button("docker-endpoint", IconName::Settings, t!("docker.endpoint")).on_click(
                    cx.listener(|this, _, window, cx| this.configure_endpoint(window, cx)),
                ),
            )
            .child(
                icon_button(
                    "docker-reconnect",
                    IconName::RefreshCw,
                    t!("docker.reconnect"),
                )
                .on_click(cx.listener(|this, _, _, cx| this.reconnect(cx))),
            );
        let mut surface = div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(rgb(p.text))
            .bg(rgb(p.panel))
            .child(header);
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
                        (Operation::Start, IconName::Play, !running),
                        (Operation::Stop, IconName::Square, running),
                        (Operation::Restart, IconName::RefreshCw, running),
                        (Operation::Remove, IconName::Trash, !running),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (operation, icon, enabled))| {
                        icon_button(("docker-operation", index), icon, t!(operation.key()))
                            .disabled(!ready || !enabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_operation(operation, window, cx)
                            }))
                    }),
                )
                .child(
                    button("docker-terminal", IconName::Terminal, t!("docker.terminal"))
                        .disabled(!ready || !running)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_terminal(window, cx)),
                        ),
                ),
        );
        let tab = engine.tab;
        surface = surface.child(
            div()
                .h(px(34.))
                .flex_shrink_0()
                .px_2()
                .flex()
                .gap_1()
                .border_b_1()
                .border_color(rgb(p.border))
                .children(
                    [
                        (Tab::Overview, "docker.overview", IconName::Info),
                        (Tab::Logs, "docker.logs", IconName::ScrollText),
                        (Tab::Inspect, "docker.inspect", IconName::Braces),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (value, key, icon))| {
                        button(("docker-tab", index), icon, t!(key))
                            .selected(tab == value)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_tab(value, cx)))
                    }),
                ),
        );
        let content = match tab {
            Tab::Overview => self.overview(),
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
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(engine) = this.engine_mut() {
                                    engine.auto_scroll = !engine.auto_scroll;
                                }
                                cx.notify();
                            })),
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
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.logs_worker_reset(cx);
                            })),
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
