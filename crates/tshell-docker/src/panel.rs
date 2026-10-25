use super::{
    Endpoint, Target, i18n,
    logs::LogBuffer,
    resources::{Resource, ResourcePage, Resources},
    t,
};
use bollard::{
    Docker,
    models::{ContainerInspectResponse, ContainerSummary, ContainerSummaryStateEnum, SystemInfo},
    query_parameters::{EventsOptionsBuilder, ListContainersOptionsBuilder, LogsOptionsBuilder},
};
use futures_util::StreamExt;
use gpui_kit::component::{
    WindowExt,
    input::{EditorState, InputEvent, InputState},
    notification::Notification,
    scroll::ScrollableElement,
};
use gpui_kit::{prelude::*, *};
#[cfg(debug_assertions)]
use std::collections::HashMap;
use std::{collections::BTreeMap, time::Duration};
use tshell_host::{Failure, HostId};
use tshell_ui::appearance::Palette;
#[derive(Clone)]
pub enum Event {
    OpenTerminal {
        target: Target,
        docker: Docker,
        container: String,
        name: String,
        command: Vec<String>,
    },
    LocalEndpointChanged(Option<String>),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Overview,
    Logs,
    Inspect,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SidebarGroup {
    Containers,
    Compose,
    Images,
    Engine,
}
pub(crate) struct Engine {
    pub target: Target,
    pub docker: Option<Docker>,
    pub info: Option<SystemInfo>,
    pub containers: Vec<ContainerSummary>,
    pub selected: Option<String>,
    pub detail: Option<ContainerInspectResponse>,
    pub stats: Option<serde_json::Value>,
    pub loading: bool,
    pub error: Option<Failure>,
    pub busy: Option<String>,
    pub tab: Tab,
    pub resources: Resources,
    pub query: Entity<InputState>,
    pub logs_query: Entity<InputState>,
    pub logs_editor: Entity<EditorState>,
    pub inspect_editor: Entity<EditorState>,
    pub log: LogBuffer,
    pub logs_dirty: bool,
    pub paused: bool,
    pub auto_scroll: bool,
    pub logs_error: Option<Failure>,
    pub logs_loading: bool,
}
struct Worker(tokio::task::JoinHandle<()>);
#[derive(Clone)]
enum FormField {
    Input(Entity<InputState>),
    Json(Entity<EditorState>),
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.0.abort();
    }
}
type Outcome<T> = Result<T, Failure>;
fn failure(error: impl Into<anyhow::Error>) -> Failure {
    let error = error.into();
    Failure::from(&error)
}
fn worker(future: impl std::future::Future<Output = ()> + Send + 'static) -> Worker {
    Worker(tshell_host::ssh::runtime().spawn(future))
}
pub(crate) fn list_options() -> bollard::query_parameters::ListContainersOptions {
    ListContainersOptionsBuilder::default().all(true).build()
}
enum Message {
    Connected(
        Target,
        u64,
        Outcome<(Docker, SystemInfo, Vec<ContainerSummary>)>,
    ),
    Listed(Target, u64, Outcome<Vec<ContainerSummary>>),
    Detail(
        Target,
        String,
        u64,
        Outcome<(ContainerInspectResponse, Option<serde_json::Value>)>,
    ),
    Log(Target, String, u64, Vec<u8>),
    LogEnd(Target, String, u64, Option<Failure>),

    Refresh(Target),
    EventsError(Target, Failure),
    Resources(
        Target,
        ResourcePage,
        u64,
        Outcome<crate::resources::Snapshot>,
    ),
    Preview(Target, Resource, u64, Outcome<serde_json::Value>),
    Document(
        Target,
        Resource,
        u64,
        Outcome<(crate::documents::File, Option<crate::daemon::Source>)>,
    ),
    Projects(Target, u64, Outcome<Vec<crate::resources::Project>>),
    EngineEvent(Target, serde_json::Value),
    Managed(
        Target,
        Option<Resource>,
        crate::actions::Action,
        String,
        Outcome<crate::actions::Outcome>,
    ),
    Progress(Target, String),
}
pub struct DockerView {
    pub(crate) targets: Vec<Target>,
    pub(crate) selected: Option<HostId>,
    pub(crate) engines: BTreeMap<HostId, Engine>,
    pub palette: Palette,
    pub terminal_palette: Palette,
    pub font_family: String,
    pub font_size: f32,
    pub(crate) visible: bool,
    query_epoch: u64,
    detail_epoch: u64,
    logs_epoch: u64,
    refresh_pending: bool,
    sender: async_channel::Sender<Message>,
    query_worker: Option<Worker>,
    detail_worker: Option<Worker>,
    logs_worker: Option<Worker>,
    events_worker: Option<Worker>,
    operations: BTreeMap<HostId, Worker>,
    resource_epoch: u64,
    preview_epoch: u64,
    resource_worker: Option<Worker>,
    preview_worker: Option<Worker>,
    project_worker: Option<Worker>,
    pending_form: Option<(Target, Resource, crate::actions::Action)>,
    #[cfg(debug_assertions)]
    fixture: bool,
    _updates: Task<()>,
}
impl EventEmitter<Event> for DockerView {}
impl DockerView {
    pub(crate) fn collection_loading(&self) -> bool {
        self.resource_worker.is_some() || self.project_worker.is_some()
    }

    pub(crate) fn preview_loading(&self) -> bool {
        self.preview_worker.is_some()
    }

    pub(crate) fn detail_loading(&self) -> bool {
        self.detail_worker.is_some()
    }

