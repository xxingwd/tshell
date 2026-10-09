//! The application owns session routes; feature entities own their state and IO.
use super::*;
impl AppView {
    pub(super) fn show_files(&mut self, cx: &mut Context<Self>) {
        self.open_tools(Page::Explorer, cx);
    }
    pub(super) fn show_git(&mut self, cx: &mut Context<Self>) {
        self.open_tools(Page::Git, cx);
    }
    pub(super) fn show_terminal(&mut self, cx: &mut Context<Self>) {
        self.page = Page::Terminal;
        self.need_focus = true;
        cx.notify();
    }
    pub(super) fn explorer_entity(&self) -> Option<Entity<Explorer>> {
        self.active_workspace
            .as_ref()
            .and_then(|key| self.workspaces.get(key))
            .and_then(|workspace| workspace.explorer.clone())
    }
    pub(super) fn git_entity(&self) -> Option<Entity<GitView>> {
        self.active_workspace
            .as_ref()
            .and_then(|key| self.workspaces.get(key))
            .and_then(|workspace| workspace.git.clone())
    }
    pub(super) fn remote_files(&mut self) -> Option<remote_files::Session> {
        if self.hosts[self.active].backend.is_none() {
            return None;
        }
        self.hosts[self.active]
            .config
            .as_ref()
            .map(|host| remote_files::Session::new(host.ssh.clone()))
    }
    fn refresh_tools(&mut self, cx: &mut Context<Self>) {
        match self.page {
            Page::Explorer => self.load_files(cx),
            Page::Git => self.refresh_git(cx),
            Page::Terminal | Page::Docker => {}
        }
        cx.notify();
    }
    pub(super) fn load_files(&mut self, cx: &mut Context<Self>) {
        if let Some(entity) = self.explorer_entity()
            && entity.read(cx).visible
        {
            entity.update(cx, |explorer, cx| explorer.load_files(cx));
            if let Some(workspace) = self
                .active_workspace
                .as_ref()
                .and_then(|key| self.workspaces.get_mut(key))
            {
                workspace.refresh = false;
            }
        } else if let Some(workspace) = self
            .active_workspace
            .as_ref()
            .and_then(|key| self.workspaces.get_mut(key))
        {
            workspace.refresh = true;
        }
    }
    pub(super) fn refresh_files(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.load_files(cx);
        cx.notify();
    }
    pub(super) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        if let Some(entity) = self.git_entity() {
            entity.update(cx, |git, cx| git.refresh_git(cx));
        } else if let Some(workspace) = self
            .active_workspace
            .as_ref()
            .and_then(|key| self.workspaces.get_mut(key))
        {
            workspace.refresh = true;
        }
    }
    pub(super) fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_path_at(path, None, window, cx);
    }
    pub(super) fn open_path_at(
        &mut self,
        path: PathBuf,
        position: Option<(u32, u32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page = Page::Explorer;
        self.ensure_tools(window, cx);
        if let Some(entity) = self.explorer_entity() {
            entity.update(cx, |explorer, cx| {
                let local = path == Self::theme_path();
                explorer.open_path_at_on(path, position, local, window, cx);
            });
        }
        cx.notify();
    }
    pub(super) fn save_open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(entity) = self.explorer_entity() {
            entity.update(cx, |explorer, cx| explorer.save_open_file(window, cx));
        }
    }
    fn ensure_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigation_loading.is_some() {
            return;
        }
        let Some(key) = self.active_workspace.clone() else {
            return;
        };
        let Some(host) = self.hosts.iter().find(|host| host.identity() == key.0) else {
            return;
        };
        let config = host.config.as_ref().map(|host| host.ssh.clone());
        let cwd = self
            .workspaces
            .get(&key)
            .and_then(|workspace| workspace.cwd.clone())
            .unwrap_or_else(|| self.cwd.clone());
        let workspace = self.workspaces.entry(key.clone()).or_default();
        if self.page == Page::Explorer && workspace.explorer.is_none() {
            let entity = cx.new(|cx| {
                Explorer::new(
                    config.clone(),
                    cwd.clone(),
                    self.palette,
                    self.terminal_palette,
                    self.font_family.clone(),
                    self.font_size,
                    window,
                    cx,
                )
            });
            if let Some(clipboard) = self
                .workspaces
                .iter()
                .filter(|((id, _), _)| id == &key.0)
                .filter_map(|(_, workspace)| workspace.explorer.as_ref())
                .next()
                .map(|other| other.read(cx).file_clipboard.clone())
            {
                entity.update(cx, |explorer, _| explorer.file_clipboard = clipboard);
            }
            let source = key.clone();
            let subscription =
                cx.subscribe_in(&entity, window, move |this, entity, event, window, cx| {
                    this.explorer_event(source.clone(), entity.clone(), event.clone(), window, cx)
                });
            let observation = cx.observe(&entity, |_, _, cx| cx.notify());
            let workspace = self.workspaces.get_mut(&key).unwrap();
            workspace.explorer = Some(entity);
            workspace.subscriptions.extend([subscription, observation]);
            workspace.refresh = true;
        }
        let workspace = self.workspaces.get(&key).unwrap();
        if self.page == Page::Git && workspace.git.is_none() {
            let entity = cx.new(|_| {
                GitView::new(
                    config,
                    cwd,
                    self.palette,
                    self.terminal_palette,
                    self.font_family.clone(),
                    self.font_size,
                )
            });
            let source = key.clone();
            let subscription =
                cx.subscribe_in(&entity, window, move |this, _, event, window, cx| {
                    let tshell_git::Event::OpenFile(path) = event;
                    this.open_workspace_file(source.clone(), path.clone(), window, cx);
                });
            let observation = cx.observe(&entity, |_, _, cx| cx.notify());
            let workspace = self.workspaces.get_mut(&key).unwrap();
            workspace.git = Some(entity);
            workspace.subscriptions.extend([subscription, observation]);
            workspace.refresh = true;
        }
    }
    fn open_workspace_file(
        &mut self,
        key: WorkspaceKey,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspaces.contains_key(&key) {
            return;
        }
        let Some(index) = self.hosts.iter().position(|host| host.identity() == key.0) else {
            return;
        };
        if !self.hosts[index]
            .snapshot
            .sessions
            .iter()
            .any(|session| session.id == key.1)
        {
            return;
        }
        self.switch_host(index, cx);
        self.act(Action::SelectSession(key.1.clone()), cx);
        self.sync(cx);
        self.open_path(path, window, cx);
    }
    fn explorer_event(
        &mut self,
        key: WorkspaceKey,
        entity: Entity<Explorer>,
        event: explorer::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .workspaces
            .get(&key)
            .and_then(|workspace| workspace.explorer.as_ref())
            .is_some_and(|current| current == &entity)
        {
            return;
        }
        match event {
            explorer::Event::Activated => {
                if self.active_workspace.as_ref() == Some(&key) {
                    self.page = Page::Explorer;
                    self.need_focus = true;
                }
            }
            explorer::Event::Saved(path) => {
                if path == Self::theme_path() && entity.read(cx).document_local {
                    self.reload_theme_file(window, cx);
                }
            }
            explorer::Event::OpenTerminal(path) => {
                if let Some(index) = self.hosts.iter().position(|host| host.identity() == key.0) {
                    if !self.hosts[index]
                        .snapshot
                        .sessions
                        .iter()
                        .any(|session| session.id == key.1)
                    {
                        return;
                    }
                    self.switch_host(index, cx);
                    self.act(Action::SelectSession(key.1.clone()), cx);
                    self.act(
                        Action::NewWindowAt {
                            path: path.to_string_lossy().into_owned(),
                            name: String::new(),
                        },
                        cx,
                    );
                    self.show_terminal(cx);
                }
            }
            explorer::Event::Upload {
                remote,
                paths,
                target,
            } => self.queue_upload_for(Some(key), remote, paths, target, window, cx),
            explorer::Event::Download {
                remote,
                source,
                destination,
            } => self.queue_download_for(Some(key), remote, source, destination, window, cx),
            explorer::Event::RequestOperation(operation) => {
                let blocked = self
                    .workspaces
                    .iter()
                    .filter(|((host, _), _)| *host == key.0)
                    .filter_map(|(_, workspace)| workspace.explorer.as_ref())
                    .any(|entity| entity.read(cx).operation_conflicts(&operation));
                if blocked {
                    window.push_notification(
                        Notification::warning(crate::t!("explorer.save_first")),
                        cx,
                    );
                } else {
                    entity.update(cx, |explorer, cx| {
                        explorer.perform_operation(operation, window, cx)
                    });
                }
            }
            explorer::Event::FilesChanged(operation) => {
                let entities = self
                    .workspaces
                    .iter()
                    .filter(|((host, _), _)| *host == key.0)
                    .filter_map(|(_, workspace)| workspace.explorer.clone())
                    .filter(|other| *other != entity)
                    .collect::<Vec<_>>();
                for entity in entities {
                    entity.update(cx, |explorer, cx| explorer.files_changed(&operation, cx));
                }
            }
        }
        cx.notify();
    }
    pub(super) fn prepare_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_tools(window, cx);
        let entities = self
            .workspaces
            .iter()
            .filter_map(|(key, workspace)| {
                workspace
                    .explorer
                    .clone()
                    .map(|entity| (key.clone(), entity))
            })
            .collect::<Vec<_>>();
        for (key, entity) in entities {
            let visible = self.page == Page::Explorer
                && self.active_workspace.as_ref() == Some(&key)
                && !self.settings
                && !self.command_palette;
            entity.update(cx, |explorer, cx| {
                explorer.palette = self.palette;
                explorer.terminal_palette = self.terminal_palette;
                explorer.font_family = self.font_family.clone();
                explorer.font_size = self.font_size;
                if explorer.visible != visible {
                    explorer.set_visible(visible, cx);
                }
                explorer.prepare(window, cx);
            });
        }
        if let Some(entity) = self.git_entity() {
            entity.update(cx, |git, _| {
                git.palette = self.palette;
                git.terminal_palette = self.terminal_palette;
                git.font_family = self.font_family.clone();
                git.font_size = self.font_size;
            });
        }
        if let Some(workspace) = self
            .active_workspace
            .as_ref()
            .and_then(|key| self.workspaces.get_mut(key))
            && workspace.refresh
            && matches!(self.page, Page::Explorer | Page::Git)
        {
            workspace.refresh = false;
            self.refresh_tools(cx);
        }
    }
    pub(super) fn open_tools(&mut self, mode: Page, cx: &mut Context<Self>) {
        if self.active_is_container() {
            self.message = Some(crate::t!("docker.file_tools_unavailable").into_owned());
            cx.notify();
            return;
        }
        let Some(key) = self.active_workspace.clone() else {
            return;
        };
        self.page = mode;
        self.need_focus = true;
        if self
            .workspaces
            .get(&key)
            .is_some_and(|workspace| workspace.cwd.is_some())
        {
            self.refresh_tools(cx);
            return;
        }
        let host = &self.hosts[self.active];
        let Some(tab) = host.snapshot.window() else {
            return;
        };
        let pane = tab.active_pane.clone();
        let path = tab
            .panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| p.cwd.clone())
            .filter(|p| !p.is_empty());
        let local = host
            .backend
            .as_ref()
            .and_then(|b| b.screen(&pane))
            .and_then(|s| s.current_directory());
        let fallback = host
            .snapshot
            .session()
            .map(|s| s.cwd.clone())
            .filter(|p| !p.is_empty());
        let tmux = self.hosts[self.active]
            .config
            .as_ref()
            .filter(|host| host.tmux)
            .map(HostDefinition::tmux)
            .transpose();
        let remote = self.remote_files();
        self.tool_request += 1;
        let request = self.tool_request;
        self.navigation_loading = Some(crate::t!("tools.reading_terminal_directory").to_string());
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(host) = remote {
                        if let Some(tmux) = tmux? {
                            let command = format!(
                                "{} display-message -p -t '{}' '#{{pane_current_path}}'",
                                tmux,
                                pane.replace('\'', "'\\''")
                            );
                            let bytes = crate::ssh_pool::output(&host, command, 65536)?;
                            let path = String::from_utf8(bytes)?
                                .trim_end_matches(['\r', '\n'])
                                .to_owned();
                            anyhow::ensure!(
                                !path.is_empty(),
                                "{}",
                                crate::t!("tools.tmux_no_pane_directory")
                            );
                            Ok(PathBuf::from(path))
                        } else {
                            let path = local.or(path).or(fallback).unwrap_or_else(|| "~".into());
                            remote_files::directory(&host, &path).map(PathBuf::from)
                        }
                    } else {
                        Ok(local
                            .or(path)
                            .or(fallback)
                            .map(PathBuf::from)
                            .or_else(dirs::home_dir)
                            .unwrap_or_default())
                    }
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.tool_request != request || this.active_workspace.as_ref() != Some(&key) {
                    return;
                }
                match result {
                    Ok(root) => {
                        this.cwd = root.clone();
                        if let Some(workspace) = this.workspaces.get_mut(&key) {
                            workspace.cwd = Some(root.clone());
                            workspace.refresh = true;
                            if let Some(entity) = &workspace.explorer {
                                entity.update(cx, |explorer, _| explorer.cwd = root.clone());
                            }
                            if let Some(entity) = &workspace.git {
                                entity.update(cx, |git, _| git.cwd = root.clone());
                            }
                        }
                        this.navigation_loading = None;
                        this.refresh_tools(cx);
                    }
                    Err(error) => {
                        this.navigation_loading = None;
                        this.message = Some(
                            crate::t!(
                                "tools.read_directory_failed",
                                error = crate::i18n::format_error(&error)
                            )
                            .to_string(),
                        );
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(debug_assertions)]
impl AppView {
    pub(super) fn check_explorer<'a>(&'a self, cx: &'a App) -> &'a Explorer {
        self.explorer_entity_ref()
            .expect("UI check Explorer entity")
            .read(cx)
    }
    fn explorer_entity_ref(&self) -> Option<&Entity<Explorer>> {
        self.active_workspace
            .as_ref()
            .and_then(|key| self.workspaces.get(key))
            .and_then(|workspace| workspace.explorer.as_ref())
    }
    pub(super) fn check_git<'a>(&'a self, cx: &'a App) -> &'a GitView {
        self.active_workspace
            .as_ref()
            .and_then(|key| self.workspaces.get(key))
            .and_then(|workspace| workspace.git.as_ref())
            .expect("UI check Git entity")
            .read(cx)
    }
    pub(super) fn check_initialize_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.page;
        self.page = Page::Explorer;
        self.ensure_tools(window, cx);
        self.page = Page::Git;
        self.ensure_tools(window, cx);
        self.page = page;
        if let Some(workspace) = self
            .active_workspace
            .as_ref()
            .and_then(|key| self.workspaces.get_mut(key))
        {
            workspace.refresh = false;
        }
    }
    pub(super) fn set_diff_mode(&mut self, mode: git_view::DiffMode, cx: &mut Context<Self>) {
        if let Some(entity) = self.git_entity() {
            entity.update(cx, |git, cx| git.set_diff_mode(mode, cx));
        }
    }
}
