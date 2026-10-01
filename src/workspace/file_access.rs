use super::*;

enum OpenedFile {
    Text(String),
    Png(std::sync::Arc<Image>),
}

impl AppView {
    pub(super) fn remote_files(&mut self) -> Option<remote_files::Session> {
        if self.hosts[self.active].backend.is_none() {
            return None;
        }
        let host = self.hosts[self.active].config.clone()?;
        let id = self.hosts[self.active]
            .snapshot
            .session()
            .map(|session| session.id.clone())
            .unwrap_or_default();
        Some(
            self.sftp_sessions
                .entry((self.active, id))
                .or_insert_with(|| remote_files::Session::new(host))
                .clone(),
        )
    }

    pub(super) fn load_files(&mut self, cx: &mut Context<Self>) {
        if self.workspace_mode != WorkspaceMode::Files || self.active_file_session.is_none() {
            return;
        }
        self.tree_request += 1;
        let request = self.tree_request;
        let root = self.cwd.clone();
        let expanded: BTreeSet<_> = {
            let tree = self.file_tree.read(cx);
            (0..)
                .map_while(|i| tree.entry(i))
                .filter(|entry| entry.is_expanded())
                .map(|entry| entry.item().id.to_string())
                .collect()
        };
        self.pending_file_paths = Some(Vec::new());
        self.file_tree
            .update(cx, |tree, cx| tree.set_items(Vec::new(), cx));
        self.tree_loading.clear();
        self.tree_loaded.clear();
        let host = self.remote_files();
        self.file_loading = Some(crate::t!("file.reading_directory").to_string());
        cx.spawn(async move |view, cx| {
            let requested_expansion = expanded.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    match host {
                        Some(host) => {
                            remote_files::tree_expanded(&host, &root, requested_expansion.clone())
                        }
                        None => Ok(workbench::file_nodes(&root, &requested_expansion)),
                    }
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.tree_request != request {
                    return;
                }
                match result {
                    Ok(node) => {
                        node.loaded_directories(&mut this.tree_loaded);
                        let mut items = node.item(true).children;
                        restore_expansion(&mut items, &expanded);
                        this.set_file_items(items, cx);
                        this.file_loading = None;
                    }
                    Err(error) => {
                        this.file_loading = None;
                        this.message = Some(
                            crate::t!("file.read_directory_failed", error = format!("{error:#}"))
                                .to_string(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn expand_directory(&mut self, id: String, cx: &mut Context<Self>) {
        if self.tree_loaded.contains(&id) || !self.tree_loading.insert(id.clone()) {
            return;
        }
        let request = self.tree_request;
        let host = self.remote_files();
        let path = PathBuf::from(&id);
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match host {
                        Some(host) => remote_files::tree(&host, &path),
                        None => Ok(workbench::file_nodes(&path, &Default::default())),
                    }
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                if this.tree_request != request {
                    return;
                }
                this.tree_loading.remove(&id);
                match result {
                    Ok(node) => {
                        node.loaded_directories(&mut this.tree_loaded);
                        let children = node.item(true).children;
                        let tree = this.file_tree.read(cx);
                        let mut items: Vec<_> = (0..)
                            .map_while(|i| tree.entry(i))
                            .filter(|e| e.depth() == 0)
                            .map(|e| e.item().clone())
                            .collect();
                        replace_children(&mut items, &id, &children);
                        this.set_file_items(items, cx);
                    }
                    Err(error) => {
                        this.message = Some(
                            crate::t!("file.read_directory_failed", error = format!("{error:#}"))
                                .to_string(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_file_items(
        &mut self,
        items: Vec<gpui_kit::component::tree::TreeItem>,
        cx: &mut Context<Self>,
    ) {
        self.pending_file_paths = Some(if self.hosts[self.active].config.is_some() {
            remote_paths(&items)
        } else {
            workbench::file_paths(&items, &self.cwd)
        });
        if let Some(paths) = &mut self.pending_file_paths {
            paths.retain(|path| {
                let absolute = if self.hosts[self.active].config.is_some() {
                    PathBuf::from(path)
                } else {
                    self.cwd.join(path)
                };
                !self
                    .tree_loaded
                    .contains(absolute.to_string_lossy().as_ref())
            });
        }
        self.file_tree.update(cx, |tree, cx| {
            let selected = tree.selected_item().cloned();
            tree.set_items(items, cx);
            tree.set_selected_item(selected.as_ref(), cx);
        });
    }

    pub(super) fn refresh_files(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.load_files(cx);
        cx.notify();
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
        if self.hosts[self.active].config.is_some() && self.hosts[self.active].backend.is_none() {
            return;
        }
        if self.open_file.as_ref() == Some(&path) && self.pending_file_state.is_none() {
            self.file_request += 1;
            self.file_loading = None;
            self.workspace_mode = WorkspaceMode::Files;
            self.file_link_position = position.map(|position| (path, position));
            self.need_focus = true;
            cx.notify();
            return;
        }
        if self.editor_dirty {
            let key = self.active_file_session.clone();
            let title = crate::t!("file.unsaved_title").to_string();
            let detail = crate::t!("file.unsaved_message").to_string();
            let cancel = crate::t!("explorer.cancel").into_owned();
            let discard = crate::t!("file.discard").into_owned();
            let receiver = window.prompt(
                PromptLevel::Warning,
                &title,
                Some(&detail),
                &[cancel.as_str(), discard.as_str()],
                cx,
            );
            cx.spawn_in(window, async move |view, cx| {
                if receiver.await == Ok(1) {
                    let _ = view.update_in(cx, |this, window, cx| {
                        if this.active_file_session == key {
                            this.start_open_file(path, position, window, cx);
                        }
                    });
                }
            })
            .detach();
            return;
        }
        self.start_open_file(path, position, window, cx);
    }

    fn start_open_file(
        &mut self,
        path: PathBuf,
        position: Option<(u32, u32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let original_text = self.file_editor.read(cx).value().to_string();
        self.file_request += 1;
        let request = self.file_request;
        let host = if path == Self::theme_path() {
            None
        } else {
            self.remote_files()
        };
        self.file_loading = Some(crate::t!("file.reading_file").to_string());
        cx.spawn_in(window, async move |view, cx| {
            let read_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let png = read_path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"));
                    if png {
                        let bytes = match host {
                            Some(host) => remote_files::read_bytes(&host, &read_path)?,
                            None => {
                                anyhow::ensure!(
                                    std::fs::metadata(&read_path)?.len()
                                        <= workbench::MAX_FILE_BYTES,
                                    "PNG is too large"
                                );
                                std::fs::read(&read_path)?
                            }
                        };
                        anyhow::ensure!(
                            bytes.len() <= workbench::MAX_FILE_BYTES as usize
                                && bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                            "Invalid or oversized PNG"
                        );
                        Ok(OpenedFile::Png(std::sync::Arc::new(Image::from_bytes(
                            ImageFormat::Png,
                            bytes,
                        ))))
                    } else {
                        let text = match host {
                            Some(host) => remote_files::read(&host, &read_path)?,
                            None => workbench::read_file(&read_path)?,
                        };
                        Ok(OpenedFile::Text(text))
                    }
                })
                .await;
            let _ = view.update_in(cx, |this, window, cx| {
                if this.file_request != request {
                    return;
                }
                this.file_loading = None;
                match result {
                    Ok(opened) if this.file_editor.read(cx).value().as_ref() == original_text => {
                        let (text, image) = match opened {
                            OpenedFile::Text(text) => (text, None),
                            OpenedFile::Png(image) => (String::new(), Some(image)),
                        };
                        let language = workbench::language_for_path(&path).to_owned();
                        let preview_mode = preview::can_preview(&language, &text);
                        let preview = preview_mode.then(|| preview::parse(&language, &text));
                        this.pending_file_state = Some(SessionFileState {
                            path: Some(path.clone()),
                            saved_text: text.clone(),
                            text,
                            image,
                            preview,
                            preview_mode,
                            language,
                            dirty: false,
                        });
                        this.file_link_position = position.map(|position| (path, position));
                        this.workspace_mode = WorkspaceMode::Files;
                        this.need_focus = true;
                    }
                    Ok(_) => window.push_notification(
                        Notification::warning(crate::t!("file.changed_again")),
                        cx,
                    ),
                    Err(error) => {
                        window.push_notification(Notification::error(format!("{error:#}")), cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn save_open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.image_preview.is_some() {
            return;
        }
        let Some(path) = self.open_file.clone() else {
            return;
        };
        if self.file_saving {
            return;
        }
        self.file_saving = true;
        let host = if path == Self::theme_path() {
            None
        } else {
            self.remote_files()
        };
        let key = self.active_file_session.clone();
        let text = self.file_editor.read(cx).value().to_string();
        let expected = self.saved_file_text.clone();
        cx.spawn_in(window, async move |view, cx| {
            let save_path = path.clone();
            let saved_text = text.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    match host {
                        Some(host) => {
                            remote_files::write(&host, &save_path, &saved_text, &expected)
                        }
                        None => workbench::write_file(&save_path, &saved_text),
                    }
                })
                .await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.file_saving = false;
                match result {
                    Ok(()) => {
                        if path == Self::theme_path() {
                            this.reload_theme_file(window, cx);
                        }
                        if this.active_file_session == key {
                            if let Some(state) = this
                                .pending_file_state
                                .as_mut()
                                .filter(|state| state.path.as_ref() == Some(&path))
                            {
                                state.saved_text = text.clone();
                                state.dirty = state.text != text;
                            } else if this.open_file.as_ref() == Some(&path) {
                                this.saved_file_text = text.clone();
                                this.editor_dirty =
                                    this.file_editor.read(cx).value().as_ref() != text;
                            }
                        } else if let Some(state) = key
                            .as_ref()
                            .and_then(|key| this.file_states.get_mut(key))
                            .filter(|state| state.path.as_ref() == Some(&path))
                        {
                            state.saved_text = text.clone();
                            state.dirty = state.text != text;
                        }
                        window.push_notification(
                            Notification::success(
                                crate::t!("file.saved", path = path.display().to_string())
                                    .to_string(),
                            ),
                            cx,
                        );
                    }
                    Err(error) => {
                        window.push_notification(Notification::error(format!("{error:#}")), cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn remote_paths(items: &[gpui_kit::component::tree::TreeItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|item| {
            if item.children.is_empty() && !item.is_disabled() {
                vec![item.id.to_string()]
            } else {
                remote_paths(&item.children)
            }
        })
        .collect()
}

fn restore_expansion(
    items: &mut [gpui_kit::component::tree::TreeItem],
    expanded: &BTreeSet<String>,
) {
    for item in items {
        *item = item.clone().expanded(expanded.contains(item.id.as_ref()));
        restore_expansion(&mut item.children, expanded);
    }
}

fn replace_children(
    items: &mut [gpui_kit::component::tree::TreeItem],
    id: &str,
    children: &[gpui_kit::component::tree::TreeItem],
) {
    for item in items {
        if item.id.as_ref() == id {
            item.children = children.to_vec();
            return;
        }
        replace_children(&mut item.children, id, children);
    }
}