    pub(crate) fn discard_document(
        &mut self,
        target: Target,
        resource: Resource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.matches(&target) {
            return;
        }
        let Some(document) = self
            .engine()
            .and_then(|engine| engine.resources.document(&resource))
        else {
            return;
        };
        let path = document.file.path.clone();
        let draft = document.editor.read(cx).value().to_string();
        let receiver = window.prompt(
            PromptLevel::Warning,
            &t!("docker.discard_changes"),
            Some(&format!("{} · {path}", target.name)),
            &[
                t!("explorer.cancel").as_ref(),
                t!("explorer.confirm").as_ref(),
            ],
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            if receiver.await == Ok(1) {
                let _ = view.update_in(cx, |this, window, cx| {
                    if let Some(engine) = this.engines.get(&target.id).filter(|engine| {
                        engine.target.endpoint == target.endpoint && engine.busy.is_none()
                    }) {
                        if let Some(document) =
                            engine.resources.documents.get(&path).filter(|document| {
                                document.editor.read(cx).value().as_ref() == draft
                            })
                        {
                            document.discard(window, cx);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }
    pub(crate) fn select_document(
        &mut self,
        target: &Target,
        resource: Resource,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.matches(target) {
            return;
        }
        self.preview_worker = None;
        self.preview_epoch += 1;
        if let Some(engine) = self.engine_mut() {
            engine.resources.active_files.insert(resource, path);
        }
        self.load_resource_preview(window, cx);
        cx.notify();
    }
    pub fn new(
        targets: Vec<Target>,
        palette: Palette,
        terminal_palette: Palette,
        font_family: String,
        font_size: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (sender, receiver) = async_channel::bounded(64);
        let updates = cx.spawn_in(window, async move |view, cx| {
            while let Ok(message) = receiver.recv().await {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
                let mut messages = vec![message];
                while messages.len() < 64 {
                    let Ok(message) = receiver.try_recv() else {
                        break;
                    };
                    messages.push(message);
                }
                if view
                    .update_in(cx, |this, window, cx| {
                        for message in messages {
                            this.apply_message(message, window, cx);
                        }
                        this.sync_logs(window, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            targets,
            selected: None,
            engines: BTreeMap::new(),
            palette,
            terminal_palette,
            font_family,
            font_size,
            visible: false,
            query_epoch: 0,
            detail_epoch: 0,
            logs_epoch: 0,
            refresh_pending: false,
            sender,
            query_worker: None,
            detail_worker: None,
            logs_worker: None,
            events_worker: None,
            operations: BTreeMap::new(),
            resource_epoch: 0,
            preview_epoch: 0,
            resource_worker: None,
            preview_worker: None,
            project_worker: None,
            pending_form: None,
            #[cfg(debug_assertions)]
            fixture: false,
            _updates: updates,
        }
    }
    pub fn selected_target(&self) -> Option<HostId> {
        self.selected.clone()
    }
    pub fn target(&self) -> Option<&Target> {
        let id = self.selected.as_ref()?;
        self.targets.iter().find(|target| &target.id == id)
    }
    pub(crate) fn engine(&self) -> Option<&Engine> {
        self.engines.get(self.selected.as_ref()?)
    }
    pub(crate) fn engine_mut(&mut self) -> Option<&mut Engine> {
        self.engines.get_mut(self.selected.as_ref()?)
    }
    pub fn set_targets(
        &mut self,
        targets: Vec<Target>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.targets == targets {
            return;
        }
        let old = self.target().cloned();
        self.targets = targets;
        self.engines.retain(|id, engine| {
            self.targets
                .iter()
                .any(|target| &target.id == id && target.endpoint == engine.target.endpoint)
        });
        for target in &self.targets {
            if let Some(engine) = self.engines.get_mut(&target.id) {
                engine.target = target.clone();
            }
        }
        if old.as_ref().map(|target| &target.endpoint)
            != self.target().map(|target| &target.endpoint)
        {
            self.cancel_browsing();
            if self.visible {
                if let Some(target) = self.target().cloned() {
                    self.select_target(target.id, window, cx);
                }
            }
        }
        cx.notify();
    }
    pub fn set_visible(
        &mut self,
        visible: bool,
        host_id: HostId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target_changed = self.selected.as_ref() != Some(&host_id);
        if self.visible == visible && !target_changed {
            return;
        }
        self.visible = visible;
        if !visible {
            self.cancel_browsing();
            self.selected = Some(host_id);
        } else {
            self.select_target(host_id, window, cx);
        }
        cx.notify();
    }
    fn cancel_browsing(&mut self) {
        self.pending_form = None;
        self.query_epoch += 1;
        self.detail_epoch += 1;
        self.logs_epoch += 1;
        self.query_worker = None;
        self.detail_worker = None;
        self.logs_worker = None;
        self.events_worker = None;
        self.resource_epoch += 1;
        self.preview_epoch += 1;
        self.resource_worker = None;
        self.preview_worker = None;
        self.project_worker = None;
        self.refresh_pending = false;
        if let Some(engine) = self.engine_mut() {
            engine.loading = false;
            engine.logs_loading = false;
        }
    }
    pub(crate) fn select_target(
        &mut self,
        id: HostId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.targets.iter().find(|target| target.id == id).cloned() else {
            return;
        };
        self.cancel_browsing();
        self.selected = Some(id.clone());
        if !self.engines.contains_key(&id) {
            let query = cx
                .new(|cx| InputState::new(window, cx).placeholder(t!("docker.search_containers")));
            let logs_query =
                cx.new(|cx| InputState::new(window, cx).placeholder(t!("docker.search_logs")));
            let logs_editor = cx.new(|cx| {
                let mut editor = EditorState::new(window, cx).language("plaintext");
                editor.set_readonly(true, cx);
                editor
            });
            let inspect_editor = cx.new(|cx| {
                let mut editor = EditorState::new(window, cx).language("json");
                editor.set_highlighter_factory(tshell_ui::editor::factory(), cx);
                editor.set_readonly(true, cx);
                editor
            });
            cx.observe(&query, |_, _, cx| cx.notify()).detach();
            let editor = logs_editor.clone();
            cx.subscribe(&logs_query, move |_, query, event, cx| {
                if matches!(event, InputEvent::Change) {
                    let query = query.read(cx).value().to_string();
                    editor.update(cx, |editor, cx| editor.set_search_query(query, true, cx));
                }
            })
            .detach();
            self.engines.insert(
                id,
                Engine {
                    target: target.clone(),
                    docker: None,
                    info: None,
                    containers: Vec::new(),
                    selected: None,
                    detail: None,
                    stats: None,
                    loading: false,
                    error: None,
                    busy: None,
                    tab: Tab::Overview,
                    resources: Resources::new(window, cx),
                    query,
                    logs_query,
                    logs_editor,
                    inspect_editor,
                    log: LogBuffer::default(),
                    logs_dirty: false,
                    paused: false,
                    auto_scroll: true,
                    logs_error: None,
                    logs_loading: false,
                },
            );
        }
        self.refresh(cx);
        cx.notify();
    }
    pub(crate) fn reconnect(&mut self, cx: &mut Context<Self>) {
        self.cancel_browsing();
        if let Some(engine) = self.engine_mut() {
            engine.docker = None;
            engine.info = None;
            engine.detail = None;
            engine.stats = None;
            engine.error = None;
        }
        self.refresh(cx);
    }
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        #[cfg(debug_assertions)]
        if self.fixture {
            return;
        }
        if !self.visible {
            return;
        }
        if self.engine().is_some_and(|engine| engine.loading) {
            self.refresh_pending = true;
            return;
        }
        let Some(target) = self.target().cloned() else {
            return;
        };
        self.query_epoch += 1;
        let epoch = self.query_epoch;
        let sender = self.sender.clone();
        let docker = self.engine().and_then(|engine| engine.docker.clone());
        if let Some(engine) = self.engine_mut() {
            engine.loading = true;
            engine.error = None;
        }
        self.query_worker = Some(worker(async move {
            if let Some(docker) = docker {
                let result = docker
                    .list_containers(Some(list_options()))
                    .await
                    .map_err(failure);
                let _ = sender.send(Message::Listed(target, epoch, result)).await;
            } else {
                let result = async {
                    let docker = target.connect().await?;
                    let info = docker.info().await?;
                    let containers = docker.list_containers(Some(list_options())).await?;
                    Ok::<_, anyhow::Error>((docker, info, containers))
                }
                .await
                .map_err(failure);
                let _ = sender.send(Message::Connected(target, epoch, result)).await;
            }
        }));
        cx.notify();
    }
    pub fn set_resource_page(
        &mut self,
        page: ResourcePage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.engine().is_none() {
            return;
        }
        if self
            .engine()
            .is_some_and(|engine| engine.resources.page == page)
        {
            if let Some(collection) = crate::browser::Collection::for_page(page) {
                self.show_collection(collection, window, cx);
            }
            return;
        }
        self.cancel_browsing();
        if let Some(engine) = self.engine_mut() {
            engine.resources.page = page;
            engine.resources.collection = crate::browser::Collection::for_page(page);
            engine.resources.inspector = false;
            engine.error = None;
        }
        self.load_resource_preview(window, cx);
        self.refresh(cx);
        self.start_events();
        cx.notify();
    }
    pub(crate) fn select_resource(
        &mut self,
        resource: Resource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Resource::Container(id) = resource {
            self.select_container(id, window, cx);
            return;
        }
        let page = resource.page();
        let changed_page = self
            .engine()
            .is_some_and(|engine| engine.resources.page != page);
        self.preview_epoch += 1;
        self.preview_worker = None;
        if changed_page {
            self.cancel_browsing();
        }
        if let Some(engine) = self.engine_mut() {
            engine.resources.output_visible = false;
            engine.resources.collection = None;
            engine.resources.page = page;
            engine.resources.selected.insert(page, resource);
            engine.resources.output_visible = false;
            engine.error = None;
        }
        self.load_resource_preview(window, cx);
        if changed_page {
            self.refresh(cx);
        }
        cx.notify();
    }
    fn load_resources(&mut self, _cx: &mut Context<Self>) {
        let Some(engine) = self.engine() else { return };
        let Some(docker) = engine.docker.clone() else {
            return;
        };
        let page = engine.resources.page;
        if page == ResourcePage::Containers {
            return;
        }
        let target = engine.target.clone();
        let engine_id = engine
            .info
            .as_ref()
            .and_then(|info| info.id.clone())
            .unwrap_or_default();
        self.resource_epoch += 1;
        let epoch = self.resource_epoch;
        let sender = self.sender.clone();
        if page == ResourcePage::Compose {
            self.project_worker = Some(worker(async move {
                let original = target.clone();
                let result = tokio::task::spawn_blocking(move || {
                    crate::compose::discover(&target, &engine_id)
                })
                .await;
                let result = result
                    .map_err(failure)
                    .and_then(|result| result.map_err(failure));
                let _ = sender
                    .send(Message::Projects(original, epoch, result))
                    .await;
            }));
        } else {
            self.resource_worker = Some(worker(async move {
                let result = crate::resources::list(&docker, page).await.map_err(failure);
                let _ = sender
                    .send(Message::Resources(target, page, epoch, result))
                    .await;
            }));
        }
    }
    fn load_resource_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(engine) = self.engine() else { return };
        let Some(resource) = engine.resources.selection().cloned() else {
            return;
        };
        if let Some(value) = engine.resources.preview.get(&resource.inspect_key()) {
            let text = serde_json::to_string_pretty(value).unwrap_or_default();
            engine
                .resources
                .editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
        } else {
            engine
                .resources
                .editor
                .update(cx, |editor, cx| editor.set_value("", window, cx));
        }
        if matches!(resource, Resource::Project(_)) && !engine.resources.raw() {
            return;
        }
        if resource == Resource::Events {
            let text = serde_json::to_string_pretty(&engine.resources.events).unwrap_or_default();
            engine
                .resources
                .editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
            return;
        }
        if engine.resources.document(&resource).is_some() {
            return;
        }
        #[cfg(debug_assertions)]
        if self.fixture {
            return;
        }
        let target = engine.target.clone();
        let docker = engine.docker.clone();
        let info = engine.info.clone();
        let project = match &resource {
            Resource::Project(name) => engine
                .resources
                .projects
                .iter()
                .find(|project| &project.name == name)
                .cloned(),
            _ => None,
        };
        let source = engine.resources.daemon.clone();
        let active_file = engine.resources.active_files.get(&resource).cloned();
        self.preview_epoch += 1;
        let epoch = self.preview_epoch;
        let sender = self.sender.clone();
        if matches!(resource, Resource::Project(_) | Resource::Daemon) {
            self.preview_worker = Some(worker(async move {
                let original = target.clone();
                let key = resource.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let (path, source) = match resource {
                        Resource::Daemon => {
                            let info = info.ok_or_else(|| {
                                anyhow::anyhow!(t!("docker.loading").into_owned())
                            })?;
                            let source = match source {
                                Some(source) => source,
                                None => crate::daemon::discover(&target, &info)?,
                            };
                            (source.path.clone(), Some(source))
                        }
                        _ => {
                            let path = active_file
                                .or_else(|| {
                                    project.and_then(|project| project.files.first().cloned())
                                })
                                .ok_or_else(|| {
                                    anyhow::anyhow!(
                                        t!("docker.compose_source_missing").into_owned()
                                    )
                                })?;
                            (path, None)
                        }
                    };
                    Ok::<_, anyhow::Error>((crate::documents::read(&target, path)?, source))
                })
                .await;
                let result = result
                    .map_err(failure)
                    .and_then(|result| result.map_err(failure));
                let _ = sender
                    .send(Message::Document(original, key, epoch, result))
                    .await;
            }));
        } else if let Some(docker) = docker {
            self.preview_worker = Some(worker(async move {
                let result = crate::resources::inspect(&docker, &resource)
                    .await
                    .map_err(failure);
                let _ = sender
                    .send(Message::Preview(target, resource, epoch, result))
                    .await;
            }));
        }
    }
    fn start_events(&mut self) {
        if !self.visible || self.events_worker.is_some() {
            return;
        }
        let Some(engine) = self.engine() else {
            return;
        };
        let Some(docker) = engine.docker.clone() else {
            return;
        };
        let target = engine.target.clone();
        let sender = self.sender.clone();
        self.events_worker = Some(worker(async move {
            let options = EventsOptionsBuilder::default().build();
            let mut delay = 1;
            loop {
                let mut events = docker.events(Some(options.clone()));
                while let Some(event) = events.next().await {
                    match event {
                        Ok(event) => {
                            if let Ok(value) = serde_json::to_value(event) {
                                let _ =
                                    sender.try_send(Message::EngineEvent(target.clone(), value));
                            }
                            let _ = sender.try_send(Message::Refresh(target.clone()));
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            delay = 1;
                        }
                        Err(error) => {
                            let _ = sender
                                .send(Message::EventsError(target.clone(), failure(error)))
                                .await;
                            break;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(15);
                if sender.send(Message::Refresh(target.clone())).await.is_err() {
                    break;
                }
            }
        }));
    }
    pub(crate) fn select_container(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(engine) = self.engine_mut() {
            engine.resources.page = ResourcePage::Containers;
            engine.resources.collection = None;
            engine.resources.output_visible = false;
        }
        self.detail_epoch += 1;
        self.logs_epoch += 1;
        self.detail_worker = None;
        self.logs_worker = None;
        if let Some(engine) = self.engine_mut() {
            if engine.selected.as_ref() != Some(&id) {
                engine.detail = None;
                engine.log.clear();
                engine.logs_dirty = true;
                engine
                    .inspect_editor
                    .update(cx, |editor, cx| editor.set_value("", window, cx));
                engine.stats = None;
            }
            engine.selected = Some(id);
            engine.logs_error = None;
        }
        self.load_detail(cx);
        if self.engine().is_some_and(|engine| engine.tab == Tab::Logs) {
            self.load_logs(cx);
        }
        cx.notify();
    }
    fn load_detail(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine() else {
            return;
        };
        let (Some(docker), Some(id)) = (engine.docker.clone(), engine.selected.clone()) else {
            return;
        };
        let target = engine.target.clone();
        self.detail_epoch += 1;
        let epoch = self.detail_epoch;
        let sender = self.sender.clone();
        self.detail_worker = Some(worker(async move {
            let result = async {
                let detail = docker.inspect_container(&id, None).await?;
                let stats = if detail
                    .state
                    .as_ref()
                    .is_some_and(|state| state.running == Some(true))
                {
                    let mut stream = docker.stats(
                        &id,
                        Some(
                            bollard::query_parameters::StatsOptionsBuilder::default()
                                .stream(false)
                                .one_shot(true)
                                .build(),
                        ),
                    );
                    tokio::time::timeout(Duration::from_secs(3), stream.next())
                        .await
                        .ok()
                        .flatten()
                        .and_then(Result::ok)
                        .and_then(|value| serde_json::to_value(value).ok())
                } else {
                    None
                };
                Ok::<_, anyhow::Error>((detail, stats))
            }
            .await
            .map_err(failure);
            let _ = sender
                .send(Message::Detail(target, id, epoch, result))
                .await;
        }));
        cx.notify();
    }
    pub(crate) fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine_mut() {
            engine.tab = tab;
            engine.resources.output_visible = false;
        }
        if tab == Tab::Logs {
            if self.logs_worker.is_none() {
                self.load_logs(cx);
            }
        } else {
            self.logs_epoch += 1;
            self.logs_worker = None;
        }
        cx.notify();
    }
    pub(crate) fn logs_worker_reset(&mut self, cx: &mut Context<Self>) {
        self.logs_epoch += 1;
        self.logs_worker = None;
        self.load_logs(cx);
    }
    pub(crate) fn pause_logs(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine_mut() else {
            return;
        };
        engine.paused = !engine.paused;
        engine.logs_loading = false;
        self.logs_epoch += 1;
        self.logs_worker = None;
        if self.engine().is_some_and(|engine| !engine.paused) {
            self.load_logs(cx);
        }
        cx.notify();
    }
    pub(crate) fn load_logs(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine() else {
            return;
        };
        if !self.visible || engine.paused || engine.resources.page != ResourcePage::Containers {
            return;
        }
        let (Some(docker), Some(id)) = (engine.docker.clone(), engine.selected.clone()) else {
            return;
        };
        let target = engine.target.clone();
        self.logs_epoch += 1;
        let epoch = self.logs_epoch;
        let sender = self.sender.clone();
        if let Some(engine) = self.engine_mut() {
            engine.log.clear();
            engine.logs_dirty = true;
            engine.logs_error = None;
            engine.logs_loading = true;
        }
        self.logs_worker = Some(worker(async move {
            let mut stream = docker.logs(
                &id,
                Some(
                    LogsOptionsBuilder::default()
                        .stdout(true)
                        .stderr(true)
                        .follow(true)
                        .tail("1000")
                        .build(),
                ),
            );
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        if sender
                            .send(Message::Log(
                                target.clone(),
                                id.clone(),
                                epoch,
                                bytes.into_bytes().to_vec(),
                            ))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = sender
                            .send(Message::LogEnd(target, id, epoch, Some(failure(error))))
                            .await;
                        return;
                    }
                }
            }
            let _ = sender.send(Message::LogEnd(target, id, epoch, None)).await;
        }));
        cx.notify();
    }
    fn apply_message(&mut self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
        match message {
            Message::Connected(target, epoch, result) => {
                if !self.matches(&target) || epoch != self.query_epoch {
                    return;
                }
                self.query_worker = None;
                if let Some(engine) = self.engine_mut() {
                    engine.loading = false;
                    match result {
                        Ok((docker, info, containers)) => {
                            engine.docker = Some(docker);
                            engine.info = Some(info);
                            engine.containers = containers;
                            engine.error = None;
                        }
                        Err(error) => engine.error = Some(error),
                    }
                }
                self.after_listing(cx);
                self.start_events();
                self.load_resources(cx);
            }
            Message::Listed(target, epoch, result) => {
                if !self.matches(&target) || epoch != self.query_epoch {
                    return;
                }
                self.query_worker = None;
                if let Some(engine) = self.engine_mut() {
                    engine.loading = false;
                    match result {
                        Ok(containers) => {
                            engine.containers = containers;
                            engine.error = None;
                        }
                        Err(error) => engine.error = Some(error),
                    }
                }
                self.after_listing(cx);
                self.load_resources(cx);
            }
            Message::Detail(target, id, epoch, result) => {
                if !self.matches(&target)
                    || epoch != self.detail_epoch
                    || !self
                        .engine()
                        .is_some_and(|engine| engine.selected.as_ref() == Some(&id))
                {
                    return;
                }
                self.detail_worker = None;
                if let Some(engine) = self.engine_mut() {
                    match result {
                        Ok((detail, stats)) => {
                            let text = serde_json::to_string_pretty(&detail).unwrap_or_default();
                            engine
                                .inspect_editor
                                .update(cx, |editor, cx| editor.set_value(text, window, cx));
                            engine.detail = Some(detail);
                            engine.stats = stats;
                        }
                        Err(error) => engine.error = Some(error),
                    }
                }
                if let Some((target, resource, action)) = self.pending_form.take() {
                    if resource == Resource::Container(id)
                        && self.engine().is_some_and(|engine| engine.detail.is_some())
                    {
                        self.open_action(target, Some(resource), action, window, cx);
                    }
                }
            }
            Message::Log(target, id, epoch, bytes) => {
                if !self.matches(&target)
                    || epoch != self.logs_epoch
                    || !self
                        .engine()
                        .is_some_and(|engine| engine.selected.as_ref() == Some(&id))
                {
                    return;
                }
                if let Some(engine) = self.engine_mut() {
                    engine.log.push(&bytes);
                    engine.logs_dirty = true;
                    engine.logs_loading = false;
                }
            }
            Message::LogEnd(target, id, epoch, error) => {
                if !self.matches(&target)
                    || epoch != self.logs_epoch
                    || !self
                        .engine()
                        .is_some_and(|engine| engine.selected.as_ref() == Some(&id))
                {
                    return;
                }
                self.logs_worker = None;
                if let Some(engine) = self.engine_mut() {
                    engine.logs_error = error;
                    engine.logs_loading = false;
                }
            }
            Message::Refresh(target) => {
                if self.matches(&target) && self.visible {
                    self.refresh(cx);
                }
            }
            Message::EventsError(target, error) => {
                if self.matches(&target) {
                    if let Some(engine) = self.engine_mut() {
                        engine.error = Some(error);
                    }
                }
            }
            Message::Resources(target, page, epoch, result) => {
                if !self.matches(&target)
                    || epoch != self.resource_epoch
                    || self
                        .engine()
                        .is_none_or(|engine| engine.resources.page != page)
                {
                    return;
                }
                self.resource_worker = None;
                if let Some(engine) = self.engine_mut() {
                    match result {
                        Ok(snapshot) => engine.resources.apply(snapshot),
                        Err(error) => engine.error = Some(error),
                    }
                }
                self.load_resource_preview(window, cx);
            }
            Message::Preview(target, resource, epoch, result) => {
                if !self.matches(&target)
                    || epoch != self.preview_epoch
                    || self
                        .engine()
                        .and_then(|engine| engine.resources.selection())
                        != Some(&resource)
                {
                    return;
                }
                self.preview_worker = None;
                if let Some(engine) = self.engine_mut() {
                    match result {
                        Ok(value) => {
                            engine.resources.editor.update(cx, |editor, cx| {
                                editor.set_value(
                                    serde_json::to_string_pretty(&value).unwrap_or_default(),
                                    window,
                                    cx,
                                )
                            });
                            engine
                                .resources
                                .preview
                                .insert(resource.inspect_key(), value);
                        }
                        Err(error) => engine.error = Some(error),
                    }
                }
            }
            Message::Document(target, resource, epoch, result) => {
                if !self.matches(&target)
                    || epoch != self.preview_epoch
                    || self
                        .engine()
                        .and_then(|engine| engine.resources.selection())
                        != Some(&resource)
                {
                    return;
                }
                self.preview_worker = None;
                if let Some(engine) = self.engine_mut() {
                    match result {
                        Ok((file, source)) => {
                            if let Some(source) = source {
                                engine.resources.daemon = Some(source);
                            }
                            engine
                                .resources
                                .active_files
                                .insert(resource.clone(), file.path.clone());
                            engine
                                .resources
                                .documents
                                .entry(file.path.clone())
                                .or_insert_with(|| {
                                    crate::documents::Document::new(file, &resource, window, cx)
                                });
                        }
                        Err(error) => engine.error = Some(error),
                    }
                }
            }
            Message::Projects(target, epoch, result) => {
                if !self.matches(&target) || epoch != self.resource_epoch {
                    return;
                }
                self.project_worker = None;
                if let Some(engine) = self.engine_mut() {
                    if let Ok(projects) = result {
                        for project in projects {
                            if !engine
                                .resources
                                .projects
                                .iter()
                                .any(|entry| entry.name == project.name)
                            {
                                engine.resources.projects.push(project);
                            }
                        }
                        engine
                            .resources
                            .projects
                            .sort_by(|a, b| a.name.cmp(&b.name));
                    }
                }
            }
            Message::EngineEvent(target, value) => {
                if !self.matches(&target) {
                    return;
                }
                if let Some(engine) = self.engine_mut() {
                    engine.resources.events.push(value);
                    if engine.resources.events.len() > 500 {
                        engine.resources.events.remove(0);
                    }
                    if engine.resources.selection() == Some(&Resource::Events) {
                        let text = serde_json::to_string_pretty(&engine.resources.events)
                            .unwrap_or_default();
                        engine
                            .resources
                            .editor
                            .update(cx, |editor, cx| editor.set_value(text, window, cx));
                    }
                }
            }
            Message::Progress(target, text) => {
                if let Some(engine) = self
                    .engines
                    .get_mut(&target.id)
                    .filter(|engine| engine.target.endpoint == target.endpoint)
                {
                    append_output(engine, &text, window, cx);
                }
            }
            Message::Managed(target, resource, action, submitted, result) => {
                self.operations.remove(&target.id);
                let Some(engine) = self
                    .engines
                    .get_mut(&target.id)
                    .filter(|engine| engine.target.endpoint == target.endpoint)
                else {
                    return;
                };
                engine.busy = None;
                match result {
                    Ok(outcome) => {
                        if matches!(
                            action,
                            crate::actions::Action::Save
                                | crate::actions::Action::Validate
                                | crate::actions::Action::Restore
                        ) {
                            window.push_notification(
                                Notification::success(format!(
                                    "{} · {}",
                                    target.name, outcome.text
                                )),
                                cx,
                            );
                        }
                        append_output(engine, &outcome.text, window, cx);
                        if let Some(source) = outcome.source {
                            engine.resources.daemon = Some(source);
                        }
                        let key = outcome
                            .project
                            .as_ref()
                            .map(|project| Resource::Project(project.name.clone()))
                            .or(resource.clone());
                        if let Some(project) = outcome.project {
                            engine
                                .resources
                                .imported
                                .retain(|old| old.name != project.name);
                            engine.resources.imported.push(project);
                            engine.resources.update_projects(&engine.containers);
                        }
                        if let (Some(key), Some(file)) = (key.clone(), outcome.file) {
                            engine
                                .resources
                                .active_files
                                .insert(key.clone(), file.path.clone());
                            if let Some(document) = engine.resources.documents.get_mut(&file.path) {
                                if !matches!(action, crate::actions::Action::Source)
                                    || !document.dirty(cx)
                                {
                                    if matches!(
                                        action,
                                        crate::actions::Action::Source
                                            | crate::actions::Action::Restore
                                    ) && document.editor.read(cx).value().as_ref() == submitted
                                    {
                                        document.editor.update(cx, |editor, cx| {
                                            editor.set_value(
                                                file.original.clone().unwrap_or_default(),
                                                window,
                                                cx,
                                            )
                                        });
                                    }
                                    document.file = file;
                                }
                            } else {
                                let path = file.path.clone();
                                let document =
                                    crate::documents::Document::new(file, &key, window, cx);
                                engine.resources.documents.insert(path, document);
                            }
                        }
                        engine.error = None;
                        if self.matches(&target) {
                            if matches!(action, crate::actions::Action::ImportProject) {
                                if let Some(key) = key {
                                    self.select_resource(key, window, cx);
                                }
                            }
                            if matches!(
                                action,
                                crate::actions::Action::Reload | crate::actions::Action::Restart
                            ) {
                                self.reconnect(cx);
                            } else if action.changes_engine() {
                                self.refresh(cx);
                            }
                        }
                    }
                    Err(error) => {
                        append_output(engine, &i18n::format_failure(&error), window, cx);
                        engine.error = Some(error);
                        if self.matches(&target) && action.changes_engine() {
                            self.refresh(cx);
                        }
                    }
                }
            }
        }
    }
    fn matches(&self, target: &Target) -> bool {
        self.target()
            .is_some_and(|current| current.id == target.id && current.endpoint == target.endpoint)
    }
    fn after_listing(&mut self, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine_mut() {
            engine.resources.update_projects(&engine.containers);
        }
        let missing = self.engine().is_some_and(|engine| {
            engine.selected.as_ref().is_some_and(|id| {
                !engine
                    .containers
                    .iter()
                    .any(|container| container.id.as_ref() == Some(id))
            })
        });
        if missing {
            self.detail_epoch += 1;
            self.logs_epoch += 1;
            self.detail_worker = None;
            self.logs_worker = None;
            if let Some(engine) = self.engine_mut() {
                engine.selected = None;
                engine.detail = None;
                engine.log.clear();
                engine.logs_dirty = true;
            }
        } else if self.engine().is_some_and(|engine| {
            engine.selected.is_some() && engine.resources.page == ResourcePage::Containers
        }) {
            self.load_detail(cx);
            if self
                .engine()
                .is_some_and(|engine| engine.tab == Tab::Logs && !engine.paused)
                && self.logs_worker.is_none()
            {
                self.load_logs(cx);
            }
        }
        if self.refresh_pending {
            self.refresh_pending = false;
            self.refresh(cx);
        }
    }
    fn sync_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let palette = self.terminal_palette;
        if let Some(engine) = self.engine_mut() {
            engine.logs_editor.update(cx, |editor, _| {
                editor.set_editor_style(tshell_ui::editor::style(palette))
            });
            engine.inspect_editor.update(cx, |editor, _| {
                editor.set_editor_style(tshell_ui::editor::style(palette))
            });
            for editor in [&engine.resources.editor, &engine.resources.output] {
                editor.update(cx, |editor, _| {
                    editor.set_editor_style(tshell_ui::editor::style(palette))
                });
            }
            if let Some(document) = engine
                .resources
                .selection()
                .and_then(|resource| engine.resources.document(resource))
            {
                document.editor.update(cx, |editor, _| {
                    editor.set_editor_style(tshell_ui::editor::style(palette))
                });
            }
            if engine.logs_dirty {
                engine.logs_dirty = false;
                let text = engine.log.text();
                let last = text.lines().count().saturating_sub(1) as u32;
                let follow = engine.auto_scroll;
                engine.logs_editor.update(cx, |editor, cx| {
                    let cursor = editor.cursor_position();
                    editor.set_value(text, window, cx);
                    editor.set_cursor_position(
                        if follow {
                            gpui_kit::component::input::Position::new(last, u32::MAX)
                        } else {
                            cursor
                        },
                        window,
                        cx,
                    );
                });
            }
        }
    }
    pub(crate) fn open_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(engine) = self.engine() else {
            return;
        };
        let (Some(docker), Some(container)) = (engine.docker.clone(), engine.selected.clone())
        else {
            return;
        };
        let target = engine.target.clone();
        let name = engine
            .containers
            .iter()
            .find(|entry| entry.id.as_ref() == Some(&container))
            .map(container_name)
            .unwrap_or_else(|| container.clone());
        let windows = engine
            .info
            .as_ref()
            .and_then(|info| info.os_type.as_deref())
            == Some("windows");
        let input = cx.new(|cx| {
            InputState::new(window, cx).default_value(if windows { "cmd.exe" } else { "/bin/sh" })
        });
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (target, docker, container, name, input, owner) = (
                target.clone(),
                docker.clone(),
                container.clone(),
                name.clone(),
                input.clone(),
                owner.clone(),
            );
            tshell_ui::controls::workspace_dialog(dialog, cx)
                .title(t!("docker.terminal").into_owned())
                .child(gpui_kit::component::input::Input::new(&input))
                .on_ok(move |_, window, cx| {
                    let command = shell_words::split(&input.read(cx).value());
                    match command {
                        Ok(command) if !command.is_empty() => {
                            let _ = owner.update(cx, |_, cx| {
                                cx.emit(Event::OpenTerminal {
                                    target: target.clone(),
                                    docker: docker.clone(),
                                    container: container.clone(),
                                    name: name.clone(),
                                    command,
                                })
                            });
                            true
                        }
                        _ => {
                            window.push_notification(
                                Notification::warning(t!("docker.invalid_command")),
                                cx,
                            );
                            false
                        }
                    }
                })
        });
    }
    pub(crate) fn configure_endpoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .targets
            .iter()
            .find(|target| target.id == HostId::local())
            .and_then(|target| match &target.endpoint {
                Endpoint::Local { path } => path.clone(),
                _ => None,
            })
            .unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(value)
                .placeholder(if cfg!(windows) {
                    "//./pipe/docker_engine"
                } else {
                    "/var/run/docker.sock"
                })
        });
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (input, owner) = (input.clone(), owner.clone());
            tshell_ui::controls::workspace_dialog(dialog, cx)
                .title(t!("docker.endpoint").into_owned())
                .child(gpui_kit::component::input::Input::new(&input))
                .on_ok(move |_, window, cx| {
                    let value = input.read(cx).value().trim().to_owned();
                    if value.chars().any(char::is_control)
                        || (!value.is_empty()
                            && if cfg!(windows) {
                                !value.starts_with("//./pipe/") && !value.starts_with(r"\\.\pipe\")
                            } else {
                                !value.starts_with('/')
                            })
                    {
                        window.push_notification(
                            Notification::warning(t!("docker.invalid_endpoint")),
                            cx,
                        );
                        return false;
                    }
                    let _ = owner.update(cx, |_, cx| {
                        cx.emit(Event::LocalEndpointChanged(
                            (!value.is_empty()).then_some(value),
                        ))
                    });
                    true
                })
        });
    }
}
fn append_output(
    engine: &mut Engine,
    text: &str,
    window: &mut Window,
    cx: &mut Context<DockerView>,
) {
    engine.resources.output_text.push_str(text);
    engine.resources.output_text.push('\n');
    let text = &mut engine.resources.output_text;
    if text.len() > 256 * 1024 {
        let mut start = text.len() - 256 * 1024;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text.drain(..start);
    }
    engine
        .resources
        .output
        .update(cx, |editor, cx| editor.set_value(text.clone(), window, cx));
}
impl DockerView {
    pub fn has_unsaved_files(&self, host: &HostId, cx: &App) -> bool {
        self.engines.get(host).is_some_and(|engine| {
            engine
                .resources
                .documents
                .values()
                .any(|document| document.dirty(cx))
        })
    }
    pub fn busy(&self, host: &HostId, cx: &App) -> bool {
        self.operations.contains_key(host) || self.has_unsaved_files(host, cx)
    }
    pub(crate) fn action_button(
        &self,
        actions: Vec<crate::actions::Action>,
        resource: Option<Resource>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_kit::assets::IconName;
        use gpui_kit::component::{Disableable, menu::DropdownMenu};
        let owner = cx.entity().downgrade();
        let target = self.target().cloned();
        if actions.is_empty() {
            return div().into_any_element();
        }
        tshell_ui::controls::icon_button(
            if resource.is_none() {
                "docker-create-resource"
            } else {
                "docker-resource-actions"
            },
            if resource.is_none() {
                IconName::Plus
            } else {
                IconName::Ellipsis
            },
            t!(if resource.is_none() {
                "docker.create"
            } else {
                "file.actions"
            }),
        )
        .disabled(self.engine().is_none_or(|engine| {
            (engine.docker.is_none() && actions.iter().all(|action| action.needs_engine()))
                || engine.busy.is_some()
        }))
        .dropdown_menu(move |menu, _, _| {
            action_menu(
                menu,
                actions.clone(),
                target.clone(),
                resource.clone(),
                owner.clone(),
            )
        })
        .into_any_element()
    }
    pub(crate) fn open_action(
        &mut self,
        target: Target,
        resource: Option<Resource>,
        action: crate::actions::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.matches(&target) {
            return;
        }
        let Some(engine) = self.engine() else {
            return;
        };
        if engine.docker.is_none() && action.needs_engine() {
            window.push_notification(Notification::warning(t!("docker.loading_details")), cx);
            return;
        }
        let docker = engine.docker.clone();
        if engine.busy.is_some() {
            return;
        }
        if matches!(action, crate::actions::Action::Recreate) {
            if let Some(Resource::Container(id)) = &resource {
                if engine.detail.as_ref().and_then(|detail| detail.id.as_ref()) != Some(id) {
                    let id = id.clone();
                    self.select_container(id.clone(), window, cx);
                    self.pending_form = Some((target, Resource::Container(id), action));
                    return;
                }
            }
        }
        let document = resource
            .as_ref()
            .and_then(|key| engine.resources.document(key));
        let dirty = document.is_some_and(|document| document.dirty(cx));
        if matches!(action, crate::actions::Action::ImportProject) && dirty {
            window.push_notification(Notification::warning(t!("docker.save_first")), cx);
            return;
        }
        let request = crate::actions::Request {
            target: target.clone(),
            docker,
            engine_id: engine
                .info
                .as_ref()
                .and_then(|info| info.id.clone())
                .unwrap_or_default(),
            resource: resource.clone(),
            container: match &resource {
                Some(Resource::Container(id)) => Some(id.clone()),
                _ => engine.selected.clone(),
            },
            project: match &resource {
                Some(Resource::Project(name)) => engine
                    .resources
                    .projects
                    .iter()
                    .find(|project| &project.name == name)
                    .cloned(),
                _ => None,
            },
            source: engine.resources.daemon.clone(),
            file: document.map(|document| document.file.clone()),
            service: match &resource {
                Some(Resource::Project(name)) => engine.resources.services.get(name).cloned(),
                _ => None,
            },
            text: document
                .map(|document| document.editor.read(cx).value().to_string())
                .unwrap_or_default(),
            values: Vec::new(),
            action,
        };
        let recreate = engine.detail.as_ref().map(|detail| {
            let mut value = serde_json::to_value(&detail.config).unwrap_or_default();
            value["HostConfig"] = serde_json::to_value(&detail.host_config).unwrap_or_default();
            if let Some(networks) = detail
                .network_settings
                .as_ref()
                .and_then(|settings| settings.networks.as_ref())
            {
                let endpoints: serde_json::Map<String, serde_json::Value> = networks
                    .iter()
                    .map(|(name, settings)| {
                        let original = serde_json::to_value(settings).unwrap_or_default();
                        let fields: serde_json::Map<String, serde_json::Value> =
                            ["IPAMConfig", "Links", "Aliases", "DriverOpts"]
                                .into_iter()
                                .filter_map(|key| {
                                    original.get(key).cloned().map(|value| (key.into(), value))
                                })
                                .collect();
                        (name.clone(), serde_json::Value::Object(fields))
                    })
                    .collect();
                value["NetworkingConfig"] = serde_json::json!({"EndpointsConfig":endpoints});
            }
            serde_json::to_string_pretty(&value).unwrap_or_default()
        });
        let image_config = match &resource {
            Some(Resource::Image(image)) => Some(serde_json::json!({"Image":image.id}).to_string()),
            _ => None,
        };
        if !action.fields().is_empty() {
            let fields: Vec<_> = action
                .fields()
                .iter()
                .map(|(label, initial)| {
                    let initial = if matches!(action, crate::actions::Action::Source) {
                        request
                            .file
                            .as_ref()
                            .map(|file| file.path.as_str())
                            .unwrap_or(initial)
                    } else if matches!(action, crate::actions::Action::Push)
                        && *label == "docker.image"
                    {
                        match &resource {
                            Some(Resource::Image(image)) => image.tag.as_deref().unwrap_or(initial),
                            _ => initial,
                        }
                    } else if matches!(action, crate::actions::Action::Recreate) {
                        recreate.as_deref().unwrap_or(initial)
                    } else if matches!(action, crate::actions::Action::CreateContainer) {
                        if *label == "docker.json" {
                            image_config.as_deref().unwrap_or(initial)
                        } else {
                            initial
                        }
                    } else {
                        initial
                    };
                    let input = if *label == "docker.json" {
                        FormField::Json(cx.new(|cx| {
                            let mut editor = EditorState::new(window, cx)
                                .language("json")
                                .default_value(initial.to_owned());
                            editor.set_highlighter_factory(tshell_ui::editor::factory(), cx);
                            editor
                        }))
                    } else {
                        FormField::Input(cx.new(|cx| {
                            InputState::new(window, cx).default_value(initial.to_owned())
                        }))
                    };
                    (*label, input)
                })
                .collect();
            let request = std::sync::Arc::new(parking_lot::Mutex::new(Some(request)));
            let owner = cx.entity().downgrade();
            window.open_dialog(cx, move |dialog, window, cx| {
                use gpui_kit::component::input::Input;
                let (request, owner, fields) = (request.clone(), owner.clone(), fields.clone());
                let content = div()
                    .id("docker-action-fields")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .max_h(px(
                        (f32::from(window.viewport_size().height) - 180.).max(64.)
                    ))
                    .overflow_y_scrollbar()
                    .when(matches!(action, crate::actions::Action::Recreate), |view| {
                        view.child(
                            div()
                                .text_size(px(12.))
                                .child(t!("docker.recreate_hint").into_owned()),
                        )
                    })
                    .children(fields.iter().map(|(label, input)| {
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_size(px(12.))
                            .child(t!(*label).into_owned())
                            .child(match input {
                                FormField::Input(input) => Input::new(input).into_any_element(),
                                FormField::Json(editor) => div()
                                    .h(px((f32::from(window.viewport_size().height) - 340.)
                                        .clamp(100., 420.)))
                                    .font_family("monospace")
                                    .child(gpui_kit::base::input::Editor::new(editor))
                                    .into_any_element(),
                            })
                    }));
                tshell_ui::controls::workspace_dialog(dialog, cx)
                    .title(t!(action.key()).into_owned())
                    .width(px(
                        640_f32.min((f32::from(window.viewport_size().width) - 32.).max(0.))
                    ))
                    .child(content)
                    .on_ok(move |_, window, cx| {
                        let values: Vec<_> = fields
                            .iter()
                            .map(|(_, input)| match input {
                                FormField::Input(input) => {
                                    input.read(cx).value().trim().to_string()
                                }
                                FormField::Json(editor) => editor.read(cx).value().to_string(),
                            })
                            .collect();
                        if dirty
                            && matches!(action, crate::actions::Action::Source)
                            && request.lock().as_ref().is_some_and(|request| {
                                values.first().map(String::as_str)
                                    != request.file.as_ref().map(|file| file.path.as_str())
                            })
                        {
                            window.push_notification(
                                Notification::warning(t!("docker.save_first")),
                                cx,
                            );
                            return false;
                        }
                        if values.iter().any(|value| value.is_empty()) {
                            window.push_notification(
                                Notification::warning(t!("docker.name_required")),
                                cx,
                            );
                            return false;
                        }
                        if fields.iter().zip(&values).any(|((_, field), value)| {
                            matches!(field, FormField::Json(_))
                                && serde_json::from_str::<serde_json::Value>(value).is_err()
                        }) {
                            window.push_notification(
                                Notification::warning(t!("docker.invalid_json")),
                                cx,
                            );
                            return false;
                        }
                        if let Some(mut request) = request.lock().take() {
                            request.values = values;
                            let _ = owner
                                .update(cx, |this, cx| this.begin_managed(request, window, cx));
                        }
                        true
                    })
            });
        } else if action.destructive() {
            let detail = if matches!(
                action,
                crate::actions::Action::Reload | crate::actions::Action::Restart
            ) {
                t!("docker.confirm_engine", engine = target.name.clone()).into_owned()
            } else {
                format!(
                    "{} · {}",
                    target.name,
                    resource
                        .as_ref()
                        .map(|key| engine.resources.name(key))
                        .unwrap_or_else(|| t!("docker.title").into_owned())
                )
            };
            let title = t!(action.key()).into_owned();
            let cancel = t!("explorer.cancel");
            let confirm = t!("explorer.confirm");
            let receiver = window.prompt(
                PromptLevel::Warning,
                &title,
                Some(&detail),
                &[cancel.as_ref(), confirm.as_ref()],
                cx,
            );
            cx.spawn_in(window, async move |view, cx| {
                if receiver.await == Ok(1) {
                    let _ = view.update_in(cx, |this, window, cx| {
                        this.begin_managed(request, window, cx)
                    });
                }
            })
            .detach();
        } else {
            self.begin_managed(request, window, cx);
        }
    }
    fn begin_managed(
        &mut self,
        request: crate::actions::Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.targets.iter().any(|target| {
            target.id == request.target.id && target.endpoint == request.target.endpoint
        }) {
            window.push_notification(Notification::warning(t!("docker.target_changed")), cx);
            return;
        }
        if self.operations.contains_key(&request.target.id) {
            return;
        }
        let target = request.target.clone();
        let key = request.resource.clone();
        let action = request.action;
        let submitted = request.text.clone();
        if let Some(engine) = self.engines.get_mut(&target.id) {
            if matches!(action, crate::actions::Action::ImportProject)
                && request.values.first().is_some_and(|name| {
                    engine
                        .resources
                        .projects
                        .iter()
                        .any(|project| &project.name == name)
                })
            {
                window.push_notification(Notification::warning(t!("docker.project_exists")), cx);
                return;
            }
            engine.busy = Some(t!(action.key()).into_owned());
            engine.error = None;
            engine.resources.output_resource = key.clone();
            engine.resources.output_text.clear();
            engine.resources.output_visible = !matches!(
                action,
                crate::actions::Action::Save
                    | crate::actions::Action::Validate
                    | crate::actions::Action::Restore
                    | crate::actions::Action::Source
            );
            append_output(
                engine,
                &format!("{} · {}", target.name, t!(action.key())),
                window,
                cx,
            );
        }
        let sender = self.sender.clone();
        self.operations.insert(
            target.id.clone(),
            worker(async move {
                let progress_sender = sender.clone();
                let progress_target = target.clone();
                let engine_id = request.engine_id.clone();
                let result = crate::actions::execute(request, move |text| {
                    let _ =
                        progress_sender.try_send(Message::Progress(progress_target.clone(), text));
                })
                .await;
                let result = if result.is_ok()
                    && matches!(
                        action,
                        crate::actions::Action::Reload | crate::actions::Action::Restart
                    ) {
                    let recovery = tokio::time::timeout(Duration::from_secs(120), async {
                        loop {
                            if let Ok(docker) = target.connect().await {
                                if let Ok(info) = docker.info().await {
                                    anyhow::ensure!(
                                        info.id.as_deref() == Some(engine_id.as_str()),
                                        t!("docker.cli_engine_mismatch").into_owned()
                                    );
                                    return Ok::<_, anyhow::Error>(());
                                }
                            }
                            tokio::time::sleep(Duration::from_millis(500)).await;
                        }
                    })
                    .await;
                    match recovery {
                        Ok(Ok(())) => result.map(|mut outcome| {
                            outcome.text.push('\n');
                            outcome.text.push_str(&t!("docker.engine_recovered"));
                            outcome
                        }),
                        Ok(Err(error)) => Err(error),
                        Err(_) => Err(anyhow::anyhow!(
                            t!("docker.engine_recovery_timeout").into_owned()
                        )),
                    }
                } else {
                    result
                };
                let _ = sender
                    .send(Message::Managed(
                        target,
                        key,
                        action,
                        submitted,
                        result.map_err(failure),
                    ))
                    .await;
            }),
        );
        cx.notify();
    }
}
pub(crate) fn container_name(container: &ContainerSummary) -> String {
    container
        .names
        .as_ref()
        .and_then(|names| names.first())
        .map(|name| name.trim_start_matches('/').to_owned())
        .or_else(|| {
            container
                .id
                .as_ref()
                .map(|id| id.chars().take(12).collect())
        })
        .unwrap_or_default()
}
pub(crate) fn is_running(container: &ContainerSummary) -> bool {
    container.state == Some(ContainerSummaryStateEnum::RUNNING)
}

