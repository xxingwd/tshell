use super::*;

impl AppView {
    pub(super) fn subscribe_editor_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.observe(&self.file_editor, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe_in(&self.editor_search, window, |this, _, event, _, cx| {
            let query = this.editor_search.read(cx).value().to_string();
            this.file_editor.update(cx, |editor, cx| match event {
                InputEvent::Change => editor.set_search_query(query, true, cx),
                InputEvent::PressEnter { .. } => {
                    editor.next_search_match(cx);
                }
                _ => {}
            });
        })
        .detach();
        self.file_editor.update(cx, |editor, _| {
            editor.on_context_menu(std::rc::Rc::new(|_, capabilities, position, window, cx| {
                use gpui_kit::component::{input, native_menu::NativeMenu};
                NativeMenu::new()
                    .menu_with_disabled(
                        crate::t!("editor.cut"),
                        !capabilities.is_copyable(),
                        Box::new(input::Cut),
                    )
                    .menu_with_disabled(
                        crate::t!("editor.copy"),
                        !capabilities.is_copyable(),
                        Box::new(input::Copy),
                    )
                    .menu(crate::t!("editor.paste"), Box::new(input::Paste))
                    .separator()
                    .menu(crate::t!("editor.select_all"), Box::new(input::SelectAll))
                    .show(position, window, cx);
            }));
        });
    }

    pub(super) fn sync_editor_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.file_editor.read(cx);
        if self.workspace_mode != WorkspaceMode::Files
            || !editor.search_session().open
            || self.editor_search_revision == editor.search_activation_revision()
        {
            return;
        }
        self.editor_search_revision = editor.search_activation_revision();
        let query = editor.search_session().query.clone();
        self.editor_search.update(cx, |input, cx| {
            input.set_value(query, window, cx);
            input.focus(window, cx);
        });
    }

    pub(super) fn editor_search_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.file_editor.read(cx).search_session();
        if !search.open {
            return None;
        }
        let replace = search.replace_mode;
        let count = search.matcher.len();
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(self.palette.border))
                .flex_shrink_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&self.editor_search).small()),
                        )
                        .child(
                            div()
                                .w(px(64.))
                                .flex_shrink_0()
                                .text_size(px(12.))
                                .text_color(rgb(self.palette.muted))
                                .child(crate::t!("editor.item_count", count = count)),
                        )
                        .child(
                            icon_button(
                                "editor-previous",
                                IconName::ChevronUp,
                                crate::t!("editor.previous_match"),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.file_editor.update(cx, |editor, cx| {
                                    editor.previous_search_match(cx);
                                });
                            })),
                        )
                        .child(
                            icon_button(
                                "editor-next",
                                IconName::ChevronDown,
                                crate::t!("editor.next_match"),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.file_editor.update(cx, |editor, cx| {
                                    editor.next_search_match(cx);
                                });
                            })),
                        )
                        .child(
                            icon_button(
                                "editor-replace-toggle",
                                IconName::Replace,
                                crate::t!("editor.replace"),
                            )
                            .selected(replace)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.file_editor.update(cx, |editor, cx| {
                                        editor.set_search_replace_mode(!replace, cx)
                                    });
                                },
                            )),
                        )
                        .child(
                            icon_button(
                                "editor-search-close",
                                IconName::X,
                                crate::t!("editor.close_search"),
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.file_editor.update(cx, |editor, cx| {
                                        editor.close_search(cx);
                                        editor.focus(window, cx);
                                    });
                                },
                            )),
                        ),
                )
                .when(replace, |bar| {
                    bar.child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.editor_replace).small()),
                            )
                            .child(
                                Button::new("editor-replace-one")
                                    .ghost()
                                    .small()
                                    .label(crate::t!("editor.replace"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let text = this.editor_replace.read(cx).value().to_string();
                                        this.file_editor.update(cx, |editor, cx| {
                                            editor.replace_current_search_match(&text, window, cx);
                                        });
                                    })),
                            )
                            .child(
                                Button::new("editor-replace-all")
                                    .ghost()
                                    .small()
                                    .label(crate::t!("editor.replace_all"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let text = this.editor_replace.read(cx).value().to_string();
                                        this.file_editor.update(cx, |editor, cx| {
                                            editor.replace_all_search_matches(&text, window, cx);
                                        });
                                    })),
                            ),
                    )
                })
                .into_any_element(),
        )
    }
}
