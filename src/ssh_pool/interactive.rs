use anyhow::{Context, Result};
use async_channel::{Receiver, Sender};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

type Credentials = Arc<tokio::sync::Mutex<Option<String>>>;

pub(crate) fn credentials(host: &str) -> Credentials {
    static CREDENTIALS: OnceLock<parking_lot::Mutex<BTreeMap<String, Credentials>>> =
        OnceLock::new();
    CREDENTIALS
        .get_or_init(Default::default)
        .lock()
        .entry(host.to_owned())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
        .clone()
}

pub(crate) enum PromptKind {
    Secret { label: String, echo: bool },
    HostKey { fingerprint: String },
}

pub(crate) struct Prompt {
    pub host: String,
    pub kind: PromptKind,
    pub reply: Sender<Option<String>>,
    pub finish: Sender<()>,
    pub finished: Receiver<()>,
}

fn channel() -> &'static parking_lot::Mutex<Option<Sender<Prompt>>> {
    static CHANNEL: OnceLock<parking_lot::Mutex<Option<Sender<Prompt>>>> = OnceLock::new();
    CHANNEL.get_or_init(Default::default)
}

pub(crate) fn subscribe() -> Receiver<Prompt> {
    let (sender, receiver) = async_channel::unbounded();
    *channel().lock() = Some(sender);
    receiver
}

pub(crate) async fn ask(host: &str, kind: PromptKind) -> Result<Option<String>> {
    let (reply, answer) = async_channel::bounded(1);
    let (finish, finished) = async_channel::bounded(1);
    let sender = channel()
        .lock()
        .clone()
        .context(crate::t!("ssh.prompt_unavailable"))?;
    sender
        .send(Prompt {
            host: host.to_owned(),
            kind,
            reply,
            finish,
            finished,
        })
        .await
        .context(crate::t!("ssh.prompt_unavailable"))?;
    answer
        .recv()
        .await
        .context(crate::t!("ssh.prompt_unavailable"))
}
