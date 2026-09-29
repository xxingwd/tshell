//! Separate auxiliary/ordinary-terminal pools; tmux sessions use dedicated transports.
use crate::tmux_client::HostConfig;
use anyhow::{Context, Result, bail, ensure};
use russh::{Channel, ChannelMsg, client};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock, Weak},
    time::{Duration, Instant},
};
mod blocking;
mod connection;
pub(crate) mod interactive;
pub(crate) use blocking::{BlockingChannel, ChannelGuard, Writer};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PoolKind {
    Terminal,
    Auxiliary,
    Files,
}
type Key = (
    String,
    String,
    Option<u16>,
    Option<std::path::PathBuf>,
    PoolKind,
);
pub(crate) struct Auxiliary {
    _entry: Arc<Entry>,
}
pub(crate) fn auxiliary(host: &HostConfig) -> Auxiliary {
    Auxiliary {
        _entry: entry(host, PoolKind::Auxiliary),
    }
}
struct Entry {
    connection: tokio::sync::Mutex<Vec<connection::Connection>>,
    connecting: tokio::sync::Mutex<()>,
    slots: Arc<tokio::sync::Semaphore>,
    short_slots: Arc<tokio::sync::Semaphore>,
}
impl Drop for Entry {
    fn drop(&mut self) {
        for connection in std::mem::take(self.connection.get_mut()) {
            runtime().spawn(async move {
                let _ = connection
                    .handle
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "last channel closed",
                        "en",
                    )
                    .await;
            });
        }
    }
}
pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("SSH runtime")
    })
}
fn entry(host: &HostConfig, kind: PoolKind) -> Arc<Entry> {
    static POOL: OnceLock<parking_lot::Mutex<BTreeMap<Key, Weak<Entry>>>> = OnceLock::new();
    let mut pool = POOL.get_or_init(Default::default).lock();
    pool.retain(|_, value| value.strong_count() > 0);
    let key = (
        host.destination.clone(),
        host.user.clone(),
        host.port,
        host.identity_file.clone(),
        kind,
    );
    if let Some(entry) = pool.get(&key).and_then(Weak::upgrade) {
        return entry;
    }
    let entry = Arc::new(Entry {
        connection: tokio::sync::Mutex::new(Vec::new()),
        connecting: tokio::sync::Mutex::new(()),
        slots: Arc::new(tokio::sync::Semaphore::new(8)),
        short_slots: Arc::new(tokio::sync::Semaphore::new(4)),
    });
    pool.insert(key, Arc::downgrade(&entry));
    entry
}

