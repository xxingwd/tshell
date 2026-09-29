//! Session groups and compact tab separators, sourced from the live backend.
use super::*;
impl AppView {
    pub(super) fn edit_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.hosts[self.active]
            .snapshot
            .sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
        else {
            return;
        };
        self.home_task = None;
        self.directory_task = None;
        self.creating_tab = true;
        self.editing_session = Some(session.id);
        self.settings = false;
        self.command_palette = false;
        self.tab_name
            .update(cx, |input, cx| input.set_value(session.name, window, cx));
        self.tab_path
            .update(cx, |input, cx| input.set_value(session.cwd, window, cx));
        self.open_modal(crate::t!("session.edit"), window, cx);
        self.tab_name
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub(super) fn session_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                Field::new()
                    .label(crate::t!("ws.field_name").to_string())
                    .child(Input::new(&self.tab_name)),
            )
            .child(
                Field::new()
                    .label(crate::t!("session.directory_readonly").to_string())
                    .child(
                        div()
                            .text_size(px(12.))
                            .child(self.tab_path.read(cx).value()),
                    ),
            )
            .child(
                div().flex().justify_end().child(
                    Button::new("save-session")
                        .label(crate::t!("ws.save"))
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| this.create_tab(window, cx))),
                ),
            )
            .into_any_element()
    }

    pub(super) fn toggle_session(&mut self, id: String, cx: &mut Context<Self>) {
        let collapsed = &mut self.hosts[self.active].collapsed_sessions;
        if !collapsed.remove(&id) {
            collapsed.insert(id);
        }
        cx.notify();
    }
    pub(super) fn select_terminal(
        &mut self,
        session: String,
        tab: String,
        pane: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.act(Action::SelectSession(session), cx);
        self.act(Action::SelectWindow(tab), cx);
        if let Some(pane) = pane {
            self.mark_terminal_read(&pane, cx);
            self.act(Action::SelectPane(pane), cx);
        }
        self.show_terminal(cx);
    }
    pub(super) fn session_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let host = &self.hosts[self.active];
        let mut rows = Vec::new();
        for (group_index, session) in host.snapshot.sessions.iter().enumerate() {
            let session_id = session.id.as_str();
            let windows = &session.windows;
            let active_window = session.active_window.as_str();
            let id = session_id.to_owned();
            let create_id = id.clone();
            let collapsed = host.collapsed_sessions.contains(&id);
            let unread = windows
                .iter()
                .flat_map(|w| &w.panes)
                .any(|p| p.notice_count > host.read_notices.get(&p.id).copied().unwrap_or(0));
            let edit_id = id.clone();
            let delete_id = id.clone();
            let host_index = self.active;
            let owner = cx.entity().downgrade();
            rows.push(
                div()
                    .id(SharedString::from(format!("session-{id}")))
                    .group("session-row")
                    .h(px(28.))
                    .flex_shrink_0()
                    .mt(px(if group_index == 0 { 4. } else { 10. }))
                    .mb(px(3.))
                    .mx(px(2.))
                    .rounded_sm()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(p.row_hover())))
                    .text_size(px(13.))
                    .line_height(relative(1.25))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(p.text))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(session.name.clone()),
                    )
                    .when(collapsed && unread, |row| {
                        row.child(
                            div()
                                .size(px(5.))
                                .flex_none()
                                .rounded_full()
                                .bg(rgb(p.accent)),
                        )
                    })
                    .child(
                        div()
                            .size(px(22.))
                            .flex_shrink_0()
                            .invisible()
                            .group_hover("session-row", |style| style.visible())
                            .child(
                                icon_button(
                                    SharedString::from(format!("new-tab-{id}")),
                                    IconName::Plus,
                                    crate::t!("session.new_tab"),
                                )
                                .h(px(22.))
                                .w(px(22.))
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.hosts[this.active]
                                            .collapsed_sessions
                                            .remove(&create_id);
                                        this.act(Action::SelectSession(create_id.clone()), cx);
                                        this.show_new_tab(window, cx);
                                    },
                                )),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_session(id.clone(), cx);
                    }))
                    .context_menu(move |menu, _, _| {
                        let edit_owner = owner.clone();
                        let delete_owner = owner.clone();
                        let id = edit_id.clone();
                        let delete_id = delete_id.clone();
                        menu.item(PopupMenuItem::new(crate::t!("session.edit_menu")).on_click(
                            move |_, window, cx| {
                                let _ = edit_owner
                                    .update(cx, |this, cx| this.edit_session(&id, window, cx));
                            },
                        ))
                        .item(
                            PopupMenuItem::new(crate::t!("session.delete_menu")).on_click(
                                move |_, _, cx| {
                                    let _ = delete_owner.update(cx, |this, cx| {
                                        if this.active == host_index {
                                            this.act(Action::RemoveSession(delete_id.clone()), cx);
                                        }
                                    });
                                },
                            ),
                        )
                    })
                    .into_any_element(),
            );
            if collapsed {
                continue;
            }
            for (index, tab) in windows.iter().enumerate() {
                if index > 0 {
                    rows.push(
                        div()
                            .mx_2()
                            .my_1()
                            .h(px(1.))
                            .flex_shrink_0()
                            .bg(rgba((p.muted << 8) | 35))
                            .into_any_element(),
                    );
                }
                for pane in &tab.panes {
                    let session_id = session_id.to_owned();
                    let tab_id = tab.id.clone();
                    let pane_id = pane.id.clone();
                    let selected = session_id == host.snapshot.active_session
                        && tab.id == active_window
                        && pane.id == tab.active_pane;
                    let label = if pane.title.is_empty() {
                        crate::t!("session.terminal").to_string()
                    } else {
                        pane.title.clone()
                    };
                    rows.push(
                        div()
                            .id(SharedString::from(format!(
                                "panel-{}-{}",
                                session_id, pane.id
                            )))
                            .h(px(28.))
                            .flex_shrink_0()
                            .my(px(2.))
                            .mr(px(2.))
                            .px_2()
                            .ml_1()
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .when(selected, |row| row.bg(rgb(p.row_selected())))
                            .text_color(rgb(p.text))
                            .hover(move |s| {
                                s.bg(rgb(if selected {
                                    p.row_selected()
                                } else {
                                    p.row_hover()
                                }))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .line_height(relative(1.25))
                                    .child(label),
                            )
                            .when(
                                pane.notice_count
                                    > host.read_notices.get(&pane.id).copied().unwrap_or(0),
                                |row| {
                                    row.child(
                                        div()
                                            .size(px(5.))
                                            .flex_none()
                                            .rounded_full()
                                            .bg(rgb(p.accent)),
                                    )
                                },
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_terminal(
                                    session_id.clone(),
                                    tab_id.clone(),
                                    Some(pane_id.clone()),
                                    cx,
                                );
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        div()
            .id("session-list")
            .size_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .px_1()
            .children(rows)
            .into_any_element()
    }
}

pub(super) fn session_name(name: &str, path: &str) -> String {
    if !name.is_empty() {
        return name.to_owned();
    }
    let name = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    let name = name.replace([':', '.'], "_");
    if name.is_empty() { "root".into() } else { name }
}
#[cfg(test)]
mod tests {
    #[test]
    fn session_name_defaults_to_directory() {
        assert_eq!(super::session_name("", "/home/dev/project/"), "project");
        assert_eq!(super::session_name("", "/"), "root");
        assert_eq!(super::session_name("build", "/tmp"), "build");
    }
}
