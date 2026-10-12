use crate::{
    DockerView,
    browser::{Collection, rows},
    panel::{Engine, SidebarGroup, is_running},
    resources::{Resource, service_name},
    t,
};
use gpui_kit::{assets::IconName, component::Icon, prelude::*, *};

#[derive(Clone)]
enum Destination {
    Collection(Collection),
    Detail(Resource),
}
impl Destination {
    fn active(&self, engine: &Engine) -> bool {
        match self {
            Self::Collection(collection) => engine.resources.collection == Some(*collection),
            Self::Detail(Resource::Container(id)) => {
                engine.resources.page == crate::ResourcePage::Containers
                    && engine.selected.as_ref() == Some(id)
            }
            Self::Detail(resource) => {
                engine.resources.page == resource.page()
                    && engine.resources.selection() == Some(resource)
            }
        }
    }
    fn open(&self, view: &mut DockerView, window: &mut Window, cx: &mut Context<DockerView>) {
        match self {
            Self::Collection(collection) => view.show_collection(*collection, window, cx),
            Self::Detail(resource) => view.select_resource(resource.clone(), window, cx),
        }
    }
}
#[derive(Clone)]
struct Node {
    key: SharedString,
    label: SharedString,
    icon: IconName,
    depth: u8,
    destination: Destination,
    group: Option<SidebarGroup>,
    project: Option<String>,
}
fn leaf(resource: Resource, label: String, icon: IconName, depth: u8) -> Node {
    Node {
        key: format!("docker-node-{resource:?}").into(),
        label: label.into(),
        icon,
        depth,
        destination: Destination::Detail(resource),
        group: None,
        project: None,
    }
}
fn branches(engine: &Engine, group: SidebarGroup) -> Vec<Node> {
    let collection = match group {
        SidebarGroup::Containers => Collection::Containers,
        SidebarGroup::Compose => Collection::Projects,
        SidebarGroup::Images => Collection::Images,
        SidebarGroup::Engine => {
            return [
                (
                    Destination::Detail(Resource::Info),
                    IconName::Info,
                    "docker.overview",
                ),
                (
                    Destination::Collection(Collection::Volumes),
                    IconName::HardDrive,
                    "docker.volumes",
                ),
                (
                    Destination::Collection(Collection::Networks),
                    IconName::Network,
                    "docker.networks",
                ),
                (
                    Destination::Detail(Resource::Disk),
                    IconName::HardDrive,
                    "docker.storage",
                ),
                (
                    Destination::Detail(Resource::Events),
                    IconName::ScrollText,
                    "docker.events",
                ),
                (
                    Destination::Detail(Resource::Daemon),
                    IconName::Settings,
                    "docker.engine_config",
                ),
            ]
            .into_iter()
            .map(|(destination, icon, key)| Node {
                key: key.into(),
                label: t!(key).into_owned().into(),
                icon,
                depth: 1,
                destination,
                group: None,
                project: None,
            })
            .collect();
        }
    };
    let mut nodes = Vec::new();
    let mut services = std::collections::BTreeMap::<&str, Vec<_>>::new();
    let running: std::collections::BTreeSet<_> = engine
        .containers
        .iter()
        .filter(|container| is_running(container))
        .filter_map(|container| container.id.as_deref())
        .collect();
    if group == SidebarGroup::Compose {
        for container in &engine.containers {
            if let Some(project) = container
                .labels
                .as_ref()
                .and_then(|labels| labels.get("com.docker.compose.project"))
            {
                services.entry(project).or_default().push(container);
            }
        }
    }
    for row in rows(engine, collection, "") {
        let project = if let Resource::Project(name) = &row.resource {
            Some(name.clone())
        } else {
            None
        };
        let icon = match &row.resource {
            Resource::Container(id) => {
                if running.contains(id.as_str()) {
                    IconName::Play
                } else {
                    IconName::Square
                }
            }
            Resource::Project(_) => IconName::Layers,
            _ => IconName::Image,
        };
        let mut node = leaf(row.resource, row.cells[0].clone(), icon, 1);
        node.project = project.clone();
        nodes.push(node);
        if let Some(project) =
            project.filter(|project| engine.resources.sidebar_projects.contains(project))
        {
            if let Some(containers) = services.get(project.as_str()) {
                let mut children: Vec<_> = containers
                    .iter()
                    .filter_map(|container| {
                        let id = container.id.clone()?;
                        Some(leaf(
                            Resource::Container(id),
                            service_name(container),
                            if is_running(container) {
                                IconName::Play
                            } else {
                                IconName::Square
                            },
                            2,
                        ))
                    })
                    .collect();
                children.sort_by(|left, right| left.label.cmp(&right.label));
                nodes.extend(children);
            }
        }
    }
    nodes
}
impl DockerView {
    #[cfg(debug_assertions)]
    pub fn check_image_tree_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.show_collection(Collection::Images, window, cx);
        let engine = self.engine_mut().expect("Docker tree fixture");
        engine.resources.sidebar_group = Some(SidebarGroup::Images);
        engine.resources.sidebar_cursor = Some("docker.images".into());
        engine.resources.focus.clone()
    }
    fn toggle_sidebar_node(
        &mut self,
        node: &Node,
        expanded: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(engine) = self.engine_mut() {
            if let Some(group) = node.group {
                engine.resources.sidebar_group = expanded.then_some(group);
            } else if let Some(project) = &node.project {
                if expanded {
                    engine.resources.sidebar_projects.insert(project.clone());
                } else {
                    engine.resources.sidebar_projects.remove(project);
                }
            }
            engine.resources.sidebar_cursor = Some(node.key.clone());
        }
        if expanded && node.group.is_some() {
            node.destination.open(self, window, cx);
        }
        cx.notify();
    }
    fn sidebar_node(&self, node: Node, cx: &mut Context<Self>) -> AnyElement {
        let Some(engine) = self.engine() else {
            return div().into_any_element();
        };
        let active = node.destination.active(engine);
        let expanded = node
            .group
            .is_some_and(|group| engine.resources.sidebar_group == Some(group))
            || node
                .project
                .as_ref()
                .is_some_and(|project| engine.resources.sidebar_projects.contains(project));
        let has_children = node.group.is_some() || node.project.is_some();
        let toggle_node = node.clone();
        let open_node = node.clone();
        let label = node.label.clone();
        div()
            .id(node.key.clone())
            .role(Role::TreeItem)
            .aria_label(label.clone())
            .aria_selected(active)
            .when(has_children, |row| row.aria_expanded(expanded))
            .h(px(30.))
            .w_full()
            .flex_shrink_0()
            .pl(px(4. + f32::from(node.depth) * 14.))
            .pr_2()
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .text_size(px(12.))
            .text_color(rgb(self.palette.text))
            .when(active, |row| row.bg(rgb(self.palette.selected)))
            .hover(|row| row.bg(rgb(self.palette.row_hover())))
            .child(
                div()
                    .id(SharedString::from(format!("{}-toggle", node.key)))
                    .w(px(18.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(has_children, |toggle| {
                        toggle.child(
                            Icon::new(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(12.)),
                        )
                    })
                    .when(has_children, |toggle| {
                        toggle.on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle_sidebar_node(&toggle_node, !expanded, window, cx);
                            cx.stop_propagation();
                        }))
                    }),
            )
            .child(
                Icon::new(node.icon)
                    .size(px(13.))
                    .text_color(rgb(self.palette.muted)),
            )
            .child(div().min_w_0().flex_1().truncate().child(node.label))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(engine) = this.engine_mut() {
                    engine.resources.sidebar_cursor = Some(open_node.key.clone());
                    if let Some(group) = open_node.group {
                        engine.resources.sidebar_group = Some(group);
                    }
                }
                open_node.destination.open(this, window, cx);
            }))
            .into_any_element()
    }
    pub fn sidebar(&self, _width: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(engine) = self.engine() else {
            return div().into_any_element();
        };
        let focus = engine.resources.focus.clone();
        let scroll = engine.resources.sidebar_scrolls[&engine
            .resources
            .sidebar_group
            .unwrap_or(SidebarGroup::Containers)]
            .clone();
        let groups = [
            (
                SidebarGroup::Containers,
                Destination::Collection(Collection::Containers),
                IconName::Box,
                Collection::Containers.key(),
            ),
            (
                SidebarGroup::Compose,
                Destination::Collection(Collection::Projects),
                IconName::Layers,
                Collection::Projects.key(),
            ),
            (
                SidebarGroup::Images,
                Destination::Collection(Collection::Images),
                IconName::Image,
                Collection::Images.key(),
            ),
            (
                SidebarGroup::Engine,
                Destination::Detail(Resource::Info),
                IconName::Server,
                "docker.engine_info",
            ),
        ];
        let mut navigation = Vec::new();
        let mut sections = Vec::new();
        for (group, destination, icon, key) in groups {
            let root = Node {
                key: key.into(),
                label: t!(key).into_owned().into(),
                icon,
                depth: 0,
                destination,
                group: Some(group),
                project: None,
            };
            navigation.push(root.clone());
            let expanded = engine.resources.sidebar_group == Some(group);
            let children = if expanded {
                branches(engine, group)
            } else {
                Vec::new()
            };
            navigation.extend(children.iter().cloned());
            let count = children.len();
            let mut section = div()
                .min_h_0()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .when(expanded, |section| section.flex_1())
                .child(self.sidebar_node(root, cx));
            if expanded && count > 0 {
                section = section.child(
                    uniform_list(
                        ("docker-navigation-children", group as usize),
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| this.sidebar_node(children[index].clone(), cx))
                                .collect()
                        }),
                    )
                    .track_scroll(&scroll)
                    .flex_1()
                    .min_h_0(),
                );
            }
            sections.push(section.into_any_element());
        }
        div()
            .id("docker-menu")
            .role(Role::Tree)
            .size_full()
            .flex()
            .flex_col()
            .py_1()
            .track_focus(&focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, window, cx| focus.focus(window, cx)),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let current = this
                    .engine()
                    .and_then(|engine| {
                        navigation.iter().position(|node| {
                            engine.resources.sidebar_cursor.as_ref() == Some(&node.key)
                        })
                    })
                    .unwrap_or(0);
                let node = &navigation[current];
                let next = match event.keystroke.key.as_str() {
                    "down" => Some((current + 1).min(navigation.len() - 1)),
                    "up" => Some(current.saturating_sub(1)),
                    "home" => Some(0),
                    "end" => Some(navigation.len() - 1),
                    "left" | "right" if node.group.is_some() || node.project.is_some() => {
                        this.toggle_sidebar_node(node, event.keystroke.key == "right", window, cx);
                        None
                    }
                    "enter" => Some(current),
                    _ => return,
                };
                if let Some(index) = next {
                    let node = &navigation[index];
                    if let Some(engine) = this.engine_mut() {
                        engine.resources.sidebar_cursor = Some(node.key.clone());
                        if let Some(group) = node.group {
                            engine.resources.sidebar_group = Some(group);
                        }
                    }
                    node.destination.open(this, window, cx);
                    if node.group.is_none() {
                        let child_index = navigation[..index]
                            .iter()
                            .rev()
                            .take_while(|node| node.group.is_none())
                            .count();
                        scroll.scroll_to_item(child_index, ScrollStrategy::Top);
                    }
                }
                cx.stop_propagation();
                window.prevent_default();
            }))
            .children(sections)
            .into_any_element()
    }
}