#[cfg(debug_assertions)]
impl DockerView {
    pub fn check_action_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(engine) = self.engine_mut() else {
            return;
        };
        let target = engine.target.clone();
        let connection = engine
            .docker
            .replace(Docker::connect_with_local_defaults().unwrap());
        self.open_action(
            target,
            None,
            crate::actions::Action::CreateContainer,
            window,
            cx,
        );
        self.engine_mut().unwrap().docker = connection;
    }

    pub fn check_feedback_state(&mut self, state: usize, cx: &mut Context<Self>) {
        let engine = self.engine_mut().expect("Docker fixture Engine");
        engine.loading = state == 1;
        engine.error = (state == 2).then(|| failure(anyhow::anyhow!("Fixture Engine request failed. Existing content remains available while the request is retried.")));
        engine.logs_loading = state == 1;
        engine.logs_error = engine.error.clone();
        cx.notify();
    }

    pub fn check_feedback_bounds(&self, cx: &App) -> Vec<Bounds<Pixels>> {
        let engine = self.engine().expect("Docker fixture Engine");
        if self.collection_visible() {
            let collection = engine.resources.collection.unwrap();
            let query = if collection == crate::browser::Collection::Containers {
                &engine.query
            } else {
                &engine.resources.queries[&collection.page()]
            };
            return vec![
                query.read(cx).input_bounds(),
                engine.resources.table_scrolls[&collection]
                    .0
                    .borrow()
                    .base_handle
                    .bounds(),
            ];
        }
        if engine.resources.page == ResourcePage::Containers {
            return match engine.tab {
                Tab::Logs => vec![
                    engine.logs_query.read(cx).input_bounds(),
                    engine.logs_editor.read(cx).input_bounds(),
                ],
                Tab::Inspect => vec![engine.inspect_editor.read(cx).input_bounds()],
                Tab::Overview => Vec::new(),
            };
        }
        let resource = engine.resources.selection().unwrap();
        if let Some(document) = engine.resources.document(resource) {
            vec![document.editor.read(cx).input_bounds()]
        } else if engine.resources.raw() || matches!(resource, Resource::Events | Resource::Daemon)
        {
            vec![engine.resources.editor.read(cx).input_bounds()]
        } else {
            Vec::new()
        }
    }

    pub fn check_config_document_state(
        &mut self,
        state: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(engine) = self.engine_mut() else {
            return false;
        };
        if engine.resources.output_visible {
            return false;
        }
        let Some(resource) = engine.resources.selection().cloned() else {
            return false;
        };
        let path = match &resource {
            Resource::Project(name) if engine.resources.raw() => engine
                .resources
                .projects
                .iter()
                .find(|project| &project.name == name)
                .and_then(|project| project.files.first().cloned()),
            Resource::Daemon => engine
                .resources
                .daemon
                .as_ref()
                .map(|source| source.path.clone()),
            _ => return false,
        };
        let Some(path) = path else { return false };
        let Some(document) = engine.resources.documents.get(&path) else {
            return false;
        };
        if state == 0 {
            engine.resources.active_files.remove(&resource);
            engine.resources.preview.remove(&resource.inspect_key());
        } else {
            engine.resources.active_files.insert(resource, path);
            let mut text = document.file.original.clone().unwrap_or_default();
            if state == 2 {
                text.push('\n');
            }
            document
                .editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
        }
        engine.loading = state == 0;
        cx.notify();
        true
    }

    pub fn check_compact_sidebar(&mut self, compact: bool, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine_mut() {
            engine.resources.sidebar_group = (!compact).then_some(SidebarGroup::Containers);
        }
        cx.notify();
    }

    pub fn check_feedback_empty_collection(
        &mut self,
        empty: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.collection_visible() {
            return false;
        }
        let engine = self.engine_mut().unwrap();
        let collection = engine.resources.collection.unwrap();
        let query = if collection == crate::browser::Collection::Containers {
            &engine.query
        } else {
            &engine.resources.queries[&collection.page()]
        };
        query.update(cx, |query, cx| {
            query.set_value(
                if empty {
                    "fixture-no-matching-resource"
                } else {
                    ""
                },
                window,
                cx,
            )
        });
        true
    }

    pub fn check_layout(&self, cx: &App) {
        let engine = self.engine().expect("Docker fixture Engine");
        if engine.resources.page != ResourcePage::Containers {
            let resource = engine.resources.selection().unwrap();
            let editor = if engine.resources.output_visible {
                Some(&engine.resources.output)
            } else if let Some(document) = engine
                .resources
                .document(resource)
                .filter(|_| engine.resources.raw() || *resource == Resource::Daemon)
            {
                Some(&document.editor)
            } else if engine.resources.raw() || *resource == Resource::Events {
                Some(&engine.resources.editor)
            } else {
                None
            };
            if let Some(editor) = editor {
                assert!(
                    editor.read(cx).input_bounds().size.height > px(100.),
                    "Resource editor did not fill the main pane"
                );
            }
            return;
        }
        let editor = match engine.tab {
            Tab::Logs => &engine.logs_editor,
            Tab::Inspect => &engine.inspect_editor,
            Tab::Overview => return,
        };
        assert!(
            editor.read(cx).input_bounds().size.height > px(100.),
            "Docker editor did not fill the main pane"
        );
    }
    /// Native UI fixture; never opens a real Engine or an SSH connection.
    pub fn check_fixture(
        &mut self,
        host_id: HostId,
        tab: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fixture = true;
        self.cancel_browsing();
        self.visible = false;
        self.select_target(host_id, window, cx);
        self.visible = true;
        let engine = self.engine_mut().expect("fixture Engine");
        engine.info = Some(
            serde_json::from_value(
                serde_json::json!({"OperatingSystem":"Linux","Name":"fixture-engine"}),
            )
            .unwrap(),
        );
        engine.containers = (0..10_000)
            .map(|index| ContainerSummary {
                id: Some(format!("fixture-container-{index:064}")),
                names: Some(vec![format!("/service-{index}")]),
                image: Some("alpine:3".into()),
                state: Some(ContainerSummaryStateEnum::RUNNING),
                status: Some("Up 1 minute".into()),
                labels: Some(HashMap::from([(
                    "com.docker.compose.project".into(),
                    "fixture-stack".into(),
                )])),
                ..Default::default()
            })
            .collect();
        let id = engine.containers[tab].id.clone().unwrap();
        engine.selected = Some(id.clone());
        engine.detail = Some(serde_json::from_value(serde_json::json!({
            "Id":id,"Name":"/service-0","Created":"2026-10-09T00:00:00Z",
            "Config":{"Image":"alpine:3"},"State":{"Running":true,"Status":"running","StartedAt":"2026-10-09T00:00:01Z"},"Mounts":[],"NetworkSettings":{"Ports":{}}
        })).unwrap());
        engine.loading = false;
        engine.error = None;
        engine.tab = match tab {
            1 => Tab::Logs,
            2 => Tab::Inspect,
            _ => Tab::Overview,
        };
        engine.paused = true;
        engine.auto_scroll = tab % 2 == 0;
        engine.resources.page = ResourcePage::Containers;
        engine.resources.collection = None;
        engine.logs_query.update(cx, |query, cx| {
            query.set_value(format!("fixture-{tab}"), window, cx)
        });
        engine.log.clear();
        engine.log.push("中文 logs \n".repeat(500_000).as_bytes());
        assert!(engine.log.text().len() <= 2 * 1024 * 1024);
        engine.logs_dirty = true;
        let inspect = serde_json::to_string_pretty(engine.detail.as_ref().unwrap()).unwrap();
        engine
            .inspect_editor
            .update(cx, |editor, cx| editor.set_value(inspect, window, cx));
        self.sync_logs(window, cx);
        let target = self.target().unwrap().clone();
        self.apply_message(
            Message::Listed(
                target.clone(),
                self.query_epoch.wrapping_sub(1),
                Ok(Vec::new()),
            ),
            window,
            cx,
        );
        self.apply_message(
            Message::Log(
                target,
                id,
                self.logs_epoch.wrapping_sub(1),
                b"stale".to_vec(),
            ),
            window,
            cx,
        );
        assert_eq!(self.engine().unwrap().containers.len(), 10_000);
        assert!(!self.engine().unwrap().log.text().ends_with("stale"));
        cx.notify();
    }
    pub fn check_fixture_state(&self, tab: usize, cx: &App) {
        let engine = self.engine().expect("fixture Engine");
        assert_eq!(engine.containers.len(), 10_000);
        assert_eq!(engine.selected, engine.containers[tab].id);
        assert!(engine.paused);
        assert!(!engine.log.text().is_empty());
        assert!(
            engine.tab
                == match tab {
                    1 => Tab::Logs,
                    2 => Tab::Inspect,
                    _ => Tab::Overview,
                }
        );
        assert_eq!(engine.auto_scroll, tab % 2 == 0);
        assert_eq!(
            engine.logs_query.read(cx).value().as_ref(),
            format!("fixture-{tab}")
        );
    }
    pub fn check_resource_fixture(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_browsing();
        let engine = self.engine_mut().unwrap();
        engine.resources.update_projects(&engine.containers);
        if let Some(project) = engine.resources.projects.first_mut() {
            project.files = vec![
                "/srv/fixture/compose.yml".into(),
                "/srv/fixture/override.yml".into(),
            ];
        }
        engine.resources.images = (0..10_000)
            .map(|index| bollard::models::ImageSummary {
                id: format!("sha256:{index:064}"),
                repo_tags: vec![format!(
                    "registry.example.com/team/long-container-name-{index:05}:latest"
                )],
                size: 1234567,
                ..Default::default()
            })
            .collect();
        engine.resources.volumes = vec![bollard::models::Volume {
            name: "fixture-data".into(),
            driver: "local".into(),
            ..Default::default()
        }];
        engine.resources.networks = vec![bollard::models::Network {
            id: Some("fixture-network-full-id".into()),
            name: Some("fixture-network".into()),
            ..Default::default()
        }];
        let resource = match index {
            0 | 1 => Resource::Project("fixture-stack".into()),
            2 | 3 => Resource::Image(engine.resources.images[0].id.clone().into()),
            4 => Resource::Volume("fixture-data".into()),
            5 => Resource::Network("fixture-network-full-id".into()),
            6 => Resource::Info,
            7 => Resource::Disk,
            8 => Resource::Events,
            _ => Resource::Daemon,
        };
        let value = match &resource {
            Resource::Image(image) => {
                serde_json::json!({"Id":image.id,"RepoTags":["registry.example.com/team/fixture-tool:latest"],"Size":1234567,"Created":"2026-10-10T00:00:00Z","Architecture":"amd64","Os":"linux","Config":{"Cmd":["/bin/sh"]},"RootFS":{"Layers":["sha256:fixture-layer"]},"History":[{"CreatedBy":"COPY /app /usr/local/bin/app"},{"CreatedBy":"RUN apk add --no-cache ca-certificates"}]})
            }
            Resource::Volume(name) => {
                serde_json::json!({"Name":name,"Driver":"local","Mountpoint":"/var/lib/docker/volumes/fixture-data/_data","Scope":"local","Options":{}})
            }
            Resource::Network(id) => {
                let containers: serde_json::Map<_, _> = engine
                    .containers
                    .iter()
                    .take(40)
                    .filter_map(|container| container.id.clone())
                    .map(|id| (id, serde_json::json!({})))
                    .collect();
                serde_json::json!({"Id":id,"Driver":"bridge","Scope":"local","Internal":false,"IPAM":{"Config":[{"Subnet":"172.18.0.0/16","Gateway":"172.18.0.1"}]},"Containers":containers})
            }
            Resource::Info => {
                serde_json::json!({"ID":"fixture-engine-id","Name":"fixture-engine","ServerVersion":"28.5.0","OperatingSystem":"Linux","Architecture":"x86_64","DockerRootDir":"/var/lib/docker","Driver":"overlay2","SecurityOptions":["name=seccomp,profile=builtin"]})
            }
            Resource::Disk => {
                serde_json::json!({"LayersSize":123456789,"Images":[{},{}],"Containers":[{}],"Volumes":[{}],"BuildCache":[{},{}]})
            }
            _ => serde_json::Value::Null,
        };
        if matches!(resource, Resource::Project(_) | Resource::Daemon) {
            let path = if resource == Resource::Daemon {
                "/etc/docker/daemon.json"
            } else {
                "/srv/fixture/compose.yml"
            };
            if !engine.resources.documents.contains_key(path) {
                let text = if resource == Resource::Daemon {
                    "{\n  \"debug\": false,\n  \"registry-mirrors\": [\"https://mirror.example\"]\n}\n"
                } else {
                    "services:\n  app:\n    image: alpine:3\n    command: [sleep, infinity]\n    volumes:\n      - app-data:/data\nvolumes:\n  app-data: {}\n"
                };
                let file = crate::documents::File {
                    path: path.into(),
                    original: Some(text.into()),
                    backup: None,
                };
                let document = crate::documents::Document::new(file, &resource, window, cx);
                document.editor.update(cx, |editor, cx| {
                    editor.set_value(format!("{text}\n"), window, cx)
                });
                engine.resources.documents.insert(path.into(), document);
            }
            engine
                .resources
                .active_files
                .insert(resource.clone(), path.into());
            if resource == Resource::Daemon {
                engine.resources.daemon = Some(crate::daemon::Source {
                    path: path.into(),
                    manager: crate::daemon::Manager::System {
                        user: false,
                        reload: true,
                    },
                    engine_id: Some("fixture-engine-id".into()),
                });
            }
        }
        engine.resources.editor.update(cx, |editor, cx| {
            editor.set_value(serde_json::to_string_pretty(&value).unwrap(), window, cx)
        });
        engine.resources.preview.insert(resource.clone(), value);
        engine
            .resources
            .selected
            .insert(resource.page(), resource.clone());
        engine.resources.page = resource.page();
        if matches!(index, 1 | 3) {
            engine.resources.raw.insert(engine.resources.page);
        } else {
            engine.resources.raw.remove(&engine.resources.page);
        }
        engine.resources.events = vec![
            serde_json::json!({"Type":"container","Action":"start","Actor":{"ID":"fixture-container-full-id"},"time":1791571200}),
        ];
        if resource == Resource::Events {
            engine.resources.editor.update(cx, |editor, cx| {
                editor.set_value(
                    serde_json::to_string_pretty(&engine.resources.events).unwrap(),
                    window,
                    cx,
                )
            });
        }
        engine.resources.collection = None;
        engine.resources.output_visible = index == 10;
        engine.resources.output_resource = Some(resource.clone());
        if index == 10 {
            append_output(
                engine,
                "Fixture host · dockerd --validate\nconfiguration OK\nEngine recovered",
                window,
                cx,
            );
        }
        engine.resources.queries[&engine.resources.page]
            .update(cx, |query, cx| query.set_value("", window, cx));
        let target = self.target().unwrap().clone();
        let old = self.preview_epoch.wrapping_sub(1);
        self.apply_message(
            Message::Preview(target, resource, old, Ok(serde_json::json!({"stale":true}))),
            window,
            cx,
        );
        assert!(
            !self
                .engine()
                .unwrap()
                .resources
                .editor
                .read(cx)
                .value()
                .contains("stale")
        );
        self.sync_logs(window, cx);
        cx.notify();
    }
    pub fn check_resource_state(&self, index: usize, cx: &App) {
        let engine = self.engine().unwrap();
        assert_eq!(
            engine.resources.page,
            match index {
                0 | 1 => ResourcePage::Compose,
                2 | 3 => ResourcePage::Images,
                _ => ResourcePage::More,
            }
        );
        if matches!(index, 1 | 9) {
            assert!(
                engine
                    .resources
                    .document(engine.resources.selection().unwrap())
                    .unwrap()
                    .dirty(cx)
            );
        }
    }
    pub fn check_image_selection(&self, index: usize) {
        assert!(
            matches!(self.engine().unwrap().resources.selection(), Some(Resource::Image(reference)) if reference.id == self.engine().unwrap().resources.images[index].id)
        );
    }
    pub fn check_browser_fixture(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        let host = self.target().unwrap().id.clone();
        self.check_fixture(host, 0, window, cx);
        self.check_resource_fixture(
            match index {
                1 => 0,
                3 => 4,
                4 => 5,
                _ => 2,
            },
            window,
            cx,
        );
        let collection = crate::browser::Collection::ALL[index];
        self.show_collection(collection, window, cx);
        let engine = self.engine_mut().unwrap();
        engine
            .query
            .update(cx, |query, cx| query.set_value("", window, cx));
        engine.containers[1].state = Some(ContainerSummaryStateEnum::EXITED);
        engine.containers[1].status = Some("Exited (0) 2 hours ago".into());
        engine.containers[2].state = Some(ContainerSummaryStateEnum::PAUSED);
        engine.containers[2].status = Some("Up 12 days (Paused)".into());
        if index == 2 {
            engine.resources.images[0].repo_tags =
                vec!["caddy:latest".into(), "caddy:stable".into()];
            engine.resources.images[1].repo_tags =
                vec!["redis:7-alpine".into(), "redis:7.4-alpine".into()];
            engine.resources.images.push(bollard::models::ImageSummary {
                id: format!("sha256:{}", "f".repeat(64)),
                size: 4_500_000,
                ..Default::default()
            });
            let key = Resource::Image(engine.resources.images[0].id.clone().into());
            if let Some(preview) = engine.resources.preview.get_mut(&key) {
                preview["RepoTags"] = serde_json::json!(["caddy:latest", "caddy:stable"]);
            }
        }
        engine.resources.volumes[0].mountpoint =
            "/var/lib/docker/volumes/fixture-data/_data".into();
        engine.resources.networks[0].driver = Some("bridge".into());
        engine.resources.networks[0].scope = Some("local".into());
        engine.resources.networks[0].ipam = Some(
            serde_json::from_value(serde_json::json!({"Config":[{"Subnet":"172.18.0.0/16"}]}))
                .unwrap(),
        );
        engine.resources.table_scrolls[&collection].scroll_to_item(0, ScrollStrategy::Top);
        engine.resources.table_focus.clone()
    }
    pub fn check_image_reference(&self, index: usize, tag: Option<&str>) {
        self.check_image_selection(index);
        assert!(
            matches!(self.engine().unwrap().resources.selection(), Some(Resource::Image(reference)) if reference.tag.as_deref() == tag)
        );
    }
    pub fn check_browser_navigation(&self, index: usize, expanded: bool, inspector: bool) {
        let engine = self.engine().unwrap();
        let collection = crate::browser::Collection::ALL[index];
        assert_eq!(
            engine.resources.collection,
            if expanded { None } else { Some(collection) }
        );
        assert_eq!(engine.resources.inspector, inspector);
        if index == 0 && inspector {
            assert!(
                engine.detail.is_some(),
                "Repeated selection cleared the current preview"
            );
        }
        if index == 2 && inspector {
            assert!(
                matches!(engine.resources.selection(), Some(Resource::Image(reference)) if engine.resources.images.iter().any(|image| image.id == reference.id))
            );
        }
    }
    pub fn check_browser_return(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_collection(crate::browser::Collection::ALL[index], window, cx);
        self.check_browser_navigation(index, false, false);
    }
}

pub(crate) fn action_menu(
    mut menu: gpui_kit::component::menu::PopupMenu,
    actions: Vec<crate::actions::Action>,
    target: Option<Target>,
    resource: Option<Resource>,
    owner: WeakEntity<DockerView>,
) -> gpui_kit::component::menu::PopupMenu {
    use gpui_kit::component::menu::PopupMenuItem;
    for action in actions {
        let owner = owner.clone();
        let target = target.clone();
        let resource = resource.clone();
        menu = menu.item(PopupMenuItem::new(t!(action.key()).into_owned()).on_click(
            move |_, window, cx| {
                if let Some(target) = &target {
                    let _ = owner.update(cx, |this, cx| {
                        this.open_action(target.clone(), resource.clone(), action, window, cx)
                    });
                }
            },
        ));
    }
    menu
}
