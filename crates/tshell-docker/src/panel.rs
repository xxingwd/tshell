use super::{Endpoint, Target, logs::LogBuffer, t};
use bollard::{
    Docker,
    models::{ContainerInspectResponse, ContainerSummary, ContainerSummaryStateEnum, SystemInfo},
    query_parameters::{
        EventsOptionsBuilder, ListContainersOptionsBuilder, LogsOptionsBuilder,
        RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder, StopContainerOptionsBuilder,
    },
};
use futures_util::StreamExt;
use gpui_kit::component::{
    WindowExt,
    input::{EditorState, InputEvent, InputState},
    notification::Notification,
};
use gpui_kit::{prelude::*, *};
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};
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
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filter {
    All,
    Running,
    Stopped,
}
#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Start,
    Stop,
    Restart,
    Remove,
}
impl Operation {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Start => "docker.start",
            Self::Stop => "docker.stop",
            Self::Restart => "docker.restart",
            Self::Remove => "docker.remove",
        }
    }
}
pub(crate) struct Engine {
    pub target: Target,
    pub docker: Option<Docker>,
    pub info: Option<SystemInfo>,
    pub containers: Vec<ContainerSummary>,
    pub selected: Option<String>,
    pub detail: Option<ContainerInspectResponse>,
    pub loading: bool,
    pub error: Option<Failure>,
    pub busy: Option<String>,
    pub tab: Tab,
    pub filter: Filter,
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
fn list_options() -> bollard::query_parameters::ListContainersOptions {
    ListContainersOptionsBuilder::default().all(true).build()
}
enum Message {
    Connected(
        Target,
        u64,
        Outcome<(Docker, SystemInfo, Vec<ContainerSummary>)>,
    ),
    Listed(Target, u64, Outcome<Vec<ContainerSummary>>),
    Detail(Target, String, u64, Outcome<ContainerInspectResponse>),
    Log(Target, String, u64, Vec<u8>),
    LogEnd(Target, String, u64, Option<Failure>),
    Operation(Target, String, Outcome<()>, Outcome<Vec<ContainerSummary>>),
    Refresh(Target),
    EventsError(Target, Failure),
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
    #[cfg(debug_assertions)]
    fixture: bool,
    _updates: Task<()>,
}
impl EventEmitter<Event> for DockerView {}
impl DockerView {
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
        self.query_epoch += 1;
        self.detail_epoch += 1;
        self.logs_epoch += 1;
        self.query_worker = None;
        self.detail_worker = None;
        self.logs_worker = None;
        self.events_worker = None;
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
                    loading: false,
                    error: None,
                    busy: None,
                    tab: Tab::Overview,
                    filter: Filter::All,
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
            let filters = HashMap::from([("type".to_owned(), vec!["container".to_owned()])]);
            let options = EventsOptionsBuilder::default().filters(&filters).build();
            let mut delay = 1;
            loop {
                let mut events = docker.events(Some(options.clone()));
                while let Some(event) = events.next().await {
                    match event {
                        Ok(_) => {
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
    pub(crate) fn select_container(&mut self, id: String, cx: &mut Context<Self>) {
        self.detail_epoch += 1;
        self.logs_epoch += 1;
        self.detail_worker = None;
        self.logs_worker = None;
        if let Some(engine) = self.engine_mut() {
            if engine.selected.as_ref() != Some(&id) {
                engine.log.clear();
                engine.logs_dirty = true;
            }
            engine.selected = Some(id);
            engine.detail = None;
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
            let result = docker.inspect_container(&id, None).await.map_err(failure);
            let _ = sender
                .send(Message::Detail(target, id, epoch, result))
                .await;
        }));
        cx.notify();
    }
    pub(crate) fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine_mut() {
            engine.tab = tab;
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
        if !self.visible || engine.paused {
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
                        Ok(detail) => {
                            let text = serde_json::to_string_pretty(&detail).unwrap_or_default();
                            engine
                                .inspect_editor
                                .update(cx, |editor, cx| editor.set_value(text, window, cx));
                            engine.detail = Some(detail);
                        }
                        Err(error) => engine.error = Some(error),
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
            Message::Operation(target, id, result, listing) => {
                self.operations.remove(&target.id);
                let message = match &result {
                    Ok(()) => Notification::success(
                        t!(
                            "docker.operation_completed",
                            engine = target.name,
                            container = id
                        )
                        .into_owned(),
                    ),
                    Err(error) => Notification::error(format!(
                        "{} · {}: {}",
                        target.name,
                        id,
                        tshell_ui::i18n::format_failure(error)
                    )),
                };
                window.push_notification(message, cx);
                if let Some(engine) = self
                    .engines
                    .get_mut(&target.id)
                    .filter(|engine| engine.target.endpoint == target.endpoint)
                {
                    engine.busy = None;
                    engine.error = result.err();
                    match listing {
                        Ok(containers) => engine.containers = containers,
                        Err(error) => {
                            if engine.error.is_none() {
                                engine.error = Some(error);
                            }
                        }
                    }
                }
                if self.matches(&target) {
                    self.after_listing(cx);
                }
                let _ = id;
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
        }
    }
    fn matches(&self, target: &Target) -> bool {
        self.target()
            .is_some_and(|current| current.id == target.id && current.endpoint == target.endpoint)
    }
    fn after_listing(&mut self, cx: &mut Context<Self>) {
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
        } else if self
            .engine()
            .is_some_and(|engine| engine.selected.is_some())
        {
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
    pub(crate) fn confirm_operation(
        &mut self,
        operation: Operation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.engine() else {
            return;
        };
        let (Some(docker), Some(id)) = (engine.docker.clone(), engine.selected.clone()) else {
            return;
        };
        if engine.busy.is_some() {
            return;
        }
        let target = engine.target.clone();
        let title = t!(operation.key()).into_owned();
        let detail = t!(
            "docker.confirm_operation",
            engine = target.name.clone(),
            container = id.clone()
        )
        .into_owned();
        let cancel = t!("explorer.cancel").into_owned();
        let confirm = t!("explorer.confirm").into_owned();
        let receiver = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &[cancel.as_str(), confirm.as_str()],
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            if receiver.await == Ok(1) {
                let _ = view.update_in(cx, |this, window, cx| {
                    if !this.targets.iter().any(|current| {
                        current.id == target.id && current.endpoint == target.endpoint
                    }) {
                        window.push_notification(
                            Notification::warning(t!("docker.target_changed")),
                            cx,
                        );
                        return;
                    }
                    this.perform_operation(target, docker, id, operation, cx);
                });
            }
        })
        .detach();
    }
    fn perform_operation(
        &mut self,
        target: Target,
        docker: Docker,
        id: String,
        operation: Operation,
        cx: &mut Context<Self>,
    ) {
        if self.operations.contains_key(&target.id) {
            return;
        }
        if let Some(engine) = self.engines.get_mut(&target.id) {
            engine.busy = Some(id.clone());
            engine.error = None;
        }
        let sender = self.sender.clone();
        let host = target.id.clone();
        self.operations.insert(
            host,
            worker(async move {
                let result: Result<(), anyhow::Error> = async {
                    let detail = docker.inspect_container(&id, None).await?;
                    let running = detail
                        .state
                        .as_ref()
                        .is_some_and(|state| state.running == Some(true));
                    match operation {
                        Operation::Start => {
                            anyhow::ensure!(
                                !running,
                                tshell_host::diagnostic!("docker.state_changed")
                            );
                            docker.start_container(&id, None).await?;
                        }
                        Operation::Stop => {
                            anyhow::ensure!(
                                running,
                                tshell_host::diagnostic!("docker.state_changed")
                            );
                            docker
                                .stop_container(
                                    &id,
                                    Some(StopContainerOptionsBuilder::default().t(10).build()),
                                )
                                .await?;
                        }
                        Operation::Restart => {
                            anyhow::ensure!(
                                running,
                                tshell_host::diagnostic!("docker.state_changed")
                            );
                            docker
                                .restart_container(
                                    &id,
                                    Some(RestartContainerOptionsBuilder::default().t(10).build()),
                                )
                                .await?;
                        }
                        Operation::Remove => {
                            anyhow::ensure!(
                                !running,
                                tshell_host::diagnostic!("docker.remove_running")
                            );
                            docker
                                .remove_container(
                                    &id,
                                    Some(
                                        RemoveContainerOptionsBuilder::default()
                                            .force(false)
                                            .v(false)
                                            .build(),
                                    ),
                                )
                                .await?;
                        }
                    }
                    Ok(())
                }
                .await;
                // A failed mutation may have reached the daemon. Read its state; never replay it.
                let listing = docker
                    .list_containers(Some(list_options()))
                    .await
                    .map_err(failure);
                let _ = sender
                    .send(Message::Operation(
                        target,
                        id,
                        result.map_err(failure),
                        listing,
                    ))
                    .await;
            }),
        );
        cx.notify();
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
    pub fn check_layout(&self, cx: &App) {
        let engine = self.engine().expect("Docker fixture Engine");
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
        engine.filter = if tab == 1 {
            Filter::Running
        } else {
            Filter::All
        };
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
        assert!(
            engine.filter
                == if tab == 1 {
                    Filter::Running
                } else {
                    Filter::All
                }
        );
        assert_eq!(
            engine.logs_query.read(cx).value().as_ref(),
            format!("fixture-{tab}")
        );
    }
}
