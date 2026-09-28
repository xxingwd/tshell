use super::*;
use file_ops::Operation;
use gpui_kit::component::menu::PopupMenu;

#[derive(Clone)]
pub(super) struct FileClipboard {
    host: Option<HostConfig>,
    path: PathBuf,
    cut: bool,
}
#[derive(Clone, Copy)]
enum Command {
    Open,
    NewFile,
    NewFolder,
    Rename,
    Delete,
    Copy,
    Cut,
    Paste,
    Download,
    UploadFiles,
    UploadFolder,
    Path,
    Relative,
    Refresh,
    Collapse,
    Terminal,
    Reveal,
}

impl AppView {
    pub(super) fn explorer_menu(
        &self,
        mut menu: PopupMenu,
        path: PathBuf,
        directory: bool,
        root: bool,
        owner: WeakEntity<Self>,
    ) -> PopupMenu {
        let mut items: Vec<(String, Command)> = Vec::new();
        if !directory {
            items.push((crate::t!("explorer.open").to_string(), Command::Open));
        }
        if !root && self.hosts[self.active].config.is_some() {
            items.push((
                crate::t!("explorer.download").to_string(),
                Command::Download,
            ));
        }
        if directory {
            items.extend([
                (crate::t!("explorer.new_file").to_string(), Command::NewFile),
                (
                    crate::t!("explorer.new_folder").to_string(),
                    Command::NewFolder,
                ),
            ]);
            if self.hosts[self.active].config.is_some() {
                items.extend([
                    (
                        crate::t!("explorer.upload_files").to_string(),
                        Command::UploadFiles,
                    ),
                    (
                        crate::t!("explorer.upload_folder").to_string(),
                        Command::UploadFolder,
                    ),
                ]);
            }
        }
        if !root {
            items.extend([
                (crate::t!("explorer.rename").to_string(), Command::Rename),
                (crate::t!("explorer.copy").to_string(), Command::Copy),
                (crate::t!("explorer.cut").to_string(), Command::Cut),
            ]);
        }
        if directory
            && self
                .file_clipboard
                .as_ref()
                .is_some_and(|clip| clip.host == self.hosts[self.active].config)
        {
            items.push((crate::t!("explorer.paste").to_string(), Command::Paste));
        }
        items.extend([
            (crate::t!("explorer.copy_path").to_string(), Command::Path),
            (
                crate::t!("explorer.copy_relative").to_string(),
                Command::Relative,
            ),
            (
                crate::t!("explorer.open_terminal").to_string(),
                Command::Terminal,
            ),
        ]);
        if self.hosts[self.active].config.is_none() {
            items.push((crate::t!("explorer.reveal").to_string(), Command::Reveal));
        }
        items.extend([
            (crate::t!("explorer.refresh").to_string(), Command::Refresh),
            (
                crate::t!("explorer.collapse_all").to_string(),
                Command::Collapse,
            ),
        ]);
        if !root {
            items.push((crate::t!("explorer.delete").to_string(), Command::Delete));
        }
        let key = self.active_file_session.clone();
        let host = self.active;
        for (label, command) in items {
            if matches!(command, Command::Path | Command::Refresh | Command::Delete) {
                menu = menu.separator();
            }
            let (owner, path, key) = (owner.clone(), path.clone(), key.clone());
            menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let _ = owner.update(cx, |app, cx| {
                    if app.active == host && app.active_file_session == key {
                        app.explorer_command(command, path.clone(), directory, window, cx);
                    }
                });
            }));
        }
        menu
    }

    fn explorer_command(
        &mut self,
        command: Command,
        path: PathBuf,
        directory: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent = if directory {
            path.clone()
        } else {
            path.parent().unwrap_or(&self.cwd).to_owned()
        };
        match command {
            Command::Open => self.open_path(path, window, cx),
            Command::Download => self.explorer_download(path, directory, window, cx),
            Command::UploadFiles | Command::UploadFolder => self.explorer_pick_upload(
                parent,
                matches!(command, Command::UploadFolder),
                window,
                cx,
            ),
            Command::NewFile | Command::NewFolder | Command::Rename => {
                self.explorer_name(command, path, window, cx)
            }
            Command::Copy | Command::Cut => {
                self.file_clipboard = Some(FileClipboard {
                    host: self.hosts[self.active].config.clone(),
                    path,
                    cut: matches!(command, Command::Cut),
                });
            }
            Command::Path | Command::Relative => {
                let text = if matches!(command, Command::Relative) {
                    path.strip_prefix(&self.cwd).unwrap_or(&path)
                } else {
                    &path
                };
                let text = if text.as_os_str().is_empty() {
                    ".".into()
                } else {
                    text.to_string_lossy().into_owned()
                };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Command::Paste => {
                let Some(clip) = self.file_clipboard.clone() else {
                    return;
                };
                let Some(name) = clip.path.file_name().and_then(|name| name.to_str()) else {
                    return;
                };
                let to = match file_ops::child(&parent, name, clip.host.is_some()) {
                    Ok(path) => path,
                    Err(_) => return,
                };
                let operation = if clip.cut {
                    Operation::Rename {
                        from: clip.path,
                        to,
                    }
                } else {
                    Operation::Copy {
                        from: clip.path,
                        to,
                    }
                };
                self.explorer_run(operation, window, cx);
            }
            Command::Delete => {
                let key = self.active_file_session.clone();
                let host = self.active;
                let title = crate::t!("explorer.delete_title").to_string();
                let detail = crate::t!(
                    "explorer.delete_message",
                    path = path.display().to_string(),
                    kind = if directory {
                        crate::t!("explorer.delete_directory")
                    } else {
                        crate::t!("explorer.delete_file")
                    }
                )
                .to_string();
                let cancel = crate::t!("explorer.cancel").into_owned();
                let confirm = crate::t!("explorer.delete_confirm").into_owned();
                let answer = window.prompt(
                    PromptLevel::Warning,
                    &title,
                    Some(&detail),
                    &[cancel.as_str(), confirm.as_str()],
                    cx,
                );
                cx.spawn_in(window, async move |view, cx| {
                    if answer.await == Ok(1) {
                        let _ = view.update_in(cx, |app, window, cx| {
                            if app.active == host && app.active_file_session == key {
                                app.explorer_run(Operation::Delete(path), window, cx);
                            }
                        });
                    }
                })
                .detach();
            }
            Command::Refresh => self.refresh_files(window, cx),
            Command::Collapse => self.file_tree.update(cx, |tree, cx| {
                let items: Vec<_> = (0..)
                    .map_while(|i| tree.entry(i))
                    .filter(|entry| entry.depth() == 0)
                    .map(|entry| entry.item().clone().expanded(false))
                    .collect();
                tree.set_items(items, cx);
            }),
            Command::Terminal => {
                self.act(
                    Action::NewWindowAt {
                        path: parent.to_string_lossy().into_owned(),
                        name: String::new(),
                    },
                    cx,
                );
                self.show_terminal(cx);
            }
            Command::Reveal => cx.reveal_path(&path),
        }
        cx.notify();
    }

    fn explorer_download(
        &mut self,
        source: PathBuf,
        directory: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = self.active_file_session.clone();
        let host = self.active;
        if directory {
            let picker = cx.prompt_for_paths(PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some(crate::t!("explorer.download").into_owned().into()),
            });
            cx.spawn_in(window, async move |view, cx| {
                let Ok(Ok(Some(paths))) = picker.await else {
                    return;
                };
                let Some(parent) = paths.into_iter().next() else {
                    return;
                };
                let _ = view.update_in(cx, |app, window, cx| {
                    if app.active != host || app.active_file_session != key {
                        return;
                    }
                    let destination = source
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| anyhow::anyhow!("Invalid directory name"))
                        .and_then(|name| file_ops::child(&parent, name, false));
                    match destination {
                        Ok(destination) => {
                            app.explorer_download_to(source, destination, window, cx)
                        }
                        Err(error) => {
                            window.push_notification(Notification::error(format!("{error:#}")), cx)
                        }
                    }
                });
            })
            .detach();
            return;
        }
        let name = source.file_name().and_then(|name| name.to_str());
        let picker = cx.prompt_for_new_path(
            &dirs::download_dir().unwrap_or_else(|| self.default_cwd.clone()),
            name,
        );
        cx.spawn_in(window, async move |view, cx| {
            let Ok(Ok(Some(destination))) = picker.await else {
                return;
            };
            let _ = view.update_in(cx, |app, window, cx| {
                if app.active != host || app.active_file_session != key {
                    return;
                }
                app.explorer_download_to(source, destination, window, cx);
            });
        })
        .detach();
    }

    fn explorer_download_to(
        &mut self,
        source: PathBuf,
        destination: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(remote) = self.remote_files() else {
            return;
        };
        if self.file_operation {
            window.push_notification(Notification::warning(crate::t!("explorer.busy")), cx);
            return;
        }
        self.file_operation = true;
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    remote_files::download(&remote, &source, &destination)?;
                    Ok::<_, anyhow::Error>(destination)
                })
                .await;
            let _ = view.update_in(cx, |app, window, cx| {
                app.file_operation = false;
                match result {
                    Ok(path) => window.push_notification(
                        Notification::success(
                            crate::t!("explorer.downloaded", path = path.display().to_string())
                                .to_string(),
                        ),
                        cx,
                    ),
                    Err(error) => {
                        window.push_notification(Notification::error(format!("{error:#}")), cx)
                    }
                }
            });
        })
        .detach();
    }

    fn explorer_pick_upload(
        &mut self,
        target: PathBuf,
        folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: !folder,
            directories: folder,
            multiple: !folder,
            prompt: Some(
                if folder {
                    crate::t!("explorer.upload_folder")
                } else {
                    crate::t!("explorer.upload_files")
                }
                .into_owned()
                .into(),
            ),
        });
        let key = self.active_file_session.clone();
        let host = self.active;
        cx.spawn_in(window, async move |view, cx| {
            let Ok(Ok(Some(paths))) = picker.await else {
                return;
            };
            let _ = view.update_in(cx, |app, window, cx| {
                if app.active == host && app.active_file_session == key {
                    app.explorer_upload(paths, target, window, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn explorer_upload(
        &mut self,
        paths: Vec<PathBuf>,
        target: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        if self.file_operation {
            window.push_notification(Notification::warning(crate::t!("explorer.busy")), cx);
            return;
        }
        let remote = self.remote_files();
        let is_remote = remote.is_some();
        let key = self.active_file_session.clone();
        let host = self.active;
        self.file_operation = true;
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(remote) = remote {
                        remote_files::upload(&remote, paths, &target)
                    } else {
                        for source in paths {
                            let name = source
                                .file_name()
                                .ok_or_else(|| anyhow::anyhow!("Source has no file name"))?;
                            let destination = target.join(name);
                            file_ops::execute(
                                None,
                                &Operation::Copy {
                                    from: source,
                                    to: destination,
                                },
                            )?;
                        }
                        Ok(())
                    }
                })
                .await;
            let _ = view.update_in(cx, |app, window, cx| {
                app.file_operation = false;
                match result {
                    Ok(()) => {
                        window.push_notification(
                            Notification::success(if is_remote {
                                crate::t!("explorer.uploaded")
                            } else {
                                crate::t!("explorer.copied")
                            }),
                            cx,
                        );
                        if app.active == host && app.active_file_session == key {
                            app.load_files(cx);
                        }
                    }
                    Err(error) => {
                        window.push_notification(Notification::error(format!("{error:#}")), cx)
                    }
                }
            });
        })
        .detach();
    }

    fn explorer_name(
        &mut self,
        command: Command,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rename = matches!(command, Command::Rename);
        let title = if rename {
            crate::t!("explorer.rename_title")
        } else if matches!(command, Command::NewFolder) {
            crate::t!("explorer.new_folder_title")
        } else {
            crate::t!("explorer.new_file_title")
        };
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::t!("ws.placeholder_name")));
        input.update(cx, |input, cx| {
            if rename {
                input.set_value(
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    window,
                    cx,
                );
            }
            input.focus(window, cx);
        });
        let owner = cx.entity().downgrade();
        let key = self.active_file_session.clone();
        let host = self.active;
        let remote = self.hosts[host].config.is_some();
        let focus_input = input.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let (owner, input, path, key) =
                (owner.clone(), input.clone(), path.clone(), key.clone());
            dialog
                .button_props(
                    gpui_kit::component::dialog::DialogButtonProps::default()
                        .ok_text(crate::t!("explorer.confirm"))
                        .cancel_text(crate::t!("explorer.cancel"))
                        .show_cancel(true),
                )
                .title(title.to_string())
                .width(px(400.))
                .child(Input::new(&input))
                .on_ok(move |_, window, cx| {
                    let name = input.read(cx).value().to_string();
                    let parent = if rename {
                        path.parent().unwrap_or(&path)
                    } else {
                        &path
                    };
                    let to = match file_ops::child(parent, &name, remote) {
                        Ok(path) => path,
                        Err(error) => {
                            window.push_notification(Notification::warning(error.to_string()), cx);
                            return false;
                        }
                    };
                    let _ = owner.update(cx, |app, cx| {
                        if app.active != host || app.active_file_session != key {
                            return;
                        }
                        let operation = if rename {
                            Operation::Rename {
                                from: path.clone(),
                                to,
                            }
                        } else {
                            Operation::Create {
                                path: to,
                                directory: matches!(command, Command::NewFolder),
                            }
                        };
                        app.explorer_run(operation, window, cx);
                    });
                    true
                })
        });
        focus_input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn explorer_run(&mut self, operation: Operation, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_operation {
            window.push_notification(Notification::warning(crate::t!("explorer.busy")), cx);
            return;
        }
        if matches!(&operation, Operation::Rename { from, to } if from == to) {
            return;
        }
        let affected = match &operation {
            Operation::Rename { from, .. } | Operation::Delete(from) => Some(from),
            _ => None,
        };
        if let Some(path) = affected {
            let dirty = self.editor_dirty
                && self
                    .open_file
                    .as_ref()
                    .is_some_and(|open| open.starts_with(path))
                || self.pending_file_state.as_ref().is_some_and(|state| {
                    state.dirty
                        && state
                            .path
                            .as_ref()
                            .is_some_and(|open| open.starts_with(path))
                })
                || self.file_states.iter().any(|((host, _), state)| {
                    *host == self.active
                        && state.dirty
                        && state
                            .path
                            .as_ref()
                            .is_some_and(|open| open.starts_with(path))
                });
            if dirty || self.file_saving {
                window
                    .push_notification(Notification::warning(crate::t!("explorer.save_first")), cx);
                return;
            }
        }
        let host_id = self.active;
        let host = self.hosts[host_id].config.clone();
        let key = self.active_file_session.clone();
        let original_host = host.clone();
        let remote = self.remote_files();
        self.file_operation = true;
        cx.spawn_in(window, async move |view, cx| {
            let task = operation.clone();
            let result = cx
                .background_executor()
                .spawn(async move { file_ops::execute(remote.as_ref(), &task) })
                .await;
            let _ = view.update_in(cx, |app, window, cx| {
                app.file_operation = false;
                if app
                    .hosts
                    .get(host_id)
                    .is_none_or(|host| host.config != original_host)
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(()) => {
                        for ((owner, _), state) in &mut app.file_states {
                            if *owner == host_id {
                                update_file(state, &operation);
                            }
                        }
                        if app.active == host_id {
                            // Cancel stale file reads before updating paths or clearing an editor.
                            app.file_request += 1;
                            app.file_loading = None;
                            let mut state = SessionFileState {
                                path: app.open_file.clone(),
                                text: app.file_editor.read(cx).value().to_string(),
                                image: app.image_preview.clone(),
                                saved_text: app.saved_file_text.clone(),
                                language: app.editor_language.clone(),
                                dirty: app.editor_dirty,
                            };
                            if update_file(&mut state, &operation) {
                                app.pending_file_state = Some(state);
                            } else if let Some(state) = &mut app.pending_file_state {
                                update_file(state, &operation);
                            }
                            if app.active_file_session == key {
                                app.load_files(cx);
                            }
                        }
                        if let Operation::Rename { from, .. } = &operation {
                            if app
                                .file_clipboard
                                .as_ref()
                                .is_some_and(|clip| clip.cut && clip.path == *from)
                            {
                                app.file_clipboard = None;
                            }
                        }
                    }
                    Err(error) => window.push_notification(
                        Notification::error(
                            crate::t!("explorer.operation_failed", error = format!("{error:#}"))
                                .to_string(),
                        ),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn update_file(state: &mut SessionFileState, operation: &Operation) -> bool {
    let Some(path) = &state.path else {
        return false;
    };
    match operation {
        Operation::Rename { from, to } if path.starts_with(from) => {
            let rest = path.strip_prefix(from).unwrap();
            state.path = Some(if rest.as_os_str().is_empty() {
                to.clone()
            } else {
                PathBuf::from(format!(
                    "{}/{}",
                    to.to_string_lossy().trim_end_matches(['/', '\\']),
                    rest.to_string_lossy().replace('\\', "/")
                ))
            });
            state.language = workbench::language_for_path(state.path.as_ref().unwrap()).to_owned();
            true
        }
        Operation::Delete(from) if path.starts_with(from) && !state.dirty => {
            *state = SessionFileState::default();
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Operation, PathBuf, SessionFileState, update_file};
    #[test]
    fn rename_tracks_open_descendants_and_delete_keeps_new_edits() {
        let mut state = SessionFileState {
            path: Some(PathBuf::from("/project/src/file.txt")),
            text: "unsaved".into(),
            dirty: true,
            ..SessionFileState::default()
        };
        assert!(update_file(
            &mut state,
            &Operation::Rename {
                from: "/project/src".into(),
                to: "/project/code".into()
            }
        ));
        assert_eq!(
            state.path.as_ref().unwrap(),
            &PathBuf::from("/project/code/file.txt")
        );
        assert!(!update_file(
            &mut state,
            &Operation::Delete("/project/code".into())
        ));
        assert_eq!(state.text, "unsaved");
        state.dirty = false;
        assert!(update_file(
            &mut state,
            &Operation::Delete("/project/code".into())
        ));
        assert!(state.path.is_none());
    }
}
