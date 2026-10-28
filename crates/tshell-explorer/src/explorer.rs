use super::*;
use file_ops::Operation;
use gpui_kit::component::menu::PopupMenu;

#[derive(Clone)]
pub struct FileClipboard {
    host: Option<SshConfig>,
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

impl Explorer {
    pub fn explorer_menu(
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
        if !root && self.host.is_some() {
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
            if self.host.is_some() {
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
                .borrow()
                .as_ref()
                .is_some_and(|clip| clip.host == self.host)
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
        if self.host.is_none() {
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

        for (label, command) in items {
            if matches!(command, Command::Path | Command::Refresh | Command::Delete) {
                menu = menu.separator();
            }
            let (owner, path) = (owner.clone(), path.clone());
            menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let _ = owner.update(cx, |app, cx| {
                    app.explorer_command(command, path.clone(), directory, window, cx);
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
            tshell_host::HostPath::for_host(&path, self.host.is_some())
                .parent()
                .map(|path| path.as_path_buf())
                .unwrap_or_else(|| self.cwd.clone())
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
                *self.file_clipboard.borrow_mut() = Some(FileClipboard {
                    host: self.host.clone(),
                    path,
                    cut: matches!(command, Command::Cut),
                });
            }
            Command::Path | Command::Relative => {
                let path = tshell_host::HostPath::for_host(&path, self.host.is_some());
                let text = if matches!(command, Command::Relative) {
                    path.relative_to(&tshell_host::HostPath::for_host(
                        &self.cwd,
                        self.host.is_some(),
                    ))
                    .unwrap_or_else(|| path.as_path_buf().to_string_lossy().into_owned())
                } else {
                    path.as_path_buf().to_string_lossy().into_owned()
                };
                let text = if text.is_empty() { ".".into() } else { text };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Command::Paste => {
                let Some(clip) = self.file_clipboard.borrow().clone() else {
                    return;
                };
                let source = tshell_host::HostPath::for_host(&clip.path, self.host.is_some());
                let name = source.name();
                if name.is_empty() {
                    return;
                }
                let to = match file_ops::child(&parent, &name, clip.host.is_some()) {
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
                            app.explorer_run(Operation::Delete(path), window, cx);
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
            Command::Terminal => cx.emit(Event::OpenTerminal(parent)),
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
        let download_directory = dirs::download_dir()
            .or_else(|| {
                dirs::home_dir()
                    .map(|home| home.join("Downloads"))
                    .filter(|path| path.is_dir())
            })
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| self.default_cwd.clone());
        let name = source.file_name().and_then(|name| name.to_str());
        if directory {
            let destination = name
                .ok_or_else(|| anyhow::anyhow!("{}", crate::t!("explorer.invalid_directory_name")))
                .and_then(|name| file_ops::child(&download_directory, name, false));
            if let Err(error) = destination {
                window
                    .push_notification(Notification::error(crate::i18n::format_error(&error)), cx);
                return;
            }
        }
        let picker = cx.prompt_for_new_path(&download_directory, name);
        cx.spawn_in(window, async move |view, cx| {
            let Ok(Ok(Some(destination))) = picker.await else {
                return;
            };
            let _ = view.update_in(cx, |app, window, cx| {
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
        self.queue_download(remote, source, destination, window, cx);
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

        cx.spawn_in(window, async move |view, cx| {
            let Ok(Ok(Some(paths))) = picker.await else {
                return;
            };
            let _ = view.update_in(cx, |app, window, cx| {
                app.explorer_upload(paths, target, window, cx);
            });
        })
        .detach();
    }

    pub fn explorer_upload(
        &mut self,
        paths: Vec<PathBuf>,
        target: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        if let Some(remote) = self.remote_files() {
            self.queue_upload(remote, paths, target, window, cx);
            return;
        }
        if self.file_operation {
            window.push_notification(Notification::warning(crate::t!("explorer.busy")), cx);
            return;
        }

        self.file_operation = true;
        cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    for source in paths {
                        let name = source.file_name().ok_or_else(|| {
                            anyhow::anyhow!("{}", crate::t!("explorer.source_name_missing"))
                        })?;
                        let destination = target.join(name);
                        file_ops::execute(
                            None,
                            &Operation::Copy {
                                from: source,
                                to: destination,
                            },
                        )?;
                    }
                    Ok::<(), anyhow::Error>(())
                })
                .await;
            let _ = view.update_in(cx, |app, window, cx| {
                app.file_operation = false;
                match result {
                    Ok(()) => {
                        window.push_notification(
                            Notification::success(crate::t!("explorer.copied")),
                            cx,
                        );
                        app.load_files(cx);
                    }
                    Err(error) => window.push_notification(
                        Notification::error(crate::i18n::format_error(&error)),
                        cx,
                    ),
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
                    tshell_host::HostPath::for_host(&path, self.host.is_some()).name(),
                    window,
                    cx,
                );
            }
            input.focus(window, cx);
        });
        let owner = cx.entity().downgrade();

        let remote = self.host.is_some();
        let focus_input = input.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (owner, input, path) = (owner.clone(), input.clone(), path.clone());
            workspace_dialog(dialog, cx)
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
                    let path_on_host = tshell_host::HostPath::for_host(&path, remote);
                    let parent = if rename {
                        path_on_host.parent().unwrap_or(path_on_host)
                    } else {
                        path_on_host
                    };
                    let to = match file_ops::child(&parent.as_path_buf(), &name, remote) {
                        Ok(path) => path,
                        Err(error) => {
                            window.push_notification(
                                Notification::warning(crate::i18n::format_error(&error)),
                                cx,
                            );
                            return false;
                        }
                    };
                    let _ = owner.update(cx, |app, cx| {
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

    fn explorer_run(&mut self, operation: Operation, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(Event::RequestOperation(operation));
    }
    pub fn perform_operation(
        &mut self,
        operation: Operation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_operation {
            window.push_notification(Notification::warning(crate::t!("explorer.busy")), cx);
            return;
        }
        if matches!(&operation,Operation::Rename{from,to} if from==to) {
            return;
        }
        let remote = self.remote_files();
        self.file_operation = true;
        cx.spawn_in(window, async move |view, cx| {
            let task = operation.clone();
            let result = cx
                .background_executor()
                .spawn(async move { file_ops::execute(remote.as_ref(), &task) })
                .await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.file_operation = false;
                match result {
                    Ok(()) => {
                        this.files_changed(&operation, cx);
                        cx.emit(Event::FilesChanged(operation));
                    }
                    Err(error) => window.push_notification(
                        Notification::error(crate::i18n::format_error(&error)),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub fn files_changed(&mut self, operation: &Operation, cx: &mut Context<Self>) {
        self.file_request += 1;
        self.file_loading = None;
        let pending = self.pending_file_state.take();
        let was_pending = pending.is_some();
        let mut state = pending.unwrap_or_else(|| SessionFileState {
            path: self.open_file.clone(),
            text: self.file_editor.read(cx).value().to_string(),
            image: self.image_preview.clone(),
            pdf: self.pdf_preview.clone(),
            preview: self.file_preview.clone(),
            preview_mode: self.preview_mode,
            saved_text: self.saved_file_text.clone(),
            language: self.editor_language.clone(),
            dirty: self.editor_dirty,
        });
        if update_file(
            &mut state,
            operation,
            self.host.is_some() && !self.document_local,
        ) || was_pending
        {
            self.pending_file_state = Some(state);
        }
        if let Operation::Rename { from, .. } = operation {
            if self
                .file_clipboard
                .borrow()
                .as_ref()
                .is_some_and(|clip| clip.cut && clip.path == *from)
            {
                *self.file_clipboard.borrow_mut() = None;
            }
        }
        self.load_files(cx);
        cx.notify();
    }
}

fn update_file(state: &mut SessionFileState, operation: &Operation, remote: bool) -> bool {
    let Some(path) = &state.path else {
        return false;
    };
    let path = tshell_host::HostPath::for_host(path, remote);
    match operation {
        Operation::Rename { from, to } => {
            let Some(rest) = path.relative_to(&tshell_host::HostPath::for_host(from, remote))
            else {
                return false;
            };
            let destination = tshell_host::HostPath::for_host(to, remote);
            let destination = if rest.is_empty() {
                destination
            } else {
                destination.join(&rest)
            };
            state.path = Some(destination.as_path_buf());
            state.language = workbench::language_for_path(&destination.as_path_buf()).to_owned();
            state.pdf = None;
            state.preview_mode = preview::can_preview(&state.language, &state.text);
            state.preview = state
                .preview_mode
                .then(|| preview::parse(&state.language, &state.text));
            true
        }
        Operation::Delete(from)
            if path.is_within(&tshell_host::HostPath::for_host(from, remote)) && !state.dirty =>
        {
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
    fn remote_rename_keeps_literal_backslashes_and_component_boundaries() {
        let mut state = SessionFileState {
            path: Some(r"/srv/name\file.rs".into()),
            ..Default::default()
        };
        assert!(!update_file(
            &mut state,
            &Operation::Delete("/srv/name".into()),
            true
        ));
        assert!(update_file(
            &mut state,
            &Operation::Rename {
                from: "/srv".into(),
                to: "/new".into()
            },
            true
        ));
        assert_eq!(state.path.unwrap().to_string_lossy(), r"/new/name\file.rs");
    }
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
            },
            true
        ));
        assert_eq!(
            state.path.as_ref().unwrap(),
            &PathBuf::from("/project/code/file.txt")
        );
        assert!(!update_file(
            &mut state,
            &Operation::Delete("/project/code".into()),
            true
        ));
        assert_eq!(state.text, "unsaved");
        state.dirty = false;
        assert!(update_file(
            &mut state,
            &Operation::Delete("/project/code".into()),
            true
        ));
        assert!(state.path.is_none());
    }
}
