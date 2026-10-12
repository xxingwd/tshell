use super::*;
impl Explorer {
    pub fn file_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let owner = cx.entity().downgrade();
        let active_file = self.open_file.clone();
        let directories = self.tree_loaded.clone();
        // One menu owner for both rows and blank space; rows only select the target.
        let menu_target = std::rc::Rc::new(std::cell::RefCell::new(None::<(PathBuf, bool)>));
        let row_target = menu_target.clone();
        let tree = Tree::new(&self.file_tree, move |index, entry, _, _, _| {
            let empty_folder = !entry.is_folder() && directories.contains(entry.item().id.as_ref());
            let is_folder = entry.is_folder() || empty_folder;
            let folder_item = entry.item().clone();
            let folder_owner = owner.clone();
            let path = PathBuf::from(entry.item().id.as_ref());
            let is_active = active_file.as_ref().is_some_and(|active| active == &path);
            let is_code = !is_folder && workbench::language_for_path(&path) != "plaintext";
            let open_owner = owner.clone();
            let disclosure = if is_folder {
                Icon::new(if entry.is_expanded() {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(rgb(p.muted))
                .into_any_element()
            } else {
                div().w(px(12.)).flex_shrink_0().into_any_element()
            };
            let file_icon = Icon::new(if is_folder {
                if entry.is_expanded() {
                    IconName::FolderOpen
                } else {
                    IconName::Folder
                }
            } else if is_code {
                IconName::FileCode
            } else {
                IconName::File
            })
            .xsmall()
            .flex_shrink_0()
            .text_color(rgb(if is_folder || is_active {
                p.accent
            } else {
                p.muted
            }));
            let row = div()
                .w_full()
                .h_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(3.))
                .overflow_hidden()
                .pl(px(entry.depth() as f32 * 12.))
                .child(disclosure)
                .child(file_icon)
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .when(is_active, |label| {
                            label
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(p.text))
                        })
                        .child(entry.item().label.clone()),
                );
            let target = row_target.clone();
            let target_path = path.clone();
            let enabled = !entry.is_disabled();
            let drop_owner = owner.clone();
            let drop_target = path.clone();
            ListItem::new(SharedString::from(format!("file-entry-{index}")))
                .h(px(style::ROW_HEIGHT))
                .min_h(px(style::ROW_HEIGHT))
                .px_1()
                .py_0()
                .rounded_sm()
                .overflow_hidden()
                .text_size(px(13.))
                .line_height(relative(1.35))
                .on_mouse_down(MouseButton::Right, move |_, _, _| {
                    *target.borrow_mut() = enabled.then(|| (target_path.clone(), is_folder));
                })
                .child(row)
                .when(is_folder, |item| {
                    item.on_drop(move |paths: &ExternalPaths, window, cx| {
                        let _ = drop_owner.update(cx, |app, cx| {
                            app.explorer_upload(
                                paths.paths().to_vec(),
                                drop_target.clone(),
                                window,
                                cx,
                            )
                        });
                    })
                })
                .when(empty_folder, |item| {
                    item.on_click(move |_, _, cx| {
                        let expanded = folder_item.is_expanded();
                        folder_item.clone().expanded(!expanded);
                        let _ = folder_owner.update(cx, |_, cx| cx.notify());
                    })
                })
                .when(!is_folder && !entry.is_disabled(), |item| {
                    item.on_click(move |_, window, cx| {
                        let _ = open_owner
                            .update(cx, |app, cx| app.open_path(path.clone(), window, cx));
                    })
                })
        })
        .size_full();
        div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .pt_1()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .px_1()
                    .child(tree),
            )
            .id("explorer-background")
            .on_drop({
                let owner = cx.entity().downgrade();
                let root = self.cwd.clone();
                move |paths: &ExternalPaths, window, cx| {
                    let _ = owner.update(cx, |app, cx| {
                        app.explorer_upload(paths.paths().to_vec(), root.clone(), window, cx)
                    });
                }
            })
            .context_menu({
                let owner = cx.entity().downgrade();
                move |menu, _, cx| {
                    let Some(app) = owner.upgrade() else {
                        return menu;
                    };
                    let app = app.read(cx);
                    let target = menu_target.borrow_mut().take();
                    let root = target.is_none();
                    let (path, directory) = target.unwrap_or_else(|| (app.cwd.clone(), true));
                    app.explorer_menu(menu, path, directory, root, owner.clone())
                }
            })
            .into_any_element()
    }
    fn file_feedback(&self, content: AnyElement) -> AnyElement {
        div()
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(content)
            .when_some(
                self.file_loading
                    .clone()
                    .filter(|_| self.open_file.is_some() && self.message.is_none()),
                |view, message| {
                    view.child(style::content_notice(
                        "explorer-loading",
                        IconName::Clock,
                        message,
                        self.palette.muted,
                        self.palette,
                    ))
                },
            )
            .when_some(
                self.message.clone().filter(|_| self.open_file.is_some()),
                |view, message| {
                    view.child(style::content_notice(
                        "explorer-error",
                        IconName::CircleAlert,
                        message,
                        self.palette.error,
                        self.palette,
                    ))
                },
            )
            .into_any_element()
    }

    pub fn file_surface(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(path) = &self.open_file {
            let mut parts = Vec::new();
            parts.push(
                self.cwd
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| crate::t!("ws.workspace").to_string()),
            );
            let relative_path = path.strip_prefix(&self.cwd).unwrap_or(path);
            parts.extend(
                relative_path
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned()),
            );
            let previewable = self.image_preview.is_none()
                && preview::can_preview(
                    &self.editor_language,
                    self.file_editor.read(cx).value().as_ref(),
                );
            let owner = cx.entity().downgrade();
            let file_mode = if previewable {
                let edit_owner = owner.clone();
                let preview_owner = owner;
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        button("file-edit", IconName::Pencil, crate::t!("file.edit"))
                            .when(!self.preview_mode, |button| {
                                button.bg(rgba((self.palette.accent << 8) | 42))
                            })
                            .on_click(move |_, _, cx| {
                                let _ = edit_owner.update(cx, |this, cx| {
                                    this.preview_mode = false;
                                    this.file_preview = None;
                                    this.need_focus = true;
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        button("file-preview", IconName::Eye, crate::t!("file.preview"))
                            .when(self.preview_mode, |button| {
                                button.bg(rgba((self.palette.accent << 8) | 42))
                            })
                            .on_click(move |_, _, cx| {
                                let _ = preview_owner.update(cx, |this, cx| {
                                    let text = this.file_editor.read(cx).value().to_string();
                                    this.file_preview =
                                        Some(preview::parse(&this.editor_language, &text));
                                    this.preview_mode = true;
                                    this.need_focus = false;
                                    cx.notify();
                                });
                            }),
                    )
                    .into_any_element()
            } else {
                div().into_any_element()
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(style::TOOLBAR_HEIGHT))
                        .flex_shrink_0()
                        .px_3()
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .border_b_1()
                        .border_color(rgb(self.palette.border))
                        .text_size(px(12.))
                        .line_height(relative(1.2))
                        .child(
                            div().min_w_0().flex_1().overflow_hidden().child(
                                Breadcrumb::new()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_size(px(11.))
                                    .children(parts.into_iter().map(|part| {
                                        BreadcrumbItem::new(part)
                                            .min_w_0()
                                            .truncate()
                                            .text_color(rgb(self.terminal_palette.text))
                                    })),
                            ),
                        )
                        .child(file_mode),
                )
                .children(
                    (!self.preview_mode)
                        .then(|| self.editor_search_bar(cx))
                        .flatten(),
                )
                .child(div().flex_1().min_h_0().child(self.file_feedback(if let Some(image) = &self.image_preview {
                    div()
                        .flex_1()
                        .min_h_0()
                        .p_3()
                        .child(
                            img(image.clone())
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        )
                        .into_any_element()
                } else if self.preview_mode {
                    if self.editor_language == "markdown" {
                        self.file_preview
                            .as_ref()
                            .and_then(|document| preview::render_markdown(document, self.palette))
                            .unwrap_or_else(|| div().size_full().into_any_element())
                    } else {
                        let fallback = self.file_preview
                            .as_ref()
                            .map(|document| preview::render(document, self.palette, &self.font_family))
                            .unwrap_or_else(|| div().size_full().into_any_element());
                        #[cfg(windows)]
                        {
                            let owner = cx.entity().downgrade();
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .child(fallback)
                                .child(
                                    canvas(
                                        move |bounds, window, cx| {
                                            window.defer(cx, move |window, cx| {
                                                let _ = owner.update(cx, |this, cx| {
                                                    if !this.visible
                                                        || this.editor_language != "html"
                                                        || !this.preview_mode
                                                        || window.has_active_dialog(cx)
                                                    {
                                                        return;
                                                    }
                                                    let Some(html) = this.file_preview.as_ref().and_then(|document| document.web_html()) else {
                                                        return;
                                                    };
                                                    let result = if let Some(web) = &mut this.web_preview {
                                                        web.show(html, bounds)
                                                    } else if this.web_preview_failed {
                                                        return;
                                                    } else {
                                                        match preview::WebPreview::new(html, bounds, window) {
                                                            Ok(web) => {
                                                                this.web_preview = Some(web);
                                                                Ok(())
                                                            }
                                                            Err(error) => Err(error),
                                                        }
                                                    };
                                                    if let Err(error) = result {
                                                        tracing::warn!(%error, "WebView preview unavailable; using native fallback");
                                                        this.web_preview = None;
                                                        this.web_preview_failed = true;
                                                    }
                                                });
                                            });
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .size_full(),
                                )
                                .into_any_element()
                        }
                        #[cfg(not(windows))]
                        {
                            fallback
                        }
                    }
                } else {
                    div()
                        .flex_1()
                        .min_h_0()
                        .font_family(self.font_family.clone())
                        .text_size(px(self.font_size.clamp(12., 17.)))
                        .line_height(relative(1.5))
                        .child(gpui_kit::base::input::Editor::new(&self.file_editor))
                        .into_any_element()
                })))
                .into_any_element()
        } else {
            if let Some(message) = &self.message {
                return style::empty_state(IconName::CircleAlert, message.clone(), self.palette)
                    .text_color(rgb(self.palette.error))
                    .into_any_element();
            }
            style::empty_state(
                if self.file_loading.is_some() {
                    IconName::Clock
                } else {
                    IconName::FileCode
                },
                self.file_loading
                    .clone()
                    .unwrap_or_else(|| crate::t!("ws.pick_file").to_string()),
                self.palette,
            )
            .into_any_element()
        }
    }

    pub fn prepare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_editor.update(cx, |editor, _| {
            editor.set_editor_style(editor_theme::style(self.terminal_palette))
        });
        if let Some(paths) = self.pending_file_paths.take() {
            self.file_picker.update(cx, |state, cx| {
                state.set_items(SearchableVec::new(paths), window, cx)
            });
        }
        if let Some(state) = self.pending_file_state.take() {
            self.saved_file_text = state.saved_text;
            self.open_file = None;
            self.image_preview = state.image;
            self.file_preview = state.preview.clone();
            self.preview_mode = state.preview_mode;
            self.file_editor.update(cx, |editor, cx| {
                editor.set_value(state.text, window, cx);
                editor.set_highlighter(
                    if state.language.is_empty() {
                        "plaintext"
                    } else {
                        &state.language
                    },
                    cx,
                );
            });
            self.open_file = state.path;
            self.editor_dirty = state.dirty;
            self.editor_language = if state.language.is_empty() {
                "plaintext".into()
            } else {
                state.language
            };
        }
        if let Some((path, position)) = self.file_link_position.take()
            && self.open_file.as_ref() == Some(&path)
            && self.image_preview.is_none()
        {
            self.preview_mode = false;
            self.file_editor.update(cx, |editor, cx| {
                editor.set_cursor_position(
                    gpui_kit::component::input::Position::new(position.0, position.1),
                    window,
                    cx,
                )
            });
        }
        #[cfg(windows)]
        if !self.visible
            || !self.preview_mode
            || self.editor_language != "html"
            || self.file_preview.is_none()
            || window.has_active_dialog(cx)
        {
            if let Some(web) = &mut self.web_preview {
                web.hide();
            }
        }
        self.sync_editor_search(window, cx);
    }
}
impl Render for Explorer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare(window, cx);
        self.file_surface(cx)
    }
}
