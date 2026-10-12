use crate::{
    DockerView, ResourcePage,
    actions::Action,
    panel::{Engine, container_name},
    resources::{Resource, format_image_size, image_references, short_id},
    t,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{Sizable, input::Input, menu::ContextMenuExt};
use gpui_kit::{prelude::*, *};
use tshell_ui::controls::icon_button;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Collection {
    Containers,
    Projects,
    Images,
    Volumes,
    Networks,
}
impl Collection {
    pub const ALL: [Self; 5] = [
        Self::Containers,
        Self::Projects,
        Self::Images,
        Self::Volumes,
        Self::Networks,
    ];
    pub fn for_page(page: ResourcePage) -> Option<Self> {
        match page {
            ResourcePage::Containers => Some(Self::Containers),
            ResourcePage::Compose => Some(Self::Projects),
            ResourcePage::Images => Some(Self::Images),
            ResourcePage::More => None,
        }
    }
    pub(crate) fn for_resource(resource: &Resource) -> Option<Self> {
        match resource {
            Resource::Container(_) => Some(Self::Containers),
            Resource::Project(_) => Some(Self::Projects),
            Resource::Image(_) => Some(Self::Images),
            Resource::Volume(_) => Some(Self::Volumes),
            Resource::Network(_) => Some(Self::Networks),
            _ => None,
        }
    }
    pub(crate) fn page(self) -> ResourcePage {
        match self {
            Self::Containers => ResourcePage::Containers,
            Self::Projects => ResourcePage::Compose,
            Self::Images => ResourcePage::Images,
            Self::Volumes | Self::Networks => ResourcePage::More,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Containers => "docker.containers",
            Self::Projects => "docker.compose",
            Self::Images => "docker.images",
            Self::Volumes => "docker.volumes",
            Self::Networks => "docker.networks",
        }
    }
    fn columns(self) -> [&'static str; 4] {
        match self {
            Self::Containers => [
                "docker.name",
                "docker.state",
                "docker.image",
                "docker.ports",
            ],
            Self::Projects => [
                "docker.name",
                "docker.containers",
                "docker.source_file",
                "docker.directory",
            ],
            Self::Images => [
                "docker.tags",
                "docker.resource_id",
                "docker.size",
                "docker.containers",
            ],
            Self::Volumes => [
                "docker.name",
                "docker.driver",
                "docker.scope",
                "docker.mountpoint",
            ],
            Self::Networks => [
                "docker.name",
                "docker.driver",
                "docker.scope",
                "docker.subnets",
            ],
        }
    }
    fn create_actions(self) -> Vec<Action> {
        match self {
            Self::Containers => vec![Action::CreateContainer],
            Self::Projects => vec![Action::ImportProject],
            Self::Images => vec![Action::Pull, Action::ImportImage],
            Self::Volumes => vec![Action::CreateVolume],
            Self::Networks => vec![Action::CreateNetwork],
        }
    }
}

pub(crate) struct Row {
    pub resource: Resource,
    pub cells: [String; 4],
}
pub(crate) fn rows(engine: &Engine, collection: Collection, query: &str) -> Vec<Row> {
    let mut project_counts = std::collections::BTreeMap::new();
    if collection == Collection::Projects {
        for container in &engine.containers {
            if let Some(project) = container
                .labels
                .as_ref()
                .and_then(|labels| labels.get("com.docker.compose.project"))
            {
                *project_counts.entry(project.as_str()).or_insert(0usize) += 1;
            }
        }
    }
    let mut rows: Vec<_> = match collection {
        Collection::Containers => engine
            .containers
            .iter()
            .filter_map(|container| {
                let id = container.id.clone()?;
                Some(Row {
                    resource: Resource::Container(id),
                    cells: [
                        container_name(container),
                        container.status.clone().unwrap_or_else(|| {
                            container
                                .state
                                .map(|state| state.to_string())
                                .unwrap_or_default()
                        }),
                        container.image.clone().unwrap_or_default(),
                        container
                            .ports
                            .as_deref()
                            .unwrap_or_default()
                            .iter()
                            .map(|port| {
                                let private = format!(
                                    "{}/{}",
                                    port.private_port,
                                    port.typ
                                        .as_ref()
                                        .map(ToString::to_string)
                                        .unwrap_or_default()
                                );
                                port.public_port
                                    .map(|public| {
                                        format!(
                                            "{}:{public} → {private}",
                                            port.ip.as_deref().unwrap_or("*")
                                        )
                                    })
                                    .unwrap_or(private)
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    ],
                })
            })
            .collect(),
        Collection::Projects => engine
            .resources
            .projects
            .iter()
            .map(|project| Row {
                resource: Resource::Project(project.name.clone()),
                cells: [
                    project.name.clone(),
                    project_counts
                        .get(project.name.as_str())
                        .copied()
                        .unwrap_or_default()
                        .to_string(),
                    project.files.join(", "),
                    project.directory.clone().unwrap_or_default(),
                ],
            })
            .collect(),
        Collection::Images => {
            let by_id: std::collections::BTreeMap<_, _> = engine
                .resources
                .images
                .iter()
                .map(|image| (image.id.as_str(), image))
                .collect();
            image_references(&engine.resources.images)
                .into_iter()
                .filter_map(|reference| {
                    let image = by_id.get(reference.id.as_str())?;
                    Some(Row {
                        cells: [
                            reference
                                .tag
                                .clone()
                                .unwrap_or_else(|| "<none>:<none>".into()),
                            short_id(&image.id).into(),
                            format_image_size(image.size.max(0) as u64),
                            if image.containers < 0 {
                                "—".into()
                            } else {
                                image.containers.to_string()
                            },
                        ],
                        resource: Resource::Image(reference),
                    })
                })
                .collect()
        }
        Collection::Volumes => engine
            .resources
            .volumes
            .iter()
            .map(|volume| Row {
                resource: Resource::Volume(volume.name.clone()),
                cells: [
                    volume.name.clone(),
                    volume.driver.clone(),
                    volume
                        .scope
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    volume.mountpoint.clone(),
                ],
            })
            .collect(),
        Collection::Networks => engine
            .resources
            .networks
            .iter()
            .filter_map(|network| {
                Some(Row {
                    resource: Resource::Network(network.id.clone()?),
                    cells: [
                        network.name.clone().unwrap_or_default(),
                        network.driver.clone().unwrap_or_default(),
                        network.scope.clone().unwrap_or_default(),
                        network
                            .ipam
                            .as_ref()
                            .and_then(|ipam| ipam.config.as_ref())
                            .map(|configs| {
                                configs
                                    .iter()
                                    .filter_map(|config| config.subnet.clone())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default(),
                    ],
                })
            })
            .collect(),
    };
    rows.retain(|row| {
        row.cells
            .iter()
            .any(|cell| cell.to_lowercase().contains(query))
            || match &row.resource {
                Resource::Container(id) | Resource::Network(id) => {
                    id.to_lowercase().contains(query)
                }
                Resource::Image(image) => image.id.to_lowercase().contains(query),
                _ => false,
            }
    });
    if collection != Collection::Images {
        rows.sort_by(|left, right| left.cells[0].cmp(&right.cells[0]));
    }
    rows
}
fn selected(engine: &Engine) -> Option<Resource> {
    if engine.resources.page == ResourcePage::Containers {
        engine.selected.clone().map(Resource::Container)
    } else {
        engine.resources.selection().cloned()
    }
}

impl DockerView {
    pub(crate) fn collection_visible(&self) -> bool {
        self.engine().is_some_and(|engine| {
            engine.resources.collection.is_some()
                && !engine.resources.output_visible
                && !engine.resources.raw()
                && (engine.resources.page != ResourcePage::Containers
                    || engine.tab == crate::panel::Tab::Overview)
        })
    }
    pub(crate) fn show_collection(
        &mut self,
        collection: Collection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .engine()
            .is_some_and(|engine| engine.resources.page != collection.page())
        {
            self.set_resource_page(collection.page(), window, cx);
        }
        if let Some(engine) = self.engine_mut() {
            engine.resources.collection = Some(collection);
            engine.resources.inspector = false;
            engine.resources.output_visible = false;
            engine.resources.raw.remove(&collection.page());
            if collection == Collection::Containers {
                engine.tab = crate::panel::Tab::Overview;
            }
        }
        cx.notify();
    }
    pub(crate) fn navigate_resource(
        &mut self,
        resource: Resource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let collection = self
            .engine()
            .and_then(|engine| engine.resources.collection)
            .filter(|collection| Some(*collection) == Collection::for_resource(&resource));
        self.select_resource(resource, window, cx);
        if let Some(collection) = collection {
            if let Some(engine) = self.engine_mut() {
                engine.resources.collection = Some(collection);
                engine.resources.inspector = true;
            }
        }
        cx.notify();
    }
    pub(crate) fn browser_controls(
        &self,
        resource: &Resource,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(collection) = Collection::for_resource(resource) else {
            return div().into_any_element();
        };
        let split = self.collection_visible();
        div()
            .flex()
            .items_center()
            .gap_1()
            .when(!split, |bar| {
                bar.child(
                    icon_button(
                        "docker-browse",
                        IconName::ArrowLeft,
                        t!("docker.back_to_list"),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_collection(collection, window, cx)
                    })),
                )
            })
            .when(split, |bar| {
                bar.child(
                    icon_button(
                        "docker-expand-detail",
                        IconName::Maximize,
                        t!("docker.expand_detail"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(engine) = this.engine_mut() {
                            engine.resources.collection = None;
                        }
                        cx.notify();
                    })),
                )
                .child(
                    icon_button(
                        "docker-close-detail",
                        IconName::Close,
                        t!("docker.close_detail"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(engine) = this.engine_mut() {
                            engine.resources.inspector = false;
                        }
                        cx.notify();
                    })),
                )
            })
            .into_any_element()
    }
    pub(crate) fn browser_surface(
        &self,
        available: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(engine) = self.engine() else {
            return self.surface(cx);
        };
        let Some(collection) = engine.resources.collection else {
            return self.surface(cx);
        };
        if engine.resources.output_visible
            || engine.resources.raw()
            || (collection == Collection::Containers && engine.tab != crate::panel::Tab::Overview)
        {
            return self.surface(cx);
        }
        let width = f32::from(available.width);
        let inspect = engine.resources.inspector
            && selected(engine).as_ref().and_then(Collection::for_resource) == Some(collection);
        if !inspect {
            return self.collection_table(collection, width, cx);
        }
        let horizontal = width >= 960.;
        let table_width = if horizontal { width * 0.56 } else { width };
        let table = self.collection_table(collection, table_width, cx);
        div()
            .size_full()
            .flex()
            .when(!horizontal, |layout| layout.flex_col())
            .child(
                div()
                    .min_w_0()
                    .min_h_0()
                    .flex_shrink_0()
                    .when(horizontal, |pane| pane.w(px(table_width)).h_full())
                    .when(!horizontal, |pane| pane.w_full().h(available.height * 0.42))
                    .child(table),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .bg(rgb(self.palette.border))
                    .when(horizontal, |line| line.w(px(1.)).h_full())
                    .when(!horizontal, |line| line.h(px(1.)).w_full()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.surface(cx)),
            )
            .into_any_element()
    }
    fn collection_table(
        &self,
        collection: Collection,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(engine) = self.engine() else {
            return div().into_any_element();
        };
        let query = if collection == Collection::Containers {
            &engine.query
        } else {
            &engine.resources.queries[&collection.page()]
        };
        let rows = rows(engine, collection, &query.read(cx).value().to_lowercase());
        let navigation: Vec<_> = rows.iter().map(|row| row.resource.clone()).collect();
        let count = rows.len();
        let current = selected(engine);
        let columns = if width >= 740. {
            4
        } else if width >= 500. {
            3
        } else {
            2
        };
        let name_weight = if collection == Collection::Images {
            4.
        } else {
            2.
        };
        let focus = engine.resources.table_focus.clone();
        let scroll = engine.resources.table_scrolls[&collection].clone();
        let palette = self.palette;
        let list = uniform_list(
            ("docker-resource-table-rows", collection as usize),
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| {
                        let row = &rows[index];
                        let resource = row.resource.clone();
                        let owner = cx.entity().downgrade();
                        let target = this.target().cloned();
                        let actions = crate::resource_view::resource_actions(&resource, None);
                        let menu_resource = resource.clone();
                        div()
                            .id(("docker-table-row", index))
                            .h(px(32.))
                            .w_full()
                            .px_2()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(12.))
                            .text_color(rgb(palette.text))
                            .when(current.as_ref() == Some(&resource), |row| {
                                row.bg(rgb(palette.selected))
                            })
                            .hover(|row| row.bg(rgb(palette.row_hover())))
                            .children(row.cells.iter().take(columns).enumerate().map(
                                |(column, value)| {
                                    div()
                                        .id(("docker-table-cell", column))
                                        .min_w_0()
                                        .flex_basis(px(0.))
                                        .flex_grow(if column == 0 { name_weight } else { 1. })
                                        .px_2()
                                        .truncate()
                                        .tooltip({
                                            let value = match (&row.resource, column) {
                                                (Resource::Image(image), 1) => image.id.clone(),
                                                _ => value.clone(),
                                            };
                                            move |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    value.clone(),
                                                )
                                                .build(window, cx)
                                            }
                                        })
                                        .child(if value.is_empty() {
                                            "—".into()
                                        } else {
                                            value.clone()
                                        })
                                },
                            ))
                            .child(this.action_button(actions.clone(), Some(resource.clone()), cx))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.navigate_resource(resource.clone(), window, cx)
                            }))
                            .context_menu(move |menu, _, _| {
                                crate::panel::action_menu(
                                    menu,
                                    actions.clone(),
                                    target.clone(),
                                    Some(menu_resource.clone()),
                                    owner.clone(),
                                )
                            })
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .track_scroll(&scroll)
        .size_full();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(palette.panel))
            .text_color(rgb(palette.text))
            .child(
                div()
                    .h(px(38.))
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(query).small().cleanable(true)),
                    )
                    .child(self.action_button(collection.create_actions(), None, cx))
                    .child(
                        icon_button(
                            "docker-table-refresh",
                            IconName::RefreshCw,
                            t!("ws.refresh"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                div()
                    .h(px(30.))
                    .px_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .text_size(px(12.))
                    .text_color(rgb(palette.muted))
                    .children(
                        collection
                            .columns()
                            .into_iter()
                            .take(columns)
                            .enumerate()
                            .map(|(column, label)| {
                                div()
                                    .min_w_0()
                                    .flex_basis(px(0.))
                                    .flex_grow(if column == 0 { name_weight } else { 1. })
                                    .px_2()
                                    .truncate()
                                    .child(t!(label).into_owned())
                            }),
                    )
                    .child(div().w(px(28.)).flex_shrink_0()),
            )
            .child(
                div()
                    .id("docker-resource-table")
                    .track_focus(&focus)
                    .min_h_0()
                    .flex_1()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _, window, cx| focus.focus(window, cx)),
                    )
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        let current = this.engine().and_then(selected);
                        let position = current.as_ref().and_then(|current| {
                            navigation.iter().position(|resource| resource == current)
                        });
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
                            "escape" => {
                                if let Some(engine) = this.engine_mut() {
                                    engine.resources.inspector = false;
                                }
                                cx.notify();
                                cx.stop_propagation();
                                None
                            }
                            "enter" if position.is_some() => {
                                if let Some(engine) = this.engine_mut() {
                                    engine.resources.collection = None;
                                }
                                cx.notify();
                                cx.stop_propagation();
                                None
                            }
                            _ => None,
                        };
                        if let Some(index) = next {
                            if let Some(resource) = navigation.get(index) {
                                this.navigate_resource(resource.clone(), window, cx);
                                scroll.scroll_to_item(index, ScrollStrategy::Top);
                                cx.stop_propagation();
                            }
                        }
                    }))
                    .child(self.content_feedback(
                        "docker-table-feedback",
                        if count == 0 {
                            tshell_ui::controls::empty_state(
                                IconName::Search,
                                t!("docker.no_resources").into_owned(),
                                palette,
                            )
                            .into_any_element()
                        } else {
                            list.into_any_element()
                        },
                        engine.loading || self.collection_loading(),
                        engine.error.as_ref(),
                        count == 0,
                    )),
            )
            .into_any_element()
    }
}
