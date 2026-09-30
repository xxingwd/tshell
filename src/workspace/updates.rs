use super::*;
use crate::update::State;

impl AppView {
    pub(super) fn listen_for_updates(
        &self,
        events: async_channel::Receiver<State>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |view, cx| {
            while let Ok(state) = events.recv().await {
                if view
                    .update_in(cx, |app, window, cx| {
                        if let State::Ready(version) = &state {
                            if !matches!(&app.updater.state, State::Ready(old) if old == version) {
                                window.push_notification(
                                    Notification::info(
                                        crate::t!("updates.ready", version = version.clone())
                                            .into_owned(),
                                    ),
                                    cx,
                                );
                            }
                        }
                        app.updater.state = state;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn update_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let status = match &self.updater.state {
            State::Disabled => crate::t!("updates.disabled").into_owned(),
            State::Idle => crate::t!("updates.idle").into_owned(),
            State::Checking => crate::t!("updates.checking").into_owned(),
            State::Downloading(version) => {
                crate::t!("updates.downloading", version = version.clone()).into_owned()
            }
            State::Current => crate::t!("updates.current").into_owned(),
            State::Ready(version) => {
                crate::t!("updates.ready", version = version.clone()).into_owned()
            }
            State::Failed(error) => crate::t!("updates.failed", error = error.clone()).into_owned(),
        };
        let busy = matches!(
            self.updater.state,
            State::Checking | State::Downloading(_) | State::Disabled
        );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(16.))
                    .child(format!("TShell {}", env!("CARGO_PKG_VERSION"))),
            )
            .child(status)
            .child(
                div()
                    .text_color(rgb(self.palette.muted))
                    .child(crate::t!("updates.hint")),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("check-update")
                            .label(crate::t!("updates.check"))
                            .disabled(busy)
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.updater.check();
                                cx.notify();
                            })),
                    )
                    .when(matches!(self.updater.state, State::Ready(_)), |view| {
                        view.child(
                            Button::new("restart-update")
                                .primary()
                                .label(crate::t!("updates.restart"))
                                .on_click(cx.listener(|app, _, window, cx| {
                                    app.confirm_update_restart(window, cx)
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    pub(super) fn update_has_unsaved_files(&self) -> bool {
        self.editor_dirty
            || self.file_saving
            || self.file_operation
            || self.transfer_queue.has_pending()
            || self.file_states.values().any(|state| state.dirty)
            || self
                .pending_file_state
                .as_ref()
                .is_some_and(|state| state.dirty)
    }

    fn confirm_update_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_has_unsaved_files() {
            window.push_notification(
                Notification::warning(crate::t!("updates.unsaved").into_owned()),
                cx,
            );
            return;
        }
        let cancel = crate::t!("ws.cancel").into_owned();
        let restart = crate::t!("updates.restart").into_owned();
        let title = crate::t!("updates.confirm").into_owned();
        let receiver = window.prompt(
            PromptLevel::Warning,
            &title,
            None,
            &[cancel.as_str(), restart.as_str()],
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            if receiver.await == Ok(1) {
                let _ = view.update_in(cx, |app, window, cx| {
                    if app.update_has_unsaved_files() {
                        window.push_notification(
                            Notification::warning(crate::t!("updates.unsaved").into_owned()),
                            cx,
                        );
                        return;
                    }
                    if let Err(error) = app.save_preferences(cx) {
                        window.push_notification(
                            Notification::warning(
                                crate::t!("ws.save_failed", error = error.to_string()).into_owned(),
                            ),
                            cx,
                        );
                        return;
                    }
                    cx.restart();
                });
            }
        })
        .detach();
    }
}
