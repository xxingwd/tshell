use super::*;
use crate::ssh_pool::interactive::{Prompt, PromptKind};

struct AuthDialog {
    host: String,
    kind: PromptKind,
    input: Option<Entity<InputState>>,
    reply: async_channel::Sender<Option<String>>,
    finish: async_channel::Sender<()>,
}

impl AuthDialog {
    fn answer(&mut self, accepted: bool, window: &mut Window, cx: &mut Context<Self>) {
        let value = accepted.then(|| {
            self.input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_else(|| "yes".into())
        });
        let _ = self.reply.try_send(value);
        let _ = self.finish.try_send(());
        window.close_dialog(cx);
    }
}

impl Render for AuthDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match &self.kind {
            PromptKind::Secret { label, .. } => label.clone(),
            PromptKind::HostKey { .. } => crate::t!("ssh.host_key_confirm").into_owned(),
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .text_size(px(13.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::Server).size(px(16.)))
                    .child(
                        div()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.host.clone()),
                    ),
            )
            .child(div().child(title))
            .when(matches!(self.kind, PromptKind::HostKey { .. }), |view| {
                let PromptKind::HostKey { fingerprint } = &self.kind else {
                    return view;
                };
                view.child(
                    div()
                        .min_w_0()
                        .font_family("Consolas")
                        .text_size(px(12.))
                        .child(fingerprint.clone()),
                )
            })
            .when_some(self.input.as_ref(), |view, input| {
                view.child(Input::new(input))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("auth-cancel")
                            .small()
                            .h(px(style::CONTROL_HEIGHT))
                            .label(crate::t!("ws.cancel"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.answer(false, window, cx)),
                            ),
                    )
                    .child(
                        Button::new("auth-accept")
                            .small()
                            .h(px(style::CONTROL_HEIGHT))
                            .primary()
                            .label(match &self.kind {
                                PromptKind::Secret { .. } => crate::t!("ssh.authenticate"),
                                PromptKind::HostKey { .. } => crate::t!("ssh.trust_host"),
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.answer(true, window, cx)),
                            ),
                    ),
            )
    }
}

impl AppView {
    pub(super) fn open_auth_prompt(
        &mut self,
        prompt: Prompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reply = prompt.reply.clone();
        let finish = prompt.finish.clone();
        let dialog = cx.new(|cx| AuthDialog {
            host: prompt.host,
            input: match &prompt.kind {
                PromptKind::Secret { echo, .. } => {
                    Some(cx.new(|cx| InputState::new(window, cx).masked(!*echo)))
                }
                PromptKind::HostKey { .. } => None,
            },
            kind: prompt.kind,
            reply: prompt.reply,
            finish: prompt.finish,
        });
        let input = dialog.read(cx).input.clone();
        window.open_dialog(cx, move |builder, _, cx| {
            let reply = reply.clone();
            let finish = finish.clone();
            let submit = dialog.clone();
            workspace_dialog(builder, cx)
                .title(crate::t!("ssh.authentication"))
                .width(px(440.))
                .on_ok(move |_, window, cx| {
                    submit.update(cx, |dialog, cx| dialog.answer(true, window, cx));
                    false
                })
                .on_close(move |_, _, _| {
                    let _ = reply.try_send(None);
                    let _ = finish.try_send(());
                })
                .child(dialog.clone())
        });
        if let Some(input) = input {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }
}