pub(crate) struct SharedChannel {
    channel: Option<Channel<client::Msg>>,
    _entry: Arc<Entry>,
    _slot: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl std::ops::Deref for SharedChannel {
    type Target = Channel<client::Msg>;
    fn deref(&self) -> &Self::Target {
        self.channel.as_ref().unwrap()
    }
}
impl std::ops::DerefMut for SharedChannel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.channel.as_mut().unwrap()
    }
}
impl Drop for SharedChannel {
    fn drop(&mut self) {
        if let Some(channel) = self.channel.take() {
            drop(channel.into_stream());
        }
    }
}
impl SharedChannel {
    pub(crate) fn into_stream(
        mut self,
    ) -> impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {
        Stream {
            inner: self.channel.take().unwrap().into_stream(),
            _entry: self._entry.clone(),
            _slot: self._slot.take(),
        }
    }
}
struct Stream {
    inner: russh::ChannelStream<client::Msg>,
    _entry: Arc<Entry>,
    _slot: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl tokio::io::AsyncRead for Stream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
impl tokio::io::AsyncWrite for Stream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub(crate) async fn open(host: &HostConfig) -> Result<SharedChannel> {
    open_entry(host, entry(host, PoolKind::Terminal), false).await
}
pub(crate) async fn open_aux(host: &HostConfig) -> Result<SharedChannel> {
    open_entry(host, entry(host, PoolKind::Auxiliary), true).await
}
pub(crate) async fn open_file(host: &HostConfig) -> Result<SharedChannel> {
    open_entry(host, entry(host, PoolKind::Files), false).await
}

/// Measure an SSH protocol round trip without opening a shell or writing to a PTY.
pub(crate) async fn ping(host: &HostConfig) -> Result<Duration> {
    let pool = entry(host, PoolKind::Auxiliary);
    for attempt in 0..2 {
        let mut connections = pool.connection.lock().await;
        connections.retain(|connection| !connection.handle.is_closed());
        if let Some(connection) = connections.last_mut() {
            let started = Instant::now();
            tokio::time::timeout(Duration::from_secs(10), connection.handle.send_ping())
                .await
                .context(crate::t!("ssh.request_timeout"))??;
            return Ok(started.elapsed());
        }
        drop(connections);
        if attempt == 0 {
            let channel = open_aux(host).await?;
            drop(channel);
        }
    }
    bail!("{}", crate::t!("ssh.channel_closed"));
}

pub(crate) fn ping_async(host: HostConfig) -> async_channel::Receiver<Result<Duration>> {
    let (sender, receiver) = async_channel::bounded(1);
    runtime().spawn(async move {
        let result = ping(&host).await;
        let _ = sender.send(result).await;
    });
    receiver
}
async fn open_entry(
    host: &HostConfig,
    entry: Arc<Entry>,
    auxiliary: bool,
) -> Result<SharedChannel> {
    let host = host.clone();
    // Always create the transport on the process-wide runtime, not a short-lived caller runtime.
    runtime()
        .spawn(async move {
            let slot = if auxiliary {
                Some(
                    tokio::time::timeout(
                        Duration::from_secs(20),
                        entry.slots.clone().acquire_owned(),
                    )
                    .await
                    .context(crate::t!("ssh.helper_timeout"))??,
                )
            } else {
                None
            };
            let channel = if let Some(channel) = open_existing(&entry).await? {
                channel
            } else {
                // Serialize new transports without holding the pool lock while the user answers.
                let _connecting = entry.connecting.lock().await;
                if let Some(channel) = open_existing(&entry).await? {
                    channel
                } else {
                    let count = entry.connection.lock().await.len();
                    if auxiliary && count > 0 {
                        bail!("{}", crate::t!("ssh.channel_refused"));
                    }
                    ensure!(count < 8, "{}", crate::t!("ssh.pool_full", max = 8));
                    let transport = if host.user.is_empty() {
                        tokio::time::timeout(Duration::from_secs(25), connection::connect(&host))
                            .await
                            .context(crate::t!("ssh.auth_timeout"))??
                    } else {
                        connection::connect(&host).await?
                    };
                    // Never replay an exec or a file write after a failed channel open.
                    let channel = tokio::time::timeout(
                        Duration::from_secs(10),
                        transport.handle.channel_open_session(),
                    )
                    .await
                    .context(crate::t!("ssh.channel_timeout"))?
                    .context(crate::t!("ssh.first_channel_refused"))?;
                    entry.connection.lock().await.push(transport);
                    channel
                }
            };
            Ok(SharedChannel {
                channel: Some(channel),
                _entry: entry,
                _slot: slot,
            })
        })
        .await?
}

async fn open_existing(entry: &Entry) -> Result<Option<Channel<client::Msg>>> {
    let mut connections = entry.connection.lock().await;
    connections.retain(|connection| !connection.handle.is_closed());
    for transport in connections.iter().rev() {
        match tokio::time::timeout(
            Duration::from_secs(10),
            transport.handle.channel_open_session(),
        )
        .await
        .context(crate::t!("ssh.channel_timeout"))?
        {
            Ok(channel) => return Ok(Some(channel)),
            Err(russh::Error::ChannelOpenFailure(
                russh::ChannelOpenFailure::ConnectFailed
                | russh::ChannelOpenFailure::ResourceShortage,
            )) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(None)
}

#[cfg(test)]
pub(crate) async fn exec(host: &HostConfig, command: &str) -> Result<SharedChannel> {
    let mut channel = open(host).await?;
    channel.exec(true, command).await?;
    confirmed(&mut channel).await?;
    Ok(channel)
}
pub(crate) async fn exec_aux(host: &HostConfig, command: &str) -> Result<SharedChannel> {
    let mut channel = open_aux(host).await?;
    channel.exec(true, command).await?;
    confirmed(&mut channel).await?;
    Ok(channel)
}
#[cfg(test)]
pub(crate) async fn exec_dedicated(host: &HostConfig, command: &str) -> Result<SharedChannel> {
    let entry = Arc::new(Entry {
        connection: tokio::sync::Mutex::new(Vec::new()),
        connecting: tokio::sync::Mutex::new(()),
        slots: Arc::new(tokio::sync::Semaphore::new(1)),
        short_slots: Arc::new(tokio::sync::Semaphore::new(1)),
    });
    let mut channel = open_entry(host, entry, false).await?;
    channel.exec(true, command).await?;
    confirmed(&mut channel).await?;
    Ok(channel)
}

pub(crate) async fn exec_dedicated_pty(
    host: &HostConfig,
    command: &str,
    cols: u32,
    rows: u32,
) -> Result<SharedChannel> {
    let entry = Arc::new(Entry {
        connection: tokio::sync::Mutex::new(Vec::new()),
        connecting: tokio::sync::Mutex::new(()),
        slots: Arc::new(tokio::sync::Semaphore::new(1)),
        short_slots: Arc::new(tokio::sync::Semaphore::new(1)),
    });
    let mut channel = open_entry(host, entry, false).await?;
    channel
        .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
        .await?;
    confirmed(&mut channel).await?;
    channel.exec(true, command).await?;
    confirmed(&mut channel).await?;
    Ok(channel)
}
pub(crate) async fn confirmed(channel: &mut SharedChannel) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Success) => return Ok(()),
                Some(ChannelMsg::WindowAdjusted { .. }) => continue,
                Some(ChannelMsg::Failure) => bail!("{}", crate::t!("ssh.channel_failed")),
                other => bail!(
                    "{}",
                    crate::t!("ssh.channel_unconfirmed", reply = format!("{other:?}"))
                ),
            }
        }
    })
    .await
    .context(crate::t!("ssh.request_timeout"))?
}
pub(crate) fn output(host: &HostConfig, command: String, limit: usize) -> Result<Vec<u8>> {
    let host = host.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    runtime().spawn(async move {
        let result = async {
            let pool = entry(&host, PoolKind::Auxiliary);
            let _budget = pool.short_slots.clone().acquire_owned().await?;
            let mut channel = exec_aux(&host, &command).await?;
            tokio::time::timeout(Duration::from_secs(20), async {
                channel.eof().await?;
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                let mut status = None;
                while let Some(message) = channel.wait().await {
                    match message {
                        ChannelMsg::Data { data } => {
                            ensure!(
                                stdout.len() + data.len() <= limit,
                                "{}",
                                crate::t!("ssh.response_too_large")
                            );
                            stdout.extend_from_slice(&data);
                        }
                        ChannelMsg::ExtendedData { data, .. } => {
                            stderr.extend(data.iter().take(8192 - stderr.len()));
                        }
                        ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                        ChannelMsg::Close => break,
                        _ => {}
                    }
                }
                ensure!(
                    status == Some(0),
                    "{}",
                    crate::t!(
                        "ssh.command_failed",
                        output = String::from_utf8_lossy(&stderr)
                    )
                );
                Ok(stdout)
            })
            .await
            .context(crate::t!("ssh.command_timeout"))?
        }
        .await;
        let _ = sender.send(result);
    });
    receiver.recv().context(crate::t!("ssh.worker_stopped"))?
}

#[cfg(test)]
mod tests;
