use crate::actions::Action;
use crate::{
    DockerView, ResourcePage, i18n,
    panel::{container_name, is_running},
    resources::{Resource, image_name, service_name},
    t,
};
use gpui_kit::assets::IconName;
use gpui_kit::base::input::Editor;
use gpui_kit::component::{
    Disableable, Icon, Selectable, Sizable,
    button::{Button, ButtonVariants},
    input::Input,
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
};
use gpui_kit::{prelude::*, *};
use tshell_ui::controls::icon_button;

#[derive(Clone)]
enum Row {
    Group(&'static str),
    Container(usize, bool),
    Resource(Resource),
}
impl DockerView {
    pub fn sidebar(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let label_width = ResourcePage::ALL
            .iter()
            .map(|page| {
                t!(page.key())
                    .chars()
                    .map(|character| if character.is_ascii() { 6. } else { 12. })
                    .sum::<f32>()
            })
            .sum::<f32>()
            + 52.;
        let page = self
            .engine()
            .map(|engine| engine.resources.page)
            .unwrap_or(ResourcePage::Containers);
        let mut sidebar = div().size_full().flex().flex_col().gap_1().child(
            div()
                .h(px(34.))
                .px_1()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .border_b_1()
                .border_color(rgb(self.palette.border))
                .children(
                    ResourcePage::ALL
                        .into_iter()
                        .enumerate()
                        .map(|(index, value)| {
                            let icon = match value {
                                ResourcePage::Containers => IconName::Box,
                                ResourcePage::Compose => IconName::Layers,
                                ResourcePage::Images => IconName::Image,
                                ResourcePage::More => IconName::Ellipsis,
                            };
                            if width < label_width {
                                icon_button(("docker-resource-page", index), icon, t!(value.key()))
                                    .flex_1()
                                    .selected(page == value)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_resource_page(value, window, cx)
                                    }))
                                    .into_any_element()
                            } else {
                                Button::new(("docker-resource-page", index))
                                    .ghost()
                                    .small()
                                    .min_w_0()
                                    .px_1()
                                    .text_size(px(12.))
                                    .tooltip(t!(value.key()))
                                    .flex_grow(1.)
                                    .flex_shrink_0()
                                    .label(t!(value.key()))
                                    .selected(page == value)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_resource_page(value, window, cx)
                                    }))
                                    .into_any_element()
                            }
                        }),
                ),
        );
        let Some(engine) = self.engine() else {
            return sidebar.into_any_element();
        };
        let query_state = if page == ResourcePage::Containers {
            &engine.query
        } else {
            &engine.resources.queries[&page]
        };
        let query = query_state.read(cx).value().to_lowercase();
        let create_actions = match page {
            ResourcePage::Containers => vec![Action::CreateContainer],
            ResourcePage::Compose => vec![Action::ImportProject],
            ResourcePage::Images => vec![Action::Pull, Action::ImportImage],
            ResourcePage::More => vec![Action::CreateVolume, Action::CreateNetwork],
        };
        sidebar = sidebar.child(
            div()
                .px_2()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(query_state).small()),
                )
                .child(self.action_button(create_actions, None, cx))
                .child(
                    icon_button(
                        "docker-refresh-resources",
                        IconName::RefreshCw,
                        t!("ws.refresh"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                ),
        );
        let mut rows = Vec::new();
        match page {
            ResourcePage::Containers => {
                let mut indices: Vec<_> = engine
                    .containers
                    .iter()
                    .enumerate()
                    .filter(|(_, container)| {
                        format!(
                            "{} {} {}",
                            container_name(container),
                            container.image.as_deref().unwrap_or_default(),
                            container.id.as_deref().unwrap_or_default()
                        )
                        .to_lowercase()
                        .contains(&query)
                    })
                    .map(|(index, _)| index)
                    .collect();
                indices.sort_by_key(|index| container_name(&engine.containers[*index]));
                rows.extend(
                    indices
                        .into_iter()
                        .map(|index| Row::Container(index, false)),
                );
            }
            ResourcePage::Compose => {
                for project in &engine.resources.projects {
                    let matching = project.name.to_lowercase().contains(&query);
                    let containers: Vec<_> = engine
                        .containers
                        .iter()
                        .enumerate()
                        .filter(|(_, container)| {
                            container
                                .labels
                                .as_ref()
                                .and_then(|labels| labels.get("com.docker.compose.project"))
                                == Some(&project.name)
                        })
                        .collect();
                    if matching
                        || containers.iter().any(|(_, container)| {
                            service_name(container).to_lowercase().contains(&query)
                        })
                    {
                        rows.push(Row::Resource(Resource::Project(project.name.clone())));
                        rows.extend(
                            containers
                                .into_iter()
                                .filter(|(_, container)| {
                                    matching
                                        || service_name(container).to_lowercase().contains(&query)
                                })
                                .map(|(index, _)| Row::Container(index, true)),
                        );
                    }
                }
            }
            ResourcePage::Images => {
                let mut images: Vec<_> = engine
                    .resources
                    .images
                    .iter()
                    .filter(|image| {
                        format!("{} {}", image_name(image), image.id)
                            .to_lowercase()
                            .contains(&query)
                    })
                    .collect();
                images.sort_by_key(|image| image_name(image));
                rows.extend(
                    images
                        .into_iter()
                        .map(|image| Row::Resource(Resource::Image(image.id.clone()))),
                );
            }
            ResourcePage::More => {
                for resource in [
                    Resource::Info,
                    Resource::Disk,
                    Resource::Events,
                    Resource::Daemon,
                ] {
                    if engine
                        .resources
                        .name(&resource)
                        .to_lowercase()
                        .contains(&query)
                    {
                        rows.push(Row::Resource(resource));
                    }
                }
                let volumes: Vec<_> = engine
                    .resources
                    .volumes
                    .iter()
                    .filter(|volume| volume.name.to_lowercase().contains(&query))
                    .collect();
                if !volumes.is_empty() {
                    rows.push(Row::Group("docker.volumes"));
                    rows.extend(
                        volumes
                            .into_iter()
                            .map(|volume| Row::Resource(Resource::Volume(volume.name.clone()))),
                    );
                }
                let networks: Vec<_> = engine
                    .resources
                    .networks
                    .iter()
                    .filter(|network| {
                        network
                            .name
                            .as_deref()
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(&query)
                    })
                    .collect();
                if !networks.is_empty() {
                    rows.push(Row::Group("docker.networks"));
                    rows.extend(
                        networks
                            .into_iter()
                            .filter_map(|network| network.id.clone())
                            .map(|id| Row::Resource(Resource::Network(id))),
                    );
                }
            }
        }
        let count = rows.len();
        let navigation: Vec<_> = rows
            .iter()
            .enumerate()
            .filter_map(|(position, row)| {
                Some((
                    position,
                    match row {
                        Row::Group(_) => None,
                        Row::Resource(key) => Some(key.clone()),
                        Row::Container(index, _) => engine.containers[*index]
                            .id
                            .clone()
                            .map(Resource::Container),
                    }?,
                ))
            })
            .collect();
        let focus = engine.resources.focus.clone();
        let scroll = engine.resources.scrolls[&page].clone();
        let list = uniform_list(
            ("docker-resource-list", page as usize),
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| {
                        let row = &rows[index];
                        let Some(engine) = this.engine() else {
                            return div().into_any_element();
                        };
                        match row {
                            Row::Group(key) => div()
                                .h(px(30.))
                                .px_2()
                                .flex()
                                .items_center()
                                .text_size(px(12.))
                                .text_color(rgb(this.palette.muted))
                                .child(t!(*key).into_owned())
                                .into_any_element(),
                            Row::Container(index, service) => {
                                let container = &engine.containers[*index];
                                let id = container.id.clone().unwrap_or_default();
                                let owner = cx.entity().downgrade();
                                let target = engine.target.clone();
                                let resource = Resource::Container(id.clone());
                                let name = if *service {
                                    service_name(container)
                                } else {
                                    container_name(container)
                                };
                                let active = !service && engine.selected.as_ref() == Some(&id);
                                let paused = container.state
                                    == Some(bollard::models::ContainerSummaryStateEnum::PAUSED);
                                let mut palette = this.palette;
                                palette.muted = if is_running(container) {
                                    palette.ansi[2]
                                } else if paused {
                                    palette.ansi[3]
                                } else {
                                    palette.muted
                                };
                                let status = container.status.clone().unwrap_or_default();
                                resource_row(
                                    SharedString::from(format!("docker-container-{id}")),
                                    name,
                                    if is_running(container) {
                                        IconName::Play
                                    } else if paused {
                                        IconName::Pause
                                    } else {
                                        IconName::Square
                                    },
                                    active,
                                    palette,
                                )
                                .when(*service, |row| row.pl_5())
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(status.clone())
                                        .build(window, cx)
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_container(id.clone(), window, cx)
                                }))
                                .context_menu(move |menu, _, _| {
                                    crate::panel::action_menu(
                                        menu,
                                        resource_actions(&resource, None),
                                        Some(target.clone()),
                                        Some(resource.clone()),
                                        owner.clone(),
                                    )
                                })
                                .into_any_element()
                            }
                            Row::Resource(resource) => {
                                let name = engine.resources.name(resource);
                                let active = engine.resources.selection() == Some(resource);
                                let icon = match resource {
                                    Resource::Container(_) => IconName::Box,
                                    Resource::Project(_) => IconName::Layers,
                                    Resource::Image(_) => IconName::Image,
                                    Resource::Volume(_) => IconName::HardDrive,
                                    Resource::Network(_) => IconName::Network,
                                    Resource::Info => IconName::Info,
                                    Resource::Disk => IconName::HardDrive,
                                    Resource::Events => IconName::ScrollText,
                                    Resource::Daemon => IconName::Settings,
                                };
                                let resource = resource.clone();
                                let menu_resource = resource.clone();
                                let target = engine.target.clone();
                                let owner = cx.entity().downgrade();
                                let actions = resource_actions(
                                    &resource,
                                    engine.resources.daemon.as_ref().filter(|source| {
                                        source.engine_id.as_ref()
                                            == engine
                                                .info
                                                .as_ref()
                                                .and_then(|info| info.id.as_ref())
                                    }),
                                );
                                resource_row(
                                    SharedString::from(format!("docker-resource-{resource:?}")),
                                    name,
                                    icon,
                                    active,
                                    this.palette,
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_resource(resource.clone(), window, cx)
                                }))
                                .context_menu(move |menu, _, _| {
                                    crate::panel::action_menu(
                                        menu,
                                        actions.clone(),
                                        Some(target.clone()),
                                        Some(menu_resource.clone()),
                                        owner.clone(),
                                    )
                                })
                                .into_any_element()
                            }
                        }
                    })
                    .collect()
            }),
        )
        .track_scroll(&scroll)
        .size_full();
        sidebar
            .child(
                div()
                    .id("docker-resource-navigation")
                    .track_focus(&focus)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _, window, cx| focus.focus(window, cx)),
                    )
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        let Some(engine) = this.engine() else {
                            return;
                        };
                        let current = if engine.resources.page == ResourcePage::Containers {
                            engine.selected.clone().map(Resource::Container)
                        } else {
                            engine.resources.selection().cloned()
                        };
                        let position = current
                            .as_ref()
                            .and_then(|key| navigation.iter().position(|(_, entry)| entry == key));
                        let next = match event.keystroke.key.as_str() {
                            "down" => Some(
                                position
                                    .map(|index| {
                                        (index + 1).min(navigation.len().saturating_sub(1))
                                    })
                                    .unwrap_or(0),
                            ),
                            "up" => Some(position.unwrap_or(0).saturating_sub(1)),
                            "home" => Some(0),
                            "end" => Some(navigation.len().saturating_sub(1)),
                            _ => None,
                        };
                        if let Some(index) = next {
                            if let Some((position, resource)) = navigation.get(index) {
                                this.select_resource(resource.clone(), window, cx);
                                if let Some(engine) = this.engine() {
                                    engine.resources.scrolls[&engine.resources.page]
                                        .scroll_to_item(*position, ScrollStrategy::Top);
                                }
                                cx.stop_propagation();
                            }
                        }
                    }))
                    .flex_1()
                    .min_h_0()
                    .child(if count == 0 {
                        tshell_ui::controls::empty_state(
                            IconName::Search,
                            t!("docker.no_resources").into_owned(),
                            self.palette,
                        )
                        .into_any_element()
                    } else {
                        list.into_any_element()
                    }),
            )
            .into_any_element()
    }
    pub(super) fn resource_surface(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let mut surface = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.panel))
            .text_color(rgb(p.text));
        let Some(engine) = self.engine() else {
            return surface.into_any_element();
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
        let Some(resource) = engine.resources.selection() else {
            if engine.resources.output_visible && engine.resources.output_resource.is_none() {
                return surface
                    .child(Editor::new(&engine.resources.output))
                    .into_any_element();
            }
            return surface
                .child(tshell_ui::controls::empty_state(
                    IconName::MousePointer,
                    t!("docker.select_resource").into_owned(),
                    p,
                ))
                .into_any_element();
        };
        let document = engine.resources.document(resource);
        surface = surface.child(
            div()
                .h(px(36.))
                .px_2()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .border_b_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(engine.resources.name(resource)),
                )
                .when(matches!(resource, Resource::Project(_)), |bar| {
                    bar.child(self.service_selector(resource, cx))
                })
                .when_some(document, |bar, document| {
                    let target = engine.target.clone();
                    let resource = resource.clone();
                    bar.child(
                        icon_button("docker-save-document", IconName::Save, t!("editor.save"))
                            .disabled(!document.dirty(cx) || engine.busy.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_action(
                                    target.clone(),
                                    Some(resource.clone()),
                                    Action::Save,
                                    window,
                                    cx,
                                )
                            })),
                    )
                })
                .when(document.is_some_and(|document| document.dirty(cx)), |bar| {
                    let target = engine.target.clone();
                    let resource = resource.clone();
                    bar.child(
                        icon_button(
                            "docker-discard-document",
                            IconName::Undo2,
                            t!("docker.discard_changes"),
                        )
                        .disabled(engine.busy.is_some())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.discard_document(target.clone(), resource.clone(), window, cx)
                            },
                        )),
                    )
                })
                .when(*resource != Resource::Daemon, |bar| {
                    let resource = resource.clone();
                    bar.child(
                        icon_button(
                            "docker-preview-mode",
                            IconName::Braces,
                            t!("docker.inspect"),
                        )
                        .selected(engine.resources.raw())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if let Some(engine) = this.engine_mut() {
                                    let page = engine.resources.page;
                                    if !engine.resources.raw.remove(&page) {
                                        engine.resources.raw.insert(page);
                                    }
                                }
                                this.select_resource(resource.clone(), window, cx);
                            },
                        )),
                    )
                })
                .when(
                    *resource == Resource::Info
                        && matches!(engine.target.endpoint, crate::Endpoint::Local { .. }),
                    |bar| {
                        bar.child(
                            icon_button(
                                "docker-endpoint",
                                IconName::Settings,
                                t!("docker.endpoint"),
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| this.configure_endpoint(window, cx),
                            )),
                        )
                    },
                )
                .when(*resource == Resource::Info, |bar| {
                    bar.child(
                        icon_button(
                            "docker-reconnect",
                            IconName::RefreshCw,
                            t!("docker.reconnect"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.reconnect(cx))),
                    )
                })
                .child(self.action_button(
                    resource_actions(
                        resource,
                        engine.resources.daemon.as_ref().filter(|source| {
                            source.engine_id.as_ref()
                                == engine.info.as_ref().and_then(|info| info.id.as_ref())
                        }),
                    ),
                    Some(resource.clone()),
                    cx,
                ))
                .child(
                    icon_button(
                        "docker-output-toggle",
                        IconName::ScrollText,
                        t!("docker.output"),
                    )
                    .selected(engine.resources.output_visible)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(engine) = this.engine_mut() {
                            engine.resources.output_visible = !engine.resources.output_visible;
                        }
                        cx.notify();
                    })),
                ),
        );
        let content = if engine.resources.output_visible
            && (engine.resources.output_resource.as_ref() == Some(resource)
                || engine.resources.output_resource.is_none())
        {
            div()
                .size_full()
                .min_h_0()
                .child(Editor::new(&engine.resources.output))
                .into_any_element()
        } else if let Some(document) =
            document.filter(|_| *resource == Resource::Daemon || engine.resources.raw())
        {
            div()
                .size_full()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.file_selector(resource, document, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(Editor::new(&document.editor)),
                )
                .into_any_element()
        } else if engine.resources.raw() || *resource == Resource::Events {
            div()
                .size_full()
                .min_h_0()
                .child(Editor::new(&engine.resources.editor))
                .into_any_element()
        } else if let Resource::Project(name) = resource {
            let services: Vec<_> = engine
                .containers
                .iter()
                .filter(|container| {
                    container
                        .labels
                        .as_ref()
                        .and_then(|labels| labels.get("com.docker.compose.project"))
                        == Some(name)
                })
                .map(|container| {
                    (
                        container.id.clone().unwrap_or_default(),
                        service_name(container),
                        container.image.clone().unwrap_or_default(),
                        is_running(container),
                    )
                })
                .collect();
            uniform_list(
                "docker-project-services",
                services.len(),
                cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|index| {
                            let (id, name, image, running) = &services[index];
                            let id = id.clone();
                            resource_row(
                                SharedString::from(format!("docker-service-{id}")),
                                name.clone(),
                                if *running {
                                    IconName::Play
                                } else {
                                    IconName::Square
                                },
                                false,
                                p,
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .max_w(px(200.))
                                    .truncate()
                                    .text_color(rgb(p.muted))
                                    .child(image.clone()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_container(id.clone(), window, cx)
                            }))
                            .into_any_element()
                        })
                        .collect()
                }),
            )
            .size_full()
            .into_any_element()
        } else {
            let value = engine.resources.preview.get(resource);
            let references: Vec<_> = engine
                .containers
                .iter()
                .filter(|container| match resource {
                    Resource::Image(id) => container.image_id.as_ref() == Some(id),
                    Resource::Volume(name) => container.mounts.as_ref().is_some_and(|mounts| {
                        mounts.iter().any(|mount| mount.name.as_ref() == Some(name))
                    }),
                    Resource::Network(_) => value
                        .and_then(|value| value.get("Containers"))
                        .and_then(serde_json::Value::as_object)
                        .is_some_and(|entries| {
                            container
                                .id
                                .as_ref()
                                .is_some_and(|id| entries.contains_key(id))
                        }),
                    _ => false,
                })
                .filter_map(|container| {
                    container
                        .id
                        .clone()
                        .map(|id| (id, container_name(container)))
                })
                .collect();
            let fields = value
                .map(|value| fields(resource, value))
                .unwrap_or_default();
            div()
                .id("docker-resource-overview")
                .size_full()
                .overflow_y_scroll()
                .px_4()
                .py_2()
                .children(fields.into_iter().map(|(label, value)| {
                    div()
                        .py_2()
                        .flex()
                        .gap_3()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .text_size(px(12.))
                        .child(
                            div()
                                .w(px(110.))
                                .flex_shrink_0()
                                .text_color(rgb(p.muted))
                                .child(label),
                        )
                        .child(div().min_w_0().flex_1().whitespace_normal().child(value))
                }))
                .when(!references.is_empty(), |view| {
                    let count = references.len();
                    view.child(
                        div()
                            .py_2()
                            .text_size(px(12.))
                            .text_color(rgb(p.muted))
                            .child(t!("docker.containers").into_owned()),
                    )
                    .child(
                        uniform_list(
                            "docker-referenced-containers",
                            count,
                            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| {
                                        let (id, name) = &references[index];
                                        let id = id.clone();
                                        resource_row(
                                            SharedString::from(format!("docker-reference-{id}")),
                                            name.clone(),
                                            IconName::Box,
                                            false,
                                            p,
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.select_container(id.clone(), window, cx)
                                        }))
                                        .into_any_element()
                                    })
                                    .collect()
                            }),
                        )
                        .h(px((count as f32 * 30.).min(180.))),
                    )
                })
                .into_any_element()
        };
        surface
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .text_size(px(self.font_size.clamp(12., 17.)))
                    .child(content),
            )
            .into_any_element()
    }
    fn file_selector(
        &self,
        resource: &Resource,
        document: &crate::documents::Document,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(engine) = self.engine() else {
            return div().into_any_element();
        };
        let files = match resource {
            Resource::Project(name) => engine
                .resources
                .projects
                .iter()
                .find(|project| &project.name == name)
                .map(|project| project.files.clone())
                .unwrap_or_default(),
            _ => vec![document.file.path.clone()],
        };
        let active = document.file.path.clone();
        if files.len() <= 1 {
            let path = active.clone();
            return div()
                .id("docker-source-path")
                .h(px(28.))
                .px_2()
                .flex_shrink_0()
                .flex()
                .items_center()
                .text_size(px(12.))
                .child(div().min_w_0().truncate().child(active))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx)
                })
                .into_any_element();
        }
        let target = engine.target.clone();
        let resource = resource.clone();
        let owner = cx.entity().downgrade();
        Button::new("docker-config-file")
            .ghost()
            .small()
            .w_full()
            .min_w_0()
            .justify_start()
            .overflow_hidden()
            .label(active.clone())
            .tooltip(active.clone())
            .dropdown_caret(files.len() > 1)
            .dropdown_menu(move |mut menu, _, _| {
                for path in &files {
                    let owner = owner.clone();
                    let path = path.clone();
                    let target = target.clone();
                    let resource = resource.clone();
                    menu = menu.item(
                        PopupMenuItem::new(path.clone())
                            .checked(path == active)
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.select_document(
                                        &target,
                                        resource.clone(),
                                        path.clone(),
                                        window,
                                        cx,
                                    )
                                });
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }
    fn service_selector(&self, resource: &Resource, cx: &mut Context<Self>) -> AnyElement {
        let (Some(engine), Resource::Project(name)) = (self.engine(), resource) else {
            return div().into_any_element();
        };
        let mut services: Vec<_> = engine
            .containers
            .iter()
            .filter(|container| {
                container
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get("com.docker.compose.project"))
                    == Some(name)
            })
            .map(service_name)
            .collect();
        services.sort();
        services.dedup();
        let active = engine.resources.services.get(name).cloned();
        let target = engine.target.clone();
        let name = name.clone();
        let owner = cx.entity().downgrade();
        Button::new("docker-project-service")
            .ghost()
            .small()
            .icon(IconName::Layers)
            .tooltip(t!("docker.service"))
            .label(
                active
                    .clone()
                    .unwrap_or_else(|| t!("docker.project_scope").into_owned()),
            )
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for service in std::iter::once(None).chain(services.iter().cloned().map(Some)) {
                    let owner = owner.clone();
                    let name = name.clone();
                    let target = target.clone();
                    menu = menu.item(
                        PopupMenuItem::new(
                            service
                                .clone()
                                .unwrap_or_else(|| t!("docker.project_scope").into_owned()),
                        )
                        .checked(service == active)
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.target() != Some(&target) {
                                    return;
                                }
                                if let Some(engine) = this.engine_mut() {
                                    if let Some(service) = &service {
                                        engine
                                            .resources
                                            .services
                                            .insert(name.clone(), service.clone());
                                    } else {
                                        engine.resources.services.remove(&name);
                                    }
                                }
                                cx.notify();
                            });
                        }),
                    );
                }
                menu
            })
            .into_any_element()
    }
}
fn resource_actions(resource: &Resource, source: Option<&crate::daemon::Source>) -> Vec<Action> {
    match resource {
        Resource::Project(_) => [
            crate::compose::Action::Up,
            crate::compose::Action::Start,
            crate::compose::Action::Stop,
            crate::compose::Action::Restart,
            crate::compose::Action::Pull,
            crate::compose::Action::Build,
            crate::compose::Action::Logs,
            crate::compose::Action::Down,
        ]
        .into_iter()
        .map(Action::Compose)
        .chain([
            Action::Source,
            Action::Validate,
            Action::Save,
            Action::Restore,
        ])
        .collect(),
        Resource::Image(_) => vec![
            Action::CreateContainer,
            Action::Tag,
            Action::Push,
            Action::ExportImage,
            Action::RemoveImage,
        ],
        Resource::Container(_) => vec![
            Action::Start,
            Action::Stop,
            Action::RestartContainer,
            Action::RemoveContainer,
            Action::Rename,
            Action::Update,
            Action::Recreate,
            Action::Pause,
            Action::Resume,
        ],
        Resource::Volume(_) => vec![Action::RemoveVolume],
        Resource::Network(_) => vec![Action::Connect, Action::Disconnect, Action::RemoveNetwork],
        Resource::Disk => vec![
            Action::PruneContainers,
            Action::PruneImages,
            Action::PruneVolumes,
            Action::PruneNetworks,
            Action::PruneBuild,
        ],
        Resource::Daemon => {
            let mut actions = vec![
                Action::Source,
                Action::Validate,
                Action::Save,
                Action::Restore,
            ];
            if source.is_some_and(|source| {
                matches!(
                    source.manager,
                    crate::daemon::Manager::System { reload: true, .. }
                )
            }) {
                actions.push(Action::Reload);
            }
            if source
                .is_some_and(|source| !matches!(source.manager, crate::daemon::Manager::Manual))
            {
                actions.push(Action::Restart);
            }
            actions
        }
        Resource::Info | Resource::Events => vec![],
    }
}
fn resource_row(
    id: SharedString,
    name: String,
    icon: IconName,
    selected: bool,
    palette: tshell_ui::appearance::Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .h(px(30.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .text_size(px(12.))
        .when(selected, |row| row.bg(rgb(palette.selected)))
        .hover(|row| row.bg(rgb(palette.row_hover())))
        .child(Icon::new(icon).size(px(13.)).text_color(rgb(palette.muted)))
        .child(div().min_w_0().flex_1().truncate().child(name))
}
fn fields(resource: &Resource, value: &serde_json::Value) -> Vec<(String, String)> {
    if *resource == Resource::Disk {
        return [
            ("docker.images", "Images"),
            ("docker.volumes", "Volumes"),
            ("docker.containers", "Containers"),
            ("docker.build_cache", "BuildCache"),
        ]
        .into_iter()
        .map(|(key, field)| {
            let count = value
                .get(field)
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            let text = if field == "Images" {
                format!(
                    "{count} · {}",
                    crate::resources::format_bytes(
                        value
                            .get("LayersSize")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or_default()
                    )
                )
            } else {
                count.to_string()
            };
            (t!(key).into_owned(), text)
        })
        .collect();
    }
    let keys: &[(&str, &str)] = match resource {
        Resource::Image(_) => &[
            ("docker.resource_id", "Id"),
            ("docker.tags", "RepoTags"),
            ("docker.created", "Created"),
            ("docker.size", "Size"),
            ("docker.architecture", "Architecture"),
            ("docker.os", "Os"),
            ("docker.command", "Config.Cmd"),
            ("docker.layers", "RootFS.Layers"),
            ("docker.history", "History"),
        ],
        Resource::Volume(_) => &[
            ("docker.name", "Name"),
            ("docker.driver", "Driver"),
            ("docker.mountpoint", "Mountpoint"),
            ("docker.created", "CreatedAt"),
            ("docker.scope", "Scope"),
            ("docker.options", "Options"),
        ],
        Resource::Network(_) => &[
            ("docker.resource_id", "Id"),
            ("docker.driver", "Driver"),
            ("docker.scope", "Scope"),
            ("docker.internal", "Internal"),
            ("docker.subnets", "IPAM.Config"),
        ],
        Resource::Info => &[
            ("docker.resource_id", "ID"),
            ("docker.name", "Name"),
            ("docker.version", "ServerVersion"),
            ("docker.os", "OperatingSystem"),
            ("docker.architecture", "Architecture"),
            ("docker.storage", "DockerRootDir"),
            ("docker.driver", "Driver"),
            ("docker.security", "SecurityOptions"),
        ],
        Resource::Disk => &[
            ("docker.images", "Images"),
            ("docker.volumes", "Volumes"),
            ("docker.containers", "Containers"),
            ("docker.build_cache", "BuildCache"),
        ],
        _ => &[],
    };
    keys.iter()
        .filter_map(|(key, path)| {
            let entry = path
                .split('.')
                .try_fold(value, |value, part| value.get(part))?;
            if entry.is_null() {
                return None;
            }
            if *path == "Size" {
                return Some((
                    t!(*key).into_owned(),
                    crate::resources::format_bytes(entry.as_u64().unwrap_or_default()),
                ));
            }
            if *path == "History" {
                return Some((
                    t!(*key).into_owned(),
                    entry
                        .as_array()?
                        .iter()
                        .filter_map(|entry| {
                            entry.get("CreatedBy").and_then(serde_json::Value::as_str)
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ));
            }
            let text = match entry {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Array(values)
                    if values.iter().all(|value| value.is_string()) =>
                {
                    values
                        .iter()
                        .filter_map(|value| value.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                }
                _ => serde_json::to_string_pretty(entry).unwrap_or_default(),
            };
            Some((t!(*key).into_owned(), text))
        })
        .collect()
}
